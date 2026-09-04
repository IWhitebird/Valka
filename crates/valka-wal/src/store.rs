//! Thin wrapper over `object_store::ObjectStore` with the handful of primitives the WAL
//! needs: put / create / compare-and-swap / conditional get / list / delete.
//!
//! The wrapper also papers over one backend gap: `LocalFileSystem` does not implement
//! `PutMode::Update`. For the `local` backend (dev, single process) CAS falls back to a
//! process-wide mutex plus an ETag comparison, which is exactly as strong as it needs to
//! be for one process on one directory.

use bytes::Bytes;
use futures::TryStreamExt;
use object_store::{
    GetOptions, ObjectMeta, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload,
    UpdateVersion, local::LocalFileSystem, memory::InMemory, path::Path,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use valka_core::StorageConfig;

use crate::error::WalError;

/// Outcome of a compare-and-swap write.
#[derive(Debug)]
pub enum CasOutcome {
    /// Written; the new ETag.
    Written(Option<String>),
    /// Someone else changed the object first.
    Conflict,
}

/// Outcome of a conditional read.
#[derive(Debug)]
pub enum Conditional {
    NotModified,
    Modified { bytes: Bytes, etag: Option<String> },
    Missing,
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<dyn ObjectStore>,
    prefix: Path,
    /// Backend implements `PutMode::Update` natively.
    native_cas: bool,
    /// Serialises fallback CAS for backends without native support.
    cas_lock: Arc<Mutex<()>>,
    label: String,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("backend", &self.label)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl Store {
    /// In-memory store. Tests and `backend = "memory"`.
    pub fn memory() -> Self {
        Self::wrap(Arc::new(InMemory::new()), "", true, "memory")
    }

    /// A directory on local disk. Creates it if missing.
    pub fn local(path: &str) -> Result<Self, WalError> {
        std::fs::create_dir_all(path)?;
        let fs = LocalFileSystem::new_with_prefix(path)?.with_automatic_cleanup(true);
        Ok(Self::wrap(Arc::new(fs), "", false, "local"))
    }

    /// Wrap any `ObjectStore` (used by tests to inject faults). `native_cas` must say
    /// whether the wrapped store implements `PutMode::Update`.
    pub fn wrap(inner: Arc<dyn ObjectStore>, prefix: &str, native_cas: bool, label: &str) -> Self {
        Self {
            inner,
            prefix: Path::from(prefix),
            native_cas,
            cas_lock: Arc::new(Mutex::new(())),
            label: label.to_string(),
        }
    }

    pub fn from_config(cfg: &StorageConfig) -> Result<Self, WalError> {
        match cfg.backend.as_str() {
            "memory" => Ok(Self::memory()),
            "local" => {
                let s = Self::local(&cfg.path)?;
                Ok(s.with_prefix(&cfg.prefix))
            }
            "s3" => {
                let mut b = object_store::aws::AmazonS3Builder::from_env()
                    .with_bucket_name(&cfg.bucket)
                    .with_allow_http(cfg.allow_http);
                if let Some(ep) = &cfg.endpoint {
                    b = b.with_endpoint(ep);
                }
                if let Some(r) = &cfg.region {
                    b = b.with_region(r);
                }
                if let (Some(k), Some(s)) = (&cfg.access_key_id, &cfg.secret_access_key) {
                    b = b.with_access_key_id(k).with_secret_access_key(s);
                }
                let s3 = b.build()?;
                Ok(Self::wrap(Arc::new(s3), &cfg.prefix, true, "s3"))
            }
            other => Err(WalError::Config(format!(
                "unknown storage backend `{other}`"
            ))),
        }
    }

    pub fn with_prefix(mut self, prefix: &str) -> Self {
        self.prefix = Path::from(prefix);
        self
    }

    pub fn backend_label(&self) -> &str {
        &self.label
    }

    pub fn inner(&self) -> &Arc<dyn ObjectStore> {
        &self.inner
    }

    fn path(&self, key: &str) -> Path {
        if self.prefix.as_ref().is_empty() {
            Path::from(key)
        } else {
            Path::from(format!("{}/{}", self.prefix.as_ref(), key))
        }
    }

    fn strip(&self, p: &Path) -> String {
        let full = p.as_ref();
        if self.prefix.as_ref().is_empty() {
            full.to_string()
        } else {
            full.strip_prefix(self.prefix.as_ref())
                .and_then(|s| s.strip_prefix('/'))
                .unwrap_or(full)
                .to_string()
        }
    }

    /// Unconditional put. Returns the new ETag when the backend reports one.
    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<Option<String>, WalError> {
        let r = self
            .inner
            .put(&self.path(key), PutPayload::from(bytes))
            .await?;
        Ok(r.e_tag)
    }

    /// Create-if-absent. `Ok(None)` means an object already existed.
    pub async fn put_create(
        &self,
        key: &str,
        bytes: Bytes,
    ) -> Result<Option<Option<String>>, WalError> {
        let opts = PutOptions {
            mode: PutMode::Create,
            ..Default::default()
        };
        match self
            .inner
            .put_opts(&self.path(key), PutPayload::from(bytes), opts)
            .await
        {
            Ok(r) => Ok(Some(r.e_tag)),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Compare-and-swap: write only if the current ETag equals `expected`.
    pub async fn put_if_match(
        &self,
        key: &str,
        expected: &str,
        bytes: Bytes,
    ) -> Result<CasOutcome, WalError> {
        let path = self.path(key);
        if self.native_cas {
            let opts = PutOptions {
                mode: PutMode::Update(UpdateVersion {
                    e_tag: Some(expected.to_string()),
                    version: None,
                }),
                ..Default::default()
            };
            return match self
                .inner
                .put_opts(&path, PutPayload::from(bytes), opts)
                .await
            {
                Ok(r) => Ok(CasOutcome::Written(r.e_tag)),
                Err(object_store::Error::Precondition { .. }) => Ok(CasOutcome::Conflict),
                Err(e) => Err(e.into()),
            };
        }
        // Fallback: single-process CAS. Hold the lock across head + put.
        let _guard = self.cas_lock.lock().await;
        let current = match self.inner.head(&path).await {
            Ok(m) => m.e_tag,
            Err(object_store::Error::NotFound { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        if current.as_deref() != Some(expected) {
            return Ok(CasOutcome::Conflict);
        }
        let r = self.inner.put(&path, PutPayload::from(bytes)).await?;
        Ok(CasOutcome::Written(r.e_tag))
    }

    /// Get; `Ok(None)` when missing.
    pub async fn get(&self, key: &str) -> Result<Option<(Bytes, Option<String>)>, WalError> {
        match self.inner.get(&self.path(key)).await {
            Ok(r) => {
                let etag = r.meta.e_tag.clone();
                let bytes = r.bytes().await?;
                Ok(Some((bytes, etag)))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Conditional get: cheap "has it changed?" probe used for ownership verification.
    pub async fn get_if_none_match(&self, key: &str, etag: &str) -> Result<Conditional, WalError> {
        let opts = GetOptions {
            if_none_match: Some(etag.to_string()),
            ..Default::default()
        };
        match self.inner.get_opts(&self.path(key), opts).await {
            Ok(r) => {
                let etag = r.meta.e_tag.clone();
                let bytes = r.bytes().await?;
                Ok(Conditional::Modified { bytes, etag })
            }
            Err(object_store::Error::NotModified { .. }) => Ok(Conditional::NotModified),
            Err(object_store::Error::NotFound { .. }) => Ok(Conditional::Missing),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn head_etag(&self, key: &str) -> Result<Option<Option<String>>, WalError> {
        match self.inner.head(&self.path(key)).await {
            Ok(m) => Ok(Some(m.e_tag)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List every object under `prefix`, sorted by key. Keys are returned relative to the
    /// store prefix.
    pub async fn list(&self, prefix: &str) -> Result<Vec<(String, ObjectMeta)>, WalError> {
        let p = self.path(prefix.trim_end_matches('/'));
        let mut items: Vec<(String, ObjectMeta)> = self
            .inner
            .list(Some(&p))
            .map_ok(|m| (self.strip(&m.location), m))
            .try_collect()
            .await?;
        items.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(items)
    }

    pub async fn delete(&self, key: &str) -> Result<(), WalError> {
        match self.inner.delete(&self.path(key)).await {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_then_cas() {
        let s = Store::memory();
        let etag = s
            .put_create("a", Bytes::from("1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(s.put_create("a", Bytes::from("x")).await.unwrap().is_none());
        match s.put_if_match("a", &etag, Bytes::from("2")).await.unwrap() {
            CasOutcome::Written(_) => {}
            CasOutcome::Conflict => panic!("first CAS must win"),
        }
        match s.put_if_match("a", &etag, Bytes::from("3")).await.unwrap() {
            CasOutcome::Conflict => {}
            CasOutcome::Written(_) => panic!("stale etag must lose"),
        }
        let (b, _) = s.get("a").await.unwrap().unwrap();
        assert_eq!(&b[..], b"2");
    }

    #[tokio::test]
    async fn conditional_get() {
        let s = Store::memory();
        let etag = s.put("k", Bytes::from("v")).await.unwrap().unwrap();
        assert!(matches!(
            s.get_if_none_match("k", &etag).await.unwrap(),
            Conditional::NotModified
        ));
        s.put("k", Bytes::from("w")).await.unwrap();
        assert!(matches!(
            s.get_if_none_match("k", &etag).await.unwrap(),
            Conditional::Modified { .. }
        ));
        assert!(matches!(
            s.get_if_none_match("nope", &etag).await.unwrap(),
            Conditional::Missing
        ));
    }

    #[tokio::test]
    async fn prefix_and_list() {
        let s = Store::memory().with_prefix("tenant/x");
        s.put("wal/n/00000001-0000000000000002.seg", Bytes::from("b"))
            .await
            .unwrap();
        s.put("wal/n/00000001-0000000000000001.seg", Bytes::from("a"))
            .await
            .unwrap();
        s.put("wal/other/00000001-0000000000000001.seg", Bytes::from("c"))
            .await
            .unwrap();
        let l = s.list("wal/n/").await.unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].0.ends_with("0001.seg"));
        assert!(l[0].0.starts_with("wal/n/"));
    }

    #[tokio::test]
    async fn local_fs_cas_fallback() {
        let dir = std::env::temp_dir().join(format!("valka-store-{}", uuid::Uuid::now_v7()));
        let s = Store::local(dir.to_str().unwrap()).unwrap();
        let etag = s
            .put_create("a", Bytes::from("1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            s.put_if_match("a", &etag, Bytes::from("twenty-two"))
                .await
                .unwrap(),
            CasOutcome::Written(_)
        ));
        assert!(matches!(
            s.put_if_match("a", &etag, Bytes::from("3")).await.unwrap(),
            CasOutcome::Conflict
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
