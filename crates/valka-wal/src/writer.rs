//! Group-commit WAL writer.
//!
//! ```text
//! append() ──mpsc──► collector ──(segment, acks)──mpsc──► committer ──► resolve acks
//!                      │ encodes + spawns PUT                │ awaits PUTs in seq order
//!                      │ (bounded by max_inflight)           │ verifies ownership
//! ```
//!
//! Acks are resolved strictly in segment order, so "durable LSN" is always a prefix.

use bytes::Bytes;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

use crate::error::WalError;
use crate::lsn::{Lsn, keys};
use crate::ownership::{OwnershipCheck, OwnershipVerdict};
use crate::record::Envelope;
use crate::segment;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct WalWriterConfig {
    pub flush_interval: Duration,
    pub max_batch_bytes: usize,
    pub max_inflight: usize,
    pub put_retries: u32,
    pub compress: bool,
}

impl Default for WalWriterConfig {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(50),
            max_batch_bytes: 4 * 1024 * 1024,
            max_inflight: 4,
            put_retries: 8,
            compress: true,
        }
    }
}

impl WalWriterConfig {
    pub fn from_core(c: &valka_core::WalConfig) -> Self {
        Self {
            flush_interval: Duration::from_millis(c.flush_interval_ms),
            max_batch_bytes: c.max_batch_bytes,
            max_inflight: c.max_inflight_segments.max(1),
            put_retries: c.put_retries,
            compress: true,
        }
    }
}

/// Resolves once the appended records are durable (or failed).
pub struct Durable(oneshot::Receiver<Result<Lsn, WalError>>);

impl Durable {
    pub async fn wait(self) -> Result<Lsn, WalError> {
        self.0.await.unwrap_or(Err(WalError::WriterClosed))
    }
}

struct Pending {
    records: Vec<Envelope>,
    ack: oneshot::Sender<Result<Lsn, WalError>>,
}

struct InFlight {
    lsn: Lsn,
    record_count: u64,
    shards: Vec<valka_core::ShardId>,
    acks: Vec<oneshot::Sender<Result<Lsn, WalError>>>,
    put: JoinHandle<Result<(), WalError>>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Records appended but not yet acknowledged, and when the current backlog started.
#[derive(Default)]
struct Backlog {
    records: u64,
    since: Option<tokio::time::Instant>,
}

/// Cloneable handle. Dropping the last handle shuts the writer down after draining.
#[derive(Clone)]
pub struct WalWriter {
    tx: mpsc::UnboundedSender<Pending>,
    durable: watch::Receiver<Lsn>,
    next_lsn: Arc<AtomicU64>,
    epoch: u32,
    /// Set when the committer gave up on a segment; the engine must restart.
    poisoned: Arc<watch::Sender<Option<String>>>,
    backlog: Arc<Mutex<Backlog>>,
}

impl WalWriter {
    /// Start the writer. `start_lsn` is the first LSN this writer will use (recovery passes
    /// `last_seen.next()`; a fresh node passes `Lsn::new(epoch, 1)`).
    pub fn start(
        store: Store,
        node_id: String,
        start_lsn: Lsn,
        cfg: WalWriterConfig,
        ownership: Arc<dyn OwnershipCheck>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<Pending>();
        let (durable_tx, durable_rx) =
            watch::channel(Lsn::new(start_lsn.epoch, start_lsn.seq.saturating_sub(1)));
        let (flight_tx, flight_rx) = mpsc::channel::<InFlight>(cfg.max_inflight * 2 + 1);
        let next_lsn = Arc::new(AtomicU64::new(start_lsn.seq));
        let poisoned = Arc::new(watch::Sender::new(None));

        tokio::spawn(collector(
            rx,
            flight_tx,
            store.clone(),
            node_id,
            start_lsn.epoch,
            next_lsn.clone(),
            cfg.clone(),
        ));
        let backlog = Arc::new(Mutex::new(Backlog::default()));
        tokio::spawn(committer(
            flight_rx,
            durable_tx,
            ownership,
            poisoned.clone(),
            backlog.clone(),
        ));

        Self {
            tx,
            durable: durable_rx,
            next_lsn,
            epoch: start_lsn.epoch,
            poisoned,
            backlog,
        }
    }

