//! Public API authentication with API keys.
//!
//! `VALKA_API_KEYS` (comma-separated) turns it on: every REST call under `/api/v1` and every
//! gRPC call to `ApiService` and `WorkerService` must then carry `Authorization: Bearer
//! <key>`. Keys are compared in constant time. The admin routes keep their own token
//! (`VALKA_ADMIN_TOKEN`); `/healthz`, `/metrics`, the dashboard's static files and the gRPC
//! health and reflection services stay open. With no keys configured the API is open, and
//! the server says so loudly at startup.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The configured API keys, or none (open API).
#[derive(Clone, Default)]
pub struct ApiKeys(Option<Arc<Vec<String>>>);

impl ApiKeys {
    /// No keys: every request is allowed.
    pub fn open() -> Self {
        Self(None)
    }

    /// Keys from a comma-separated list; an empty or missing list means open.
    pub fn parse(list: Option<&str>) -> Self {
        let keys: Vec<String> = list
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect();
        if keys.is_empty() {
            Self(None)
        } else {
            Self(Some(Arc::new(keys)))
        }
    }

    pub fn is_open(&self) -> bool {
        self.0.is_none()
    }

    /// Whether `presented` (the bearer token, if any) may use the API.
    pub fn allows(&self, presented: Option<&str>) -> bool {
        match &self.0 {
            None => true,
            Some(keys) => presented.is_some_and(|p| {
                // Check every key so the time taken does not reveal which one matched.
                keys.iter().fold(false, |ok, k| {
                    valka_cluster::security::constant_time_eq(k.as_bytes(), p.as_bytes()) | ok
                })
            }),
        }
    }
}

/// The token in an `Authorization: Bearer <token>` header value.
pub fn bearer(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// `access_token` from a query string. Only the event stream accepts it: a browser's
/// `EventSource` cannot send headers.
fn query_token(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == "access_token").then(|| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned()
        })
    })
}

/// REST middleware for the public API routes.
pub async fn require_api_key(State(keys): State<ApiKeys>, req: Request, next: Next) -> Response {
    if keys.is_open() {
        return next.run(req).await;
    }
    let header_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer)
        .map(str::to_string);
    let token = header_token.or_else(|| {
        (req.uri().path() == "/api/v1/events")
            .then(|| query_token(req.uri().query()))
            .flatten()
    });
    if keys.allows(token.as_deref()) {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(serde_json::json!({
            "error": "missing or invalid API key: send Authorization: Bearer <key>",
            "code": "UNAUTHENTICATED",
        })),
    )
        .into_response()
}

/// gRPC interceptor for `ApiService` and `WorkerService`.
#[derive(Clone)]
pub struct GrpcAuth(pub ApiKeys);

impl tonic::service::Interceptor for GrpcAuth {
    fn call(&mut self, req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if self.0.is_open() {
            return Ok(req);
        }
        let token = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(bearer);
        if self.0.allows(token) {
            Ok(req)
        } else {
            Err(tonic::Status::unauthenticated(
                "missing or invalid API key: send `authorization: Bearer <key>` metadata",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_and_check() {
        assert!(ApiKeys::parse(None).is_open());
        assert!(ApiKeys::parse(Some(" , ")).is_open());
        let k = ApiKeys::parse(Some("alpha, beta"));
        assert!(!k.is_open());
        assert!(k.allows(Some("alpha")));
        assert!(k.allows(Some("beta")));
        assert!(!k.allows(Some("gamma")));
        assert!(!k.allows(Some("alph")));
        assert!(!k.allows(None));
        assert!(ApiKeys::open().allows(None));
    }

    #[test]
    fn bearer_tokens() {
        assert_eq!(bearer("Bearer abc"), Some("abc"));
        assert_eq!(bearer("bearer  abc "), Some("abc"));
        assert_eq!(bearer("Basic abc"), None);
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(
            query_token(Some("queue=a&access_token=k%2By")),
            Some("k+y".into())
        );
        assert_eq!(query_token(Some("queue=a")), None);
    }
}
