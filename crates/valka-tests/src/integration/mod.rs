//! Integration tests. Every test runs against an in-memory object store, so the whole
//! suite needs no external services. The `minio` feature adds tests against a real
//! S3-compatible endpoint.

mod helpers;

mod dispatcher_tests;
mod e2e_tests;
mod lifecycle_tests;
mod rest_api_tests;

#[cfg(feature = "minio")]
mod minio_tests;