    /// Queue records for the next segment. Returns a future that resolves when durable.
    pub fn append(&self, records: Vec<Envelope>) -> Durable {
        let (ack_tx, ack_rx) = oneshot::channel();
        if let Some(reason) = self.poisoned.borrow().clone() {
            let _ = ack_tx.send(Err(WalError::NotDurable(reason)));
            return Durable(ack_rx);
        }
        {
            let mut b = self.backlog.lock();
            if b.records == 0 && !records.is_empty() {
                b.since = Some(tokio::time::Instant::now());
            }
            b.records += records.len() as u64;
        }
        if self
            .tx
            .send(Pending {
                records,
                ack: ack_tx,
            })
            .is_err()
        {
            // Collector gone; ack_rx will observe the dropped sender.
        }
        Durable(ack_rx)
    }

    /// Records appended but not yet acknowledged as durable.
    pub fn unflushed_records(&self) -> u64 {
        self.backlog.lock().records
    }

    /// Age of the oldest unacknowledged write. Approximate: measured from when the
    /// backlog last went from empty to non-empty.
    pub fn oldest_unacked(&self) -> Option<Duration> {
        let b = self.backlog.lock();
        if b.records == 0 {
            None
        } else {
            b.since.map(|s| s.elapsed())
        }
    }

    /// Highest LSN whose acks have been released. Never regresses.
    pub fn durable_lsn(&self) -> Lsn {
        *self.durable.borrow()
    }

    pub fn durable_watch(&self) -> watch::Receiver<Lsn> {
        self.durable.clone()
    }

    /// The LSN the *next* segment will get. Used to stamp snapshots exactly.
    pub fn next_lsn(&self) -> Lsn {
        Lsn::new(self.epoch, self.next_lsn.load(Ordering::SeqCst))
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn poisoned(&self) -> Option<String> {
        self.poisoned.borrow().clone()
    }

    /// Changes once, from `None` to the reason, when the writer is poisoned.
    pub fn poison_watch(&self) -> watch::Receiver<Option<String>> {
        self.poisoned.subscribe()
    }

    /// Wait until everything appended so far is durable.
    pub async fn sync(&self) -> Result<Lsn, WalError> {
        self.append(Vec::new()).wait().await
    }
}

async fn collector(
    mut rx: mpsc::UnboundedReceiver<Pending>,
    flight_tx: mpsc::Sender<InFlight>,
    store: Store,
    node_id: String,
    epoch: u32,
    next_lsn: Arc<AtomicU64>,
    cfg: WalWriterConfig,
) {
    let semaphore = Arc::new(Semaphore::new(cfg.max_inflight));
    let mut records: Vec<Envelope> = Vec::new();
    let mut acks: Vec<oneshot::Sender<Result<Lsn, WalError>>> = Vec::new();
    let mut approx_bytes = 0usize;
    let mut deadline: Option<tokio::time::Instant> = None;

    loop {
        let recv = rx.recv();
        let item = match deadline {
            Some(d) => tokio::select! {
                biased;
                it = recv => Some(it),
                _ = tokio::time::sleep_until(d) => None,
            },
            None => Some(recv.await),
        };

        match item {
            Some(Some(p)) => {
                if deadline.is_none() {
                    deadline = Some(tokio::time::Instant::now() + cfg.flush_interval);
                }
                approx_bytes += p.records.iter().map(approx_size).sum::<usize>();
                records.extend(p.records);
                acks.push(p.ack);
                if approx_bytes < cfg.max_batch_bytes {
                    continue;
                }
            }
            Some(None) => {
                // All handles dropped: flush what we have and exit.
                if !acks.is_empty() {
                    flush(
                        &store,
                        &node_id,
                        epoch,
                        &next_lsn,
                        &cfg,
                        &semaphore,
                        &flight_tx,
                        std::mem::take(&mut records),
                        std::mem::take(&mut acks),
                    )
                    .await;
                }
                debug!("WAL collector exiting");
                return;
            }
            None => {} // deadline hit
        }

        deadline = None;
        approx_bytes = 0;
        flush(
            &store,
            &node_id,
            epoch,
            &next_lsn,
            &cfg,
            &semaphore,
            &flight_tx,
            std::mem::take(&mut records),
            std::mem::take(&mut acks),
        )
        .await;
    }
}

fn approx_size(e: &Envelope) -> usize {
    // Cheap estimate; exact size is only known after serialisation.
    128 + match &e.record {
        crate::record::WalRecord::TaskCreated { task } => {
            task.input
                .as_ref()
                .map(|v| v.to_string().len())
                .unwrap_or(0)
                + task.metadata.to_string().len()
        }
        crate::record::WalRecord::RunCompleted { output, .. } => {
            output.as_ref().map(|v| v.to_string().len()).unwrap_or(0)
        }
        _ => 0,
    }
}

#[allow(clippy::too_many_arguments)]
async fn flush(
    store: &Store,
    node_id: &str,
    epoch: u32,
    next_lsn: &AtomicU64,
    cfg: &WalWriterConfig,
    semaphore: &Arc<Semaphore>,
    flight_tx: &mpsc::Sender<InFlight>,
    records: Vec<Envelope>,
    acks: Vec<oneshot::Sender<Result<Lsn, WalError>>>,
) {
    // Empty batches (sync()) still get a segment so the durable LSN advances and the
    // ownership check runs. They are tiny.
    let permit = match semaphore.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => return,
    };
    let seq = next_lsn.fetch_add(1, Ordering::SeqCst);
    let lsn = Lsn::new(epoch, seq);
    let mut shards: Vec<valka_core::ShardId> = records.iter().map(|r| r.shard).collect();
    shards.sort_unstable();
    shards.dedup();
    let record_count = records.len() as u64;

