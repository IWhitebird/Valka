//! Connections to Valka nodes: one or more node addresses and an optional API key.
//!
//! Any node of a cluster accepts any request, so a client or worker given several
//! addresses moves to the next one when its node goes away.

use std::time::Duration;

use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::error::SdkError;

/// A channel that adds the API key to every call.
pub type AuthChannel = InterceptedService<Channel, ApiKeyAuth>;

/// Adds `authorization: Bearer <key>` to every call when an API key is set.
#[derive(Clone, Default)]
pub struct ApiKeyAuth(Option<MetadataValue<Ascii>>);

impl ApiKeyAuth {
    pub fn new(api_key: Option<&str>) -> Result<Self, SdkError> {
        api_key
            .filter(|k| !k.is_empty())
            .map(|k| {
                format!("Bearer {k}")
                    .parse::<MetadataValue<Ascii>>()
                    .map_err(|_| SdkError::Connection("the API key is not valid ASCII".into()))
            })
            .transpose()
            .map(Self)
    }
}

impl Interceptor for ApiKeyAuth {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(v) = &self.0 {
            req.metadata_mut().insert("authorization", v.clone());
        }
        Ok(req)
    }
}

/// `VALKA_API_KEY` from the environment, if set.
pub fn api_key_from_env() -> Option<String> {
    std::env::var("VALKA_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// Open a channel to one node.
pub async fn connect_channel(addr: &str, timeout: Duration) -> Result<Channel, SdkError> {
    Ok(Channel::from_shared(addr.to_string())
        .map_err(|e| SdkError::Connection(format!("{addr}: {e}")))?
        .connect_timeout(timeout)
        .http2_keep_alive_interval(Duration::from_secs(10))
        .keep_alive_timeout(Duration::from_secs(5))
        .keep_alive_while_idle(true)
        .connect()
        .await?)
}

/// Whether a failed call may succeed on a retry, possibly on another node: the node is
/// unreachable or busy, or a shard is changing owner.
pub fn is_transient(status: &tonic::Status) -> bool {
    use tonic::Code;
    matches!(
        status.code(),
        Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted | Code::Unknown
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_metadata() {
        let mut auth = ApiKeyAuth::new(Some("k1")).unwrap();
        let req = auth.call(tonic::Request::new(())).unwrap();
        assert_eq!(req.metadata().get("authorization").unwrap(), "Bearer k1");
        let mut none = ApiKeyAuth::new(None).unwrap();
        let req = none.call(tonic::Request::new(())).unwrap();
        assert!(req.metadata().get("authorization").is_none());
        assert!(ApiKeyAuth::new(Some("bad\nkey")).is_err());
    }

    #[test]
    fn transient_codes() {
        assert!(is_transient(&tonic::Status::unavailable("x")));
        assert!(is_transient(&tonic::Status::deadline_exceeded("x")));
        assert!(!is_transient(&tonic::Status::not_found("x")));
        assert!(!is_transient(&tonic::Status::unauthenticated("x")));
        assert!(!is_transient(&tonic::Status::invalid_argument("x")));
    }
}
