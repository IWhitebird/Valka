//! A fault-injecting `ObjectStore` wrapper for tests.
//!
//! Faults are configured at runtime through a shared [`FaultConfig`] so a test can flip
//! them mid-flight: "let 20 tasks be created, then make every PUT fail, then kill the node".

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, Result, path::Path,
};
use parking_lot::Mutex;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Which operation a fault applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Put,
    Get,
    List,
    Delete,
}

/// Runtime-adjustable fault knobs. Probabilities are per-mille (0..=1000).
#[derive(Debug)]
pub struct FaultConfig {
    /// Every op fails with a generic error with this probability.
    pub fail_per_mille: AtomicU32,
    /// PUTs are performed but the response is replaced with an error ("lost ack").
    pub lost_put_ack_per_mille: AtomicU32,
    /// Added latency per op.
    pub latency_ms: AtomicU64,
    /// When set, PUTs fail unconditionally (bucket unavailable).
    pub puts_down: AtomicBool,
    /// When set, every op hangs forever (node is "frozen" / partitioned from the bucket).
    pub frozen: AtomicBool,
    pub puts: AtomicU64,
    pub gets: AtomicU64,
    pub lists: AtomicU64,
    pub deletes: AtomicU64,
    rng: Mutex<StdRng>,
}

impl FaultConfig {
    pub fn new(seed: u64) -> Arc<Self> {
        Arc::new(Self {
            fail_per_mille: AtomicU32::new(0),
            lost_put_ack_per_mille: AtomicU32::new(0),
            latency_ms: AtomicU64::new(0),
            puts_down: AtomicBool::new(false),
            frozen: AtomicBool::new(false),
            puts: AtomicU64::new(0),
            gets: AtomicU64::new(0),
            lists: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            rng: Mutex::new(StdRng::seed_from_u64(seed)),
        })
    }

    pub fn set_fail_rate(&self, per_mille: u32) {
        self.fail_per_mille.store(per_mille, Ordering::SeqCst);
    }
    pub fn set_lost_put_ack_rate(&self, per_mille: u32) {
        self.lost_put_ack_per_mille
            .store(per_mille, Ordering::SeqCst);
    }
    pub fn set_latency(&self, d: Duration) {
        self.latency_ms
            .store(d.as_millis() as u64, Ordering::SeqCst);
    }
    pub fn set_puts_down(&self, down: bool) {
        self.puts_down.store(down, Ordering::SeqCst);
    }
    pub fn freeze(&self, frozen: bool) {
        self.frozen.store(frozen, Ordering::SeqCst);
    }

    fn roll(&self, per_mille: u32) -> bool {
        if per_mille == 0 {
            return false;
        }
        self.rng.lock().random_range(0..1000u32) < per_mille
    }
}

fn generic(op: Op) -> object_store::Error {
    object_store::Error::Generic {
        store: "faulty",
        source: format!("injected fault on {op:?}").into(),
    }
}

#[derive(Debug)]
pub struct FaultyStore {
    inner: Arc<dyn ObjectStore>,
    cfg: Arc<FaultConfig>,
}

impl FaultyStore {
    pub fn new(inner: Arc<dyn ObjectStore>, cfg: Arc<FaultConfig>) -> Self {
        Self { inner, cfg }
    }

    async fn before(&self, op: Op) -> Result<()> {
        if self.cfg.frozen.load(Ordering::SeqCst) {
            futures::future::pending::<()>().await;
        }
        let lat = self.cfg.latency_ms.load(Ordering::SeqCst);
        if lat > 0 {
            tokio::time::sleep(Duration::from_millis(lat)).await;
        }
        if op == Op::Put && self.cfg.puts_down.load(Ordering::SeqCst) {
            return Err(generic(op));
        }
        if self
            .cfg
            .roll(self.cfg.fail_per_mille.load(Ordering::SeqCst))
        {
            return Err(generic(op));
        }
        Ok(())
    }
}

impl std::fmt::Display for FaultyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FaultyStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for FaultyStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        self.cfg.puts.fetch_add(1, Ordering::Relaxed);
        self.before(Op::Put).await?;
        let r = self.inner.put_opts(location, payload, opts).await?;
        if self
            .cfg
            .roll(self.cfg.lost_put_ack_per_mille.load(Ordering::SeqCst))
        {
            return Err(generic(Op::Put));
        }
        Ok(r)
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        self.before(Op::Put).await?;
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        self.cfg.gets.fetch_add(1, Ordering::Relaxed);
        self.before(Op::Get).await?;
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path>>,
    ) -> BoxStream<'static, Result<Path>> {
        let cfg = self.cfg.clone();
        let gated = locations.then(move |p| {
            let cfg = cfg.clone();
            async move {
                cfg.deletes.fetch_add(1, Ordering::Relaxed);
                if cfg.frozen.load(Ordering::SeqCst) {
                    futures::future::pending::<()>().await;
                }
                if cfg.roll(cfg.fail_per_mille.load(Ordering::SeqCst)) {
                    return Err(generic(Op::Delete));
                }
                p
            }
        });
        self.inner.delete_stream(Box::pin(gated))
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        self.cfg.lists.fetch_add(1, Ordering::Relaxed);
        if self.cfg.frozen.load(Ordering::SeqCst) {
            return Box::pin(futures::stream::pending());
        }
        if self
            .cfg
            .roll(self.cfg.fail_per_mille.load(Ordering::SeqCst))
        {
            return Box::pin(futures::stream::once(async { Err(generic(Op::List)) }));
        }
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        self.before(Op::List).await?;
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> Result<()> {
        self.before(Op::Put).await?;
        self.inner.copy_opts(from, to, options).await
    }
}

/// Convenience: an in-memory store wrapped in a fault injector, plus its knobs.
pub fn faulty_memory_store(seed: u64) -> (crate::Store, Arc<FaultConfig>) {
    let cfg = FaultConfig::new(seed);
    let inner: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
    let faulty = FaultyStore::new(inner, cfg.clone());
    (
        crate::Store::wrap(Arc::new(faulty), "", true, "faulty-memory"),
        cfg,
    )
}

/// Same as [`faulty_memory_store`] but sharing an existing backing store, so a "new node"
/// can be started against the same bucket after a simulated crash.
pub fn faulty_over(inner: Arc<dyn ObjectStore>, seed: u64) -> (crate::Store, Arc<FaultConfig>) {
    let cfg = FaultConfig::new(seed);
    let faulty = FaultyStore::new(inner, cfg.clone());
    (
        crate::Store::wrap(Arc::new(faulty), "", true, "faulty"),
        cfg,
    )
}