    let bytes = match segment::encode(lsn, node_id, &records, cfg.compress) {
        Ok(b) => b,
        Err(e) => {
            for a in acks {
                let _ = a.send(Err(WalError::Encode(e.to_string())));
            }
            return;
        }
    };
    let key = keys::segment(node_id, lsn);
    let store2 = store.clone();
    let retries = cfg.put_retries;
    let put = tokio::spawn(async move { put_with_retry(&store2, &key, bytes, retries).await });

    if flight_tx
        .send(InFlight {
            lsn,
            record_count,
            shards,
            acks,
            put,
            _permit: permit,
        })
        .await
        .is_err()
    {
        warn!("WAL committer gone; dropping in-flight segment");
    }
}

/// Segments are written create-if-absent: a segment position is written exactly once, so
/// a late PUT from a fenced-out writer can never overwrite a sealed position (phase 2).
/// `AlreadyExists` after a retry is ours only if the stored bytes are ours: an earlier
/// attempt may have landed with its response lost, or someone else may have taken the
/// position meanwhile.
async fn put_with_retry(
    store: &Store,
    key: &str,
    bytes: Bytes,
    retries: u32,
) -> Result<(), WalError> {
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(20);
    loop {
        let err = match store.put_create(key, bytes.clone()).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if attempt == 0 => {
                return Err(WalError::NotDurable(format!(
                    "{key}: segment already exists (another writer holds this log)"
                )));
            }
            Ok(None) => match store.get(key).await {
                Ok(Some((stored, _))) if stored == bytes => return Ok(()),
                Ok(_) => {
                    return Err(WalError::NotDurable(format!(
                        "{key}: position taken by another writer"
                    )));
                }
                Err(e) => e,
            },
            Err(e) => e,
        };
        if attempt >= retries {
            return Err(WalError::NotDurable(format!("{key}: {err}")));
        }
        attempt += 1;
        warn!(key, attempt, error = %err, "segment PUT failed, retrying");
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(2));
    }
}

fn settle_backlog(backlog: &Mutex<Backlog>, records: u64) {
    let mut b = backlog.lock();
    b.records = b.records.saturating_sub(records);
    if b.records == 0 {
        b.since = None;
    }
}

