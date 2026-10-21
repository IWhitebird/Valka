//! Integration tests. Every test runs against an in-memory object store, so the whole
//! suite needs no external services. The `minio` feature adds tests against a real
//! S3-compatible endpoint.

pub(crate) mod helpers;

mod cluster_api_tests;
mod cluster_routing_tests;
mod cluster_worker_tests;
mod dispatcher_tests;
mod e2e_tests;
mod lifecycle_tests;
mod request_layer_tests;
mod rest_api_tests;
mod security_limits_tests;

#[cfg(feature = "minio")]
mod minio_tests;