async fn committer(
    mut rx: mpsc::Receiver<InFlight>,
    durable_tx: watch::Sender<Lsn>,
    ownership: Arc<dyn OwnershipCheck>,
    poisoned: Arc<watch::Sender<Option<String>>>,
    backlog: Arc<Mutex<Backlog>>,
) {
    while let Some(f) = rx.recv().await {
        let result = match f.put.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(join) => Err(WalError::NotDurable(format!("put task panicked: {join}"))),
        };

        let result = match result {
            Ok(()) => match ownership.verify(&f.shards).await {
                Ok(OwnershipVerdict::Unchanged) => Ok(()),
                Ok(OwnershipVerdict::Lost(lost)) => {
                    warn!(?lost, lsn = %f.lsn, "ownership lost for shards in segment");
                    Err(WalError::OwnershipLost)
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };

        settle_backlog(&backlog, f.record_count);
        match result {
            Ok(()) => {
                for a in f.acks {
                    let _ = a.send(Ok(f.lsn));
                }
                let _ = durable_tx.send(f.lsn);
            }
            Err(e) => {
                error!(lsn = %f.lsn, error = %e, "segment not committed; writer poisoned");
                let msg = e.to_string();
                poisoned.send_replace(Some(msg.clone()));
                for a in f.acks {
                    let _ = a.send(Err(WalError::NotDurable(msg.clone())));
                }
                // Fail everything behind us too: durable LSN must stay a prefix.
                while let Some(g) = rx.recv().await {
                    settle_backlog(&backlog, g.record_count);
                    g.put.abort();
                    for a in g.acks {
                        let _ = a.send(Err(WalError::NotDurable(msg.clone())));
                    }
                }
                return;
            }
        }
    }
    debug!("WAL committer exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault::faulty_memory_store;
    use crate::ownership::SingleNodeOwnership;
    use crate::reader;
    use crate::record::WalRecord;
    use valka_core::ShardId;

    fn rec(i: u32) -> Envelope {
        Envelope::new(
            ShardId(i as u16 % 4096),
            WalRecord::TaskPromoted {
                task_id: format!("t{i}"),
            },
        )
    }

    fn cfg() -> WalWriterConfig {
        WalWriterConfig {
            flush_interval: Duration::from_millis(10),
            ..Default::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retried_put_accepts_its_own_landed_bytes() {
        let (store, faults) = faulty_memory_store(3);
        let key = "wal/n/00000001-0000000000000001.seg";
        let ours = Bytes::from_static(b"ours");
        faults.set_lost_put_ack_rate(1000);
        let s = store.clone();
        let put = tokio::spawn({
            let ours = ours.clone();
            async move { put_with_retry(&s, key, ours, 5).await }
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        faults.set_lost_put_ack_rate(0);
        put.await.unwrap().expect("the landed bytes are ours");
        assert_eq!(store.get(key).await.unwrap().unwrap().0, ours);
    }

    #[tokio::test(start_paused = true)]
    async fn retried_put_rejects_a_position_taken_by_another_writer() {
        let (store, faults) = faulty_memory_store(4);
        let key = "wal/n/00000001-0000000000000001.seg";
        faults.set_puts_down(true);
        let s = store.clone();
        let put =
            tokio::spawn(
                async move { put_with_retry(&s, key, Bytes::from_static(b"ours"), 5).await },
            );
        tokio::time::sleep(Duration::from_millis(5)).await;
        faults.set_puts_down(false);
        store
            .put_create(key, Bytes::from_static(b"seal"))
            .await
            .unwrap()
            .expect("the sealer takes the free position");
        let err = put.await.unwrap().unwrap_err();
        assert!(
            err.to_string().contains("taken by another writer"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn appends_become_durable_and_readable() {
        let store = Store::memory();
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            cfg(),
            Arc::new(SingleNodeOwnership),
        );
        let d1 = w.append(vec![rec(1), rec(2)]);
        let d2 = w.append(vec![rec(3)]);
        let l1 = d1.wait().await.unwrap();
        let l2 = d2.wait().await.unwrap();
        assert_eq!(l1, l2, "same group commit");
        assert_eq!(w.durable_lsn(), l1);

        let all = reader::read_all(&store, "n", None).await.unwrap();
        let ids: Vec<_> = all
            .iter()
            .map(|(_, e)| e.record.task_id().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["t1", "t2", "t3"]);
    }

    #[tokio::test]
    async fn size_threshold_flushes_early() {
        let store = Store::memory();
        let c = WalWriterConfig {
            flush_interval: Duration::from_secs(60),
            max_batch_bytes: 1024,
            ..Default::default()
        };
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            c,
            Arc::new(SingleNodeOwnership),
        );
        let batch: Vec<Envelope> = (0..20).map(rec).collect();
        let lsn = tokio::time::timeout(Duration::from_secs(2), w.append(batch).wait())
            .await
            .expect("must flush by size, not by the 60s timer")
            .unwrap();
        assert_eq!(lsn.seq, 1);
    }

    #[tokio::test]
    async fn acks_release_in_order_under_faults() {
        let (store, faults) = faulty_memory_store(7);
        faults.set_fail_rate(300);
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            cfg(),
            Arc::new(SingleNodeOwnership),
        );
        let mut waits = Vec::new();
        for i in 0..50u32 {
            waits.push(w.append(vec![rec(i)]));
            if i % 7 == 0 {
                tokio::time::sleep(Duration::from_millis(12)).await;
            }
        }
        let mut last = Lsn::ZERO;
        for d in waits {
            let l = d.wait().await.unwrap();
            assert!(l >= last, "acks must be monotonic");
            last = l;
        }
        faults.set_fail_rate(0);
        let all = reader::read_all(&store, "n", None).await.unwrap();
        assert_eq!(all.len(), 50);
        // segments are contiguous
        let mut seqs: Vec<u64> = all.iter().map(|(l, _)| l.seq).collect();
        seqs.dedup();
        for w in seqs.windows(2) {
            assert_eq!(w[1], w[0] + 1);
        }
    }

    #[tokio::test]
    async fn backlog_counters_track_unacked_records() {
        let (store, faults) = faulty_memory_store(5);
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            cfg(),
            Arc::new(SingleNodeOwnership),
        );
        assert_eq!(w.unflushed_records(), 0);
        assert!(w.oldest_unacked().is_none());
        // Slow PUTs: the records stay in the backlog until the segment lands.
        faults.set_latency(Duration::from_millis(300));
        let d = w.append(vec![rec(1), rec(2)]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(w.unflushed_records(), 2);
        assert!(w.oldest_unacked().unwrap() >= Duration::from_millis(50));
        d.wait().await.unwrap();
        assert_eq!(w.unflushed_records(), 0);
        assert!(w.oldest_unacked().is_none());
    }

    #[tokio::test]
    async fn bucket_down_fails_acks_and_poisons() {
        let (store, faults) = faulty_memory_store(1);
        let c = WalWriterConfig {
            flush_interval: Duration::from_millis(5),
            put_retries: 2,
            ..Default::default()
        };
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            c,
            Arc::new(SingleNodeOwnership),
        );
        let mut poison = w.poison_watch();
        assert!(poison.borrow().is_none());
        faults.set_puts_down(true);
        let err = w.append(vec![rec(1)]).wait().await.unwrap_err();
        assert!(matches!(err, WalError::NotDurable(_)));
        assert!(w.poisoned().is_some());
        poison.changed().await.unwrap();
        assert_eq!(*poison.borrow(), w.poisoned());
        // subsequent appends fail fast
        assert!(w.append(vec![rec(2)]).wait().await.is_err());
        faults.set_puts_down(false);
        assert!(
            reader::read_all(&store, "n", None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn lost_put_ack_is_retried_idempotently() {
        let (store, faults) = faulty_memory_store(3);
        faults.set_lost_put_ack_rate(500);
        let w = WalWriter::start(
            store.clone(),
            "n".into(),
            Lsn::new(1, 1),
            cfg(),
            Arc::new(SingleNodeOwnership),
        );
        for i in 0..20u32 {
            w.append(vec![rec(i)]).wait().await.unwrap();
        }
        faults.set_lost_put_ack_rate(0);
        let all = reader::read_all(&store, "n", None).await.unwrap();
        assert_eq!(
            all.len(),
            20,
            "retrying a PUT of the same key never duplicates records"
        );
    }
}
