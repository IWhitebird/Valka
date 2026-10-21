//! Internal-listener security: the cluster token on every call (always, compared in
//! constant time) and optional mTLS (PHASE2 §11.6).

use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};
use tonic::{Request, Status};
use valka_core::ClusterConfig;

/// Pick rustls' crypto provider once per process. Several are compiled in (the S3 client
/// brings its own), so rustls cannot choose by itself. Safe to call more than once.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Byte-wise comparison whose duration does not depend on where the inputs differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A tonic interceptor that rejects calls without `authorization: Bearer <token>`.
/// With no token configured every call is accepted (single-node or test setups only;
/// clustered mode refuses to start without one).
#[allow(clippy::result_large_err)]
pub fn token_check(
    token: Option<String>,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone + Send + Sync + 'static {
    let expected = token
        .filter(|t| !t.is_empty())
        .map(|t| format!("Bearer {t}").into_bytes());
    move |req: Request<()>| {
        let Some(expected) = &expected else {
            return Ok(req);
        };
        let given = req
            .metadata()
            .get("authorization")
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        if constant_time_eq(given, expected) {
            Ok(req)
        } else {
            Err(Status::unauthenticated("bad or missing cluster token"))
        }
    }
}

fn read(path: &str, what: &str) -> anyhow::Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| anyhow::anyhow!("reading cluster.{what} ({path}): {e}"))
}

/// The three mTLS files, when mTLS is configured. Some but not all of them set is a
/// mistake that must not quietly fall back to plaintext (and send the cluster token in
/// clear): it is an error.
fn tls_files(c: &ClusterConfig) -> anyhow::Result<Option<(&str, &str, &str)>> {
    match (&c.tls_cert, &c.tls_key, &c.tls_ca) {
        (Some(cert), Some(key), Some(ca)) => Ok(Some((cert, key, ca))),
        (None, None, None) => Ok(None),
        _ => anyhow::bail!(
            "cluster mTLS needs all of tls_cert, tls_key and tls_ca \
             (VALKA_CLUSTER__TLS_CERT, _KEY, _CA); only some are set"
        ),
    }
}

/// Server side of mTLS: present our certificate, require one signed by the cluster CA.
pub fn server_tls(c: &ClusterConfig) -> anyhow::Result<Option<ServerTlsConfig>> {
    let Some((cert, key, ca)) = tls_files(c)? else {
        return Ok(None);
    };
    install_crypto_provider();
    Ok(Some(
        ServerTlsConfig::new()
            .identity(Identity::from_pem(
                read(cert, "tls_cert")?,
                read(key, "tls_key")?,
            ))
            .client_ca_root(Certificate::from_pem(read(ca, "tls_ca")?)),
    ))
}

/// Client side of mTLS: trust the cluster CA, present our certificate.
pub fn client_tls(c: &ClusterConfig) -> anyhow::Result<Option<ClientTlsConfig>> {
    let Some((cert, key, ca)) = tls_files(c)? else {
        return Ok(None);
    };
    install_crypto_provider();
    Ok(Some(
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(read(ca, "tls_ca")?))
            .identity(Identity::from_pem(
                read(cert, "tls_cert")?,
                read(key, "tls_key")?,
            )),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_mtls_config_is_an_error() {
        let mut c = ClusterConfig::default();
        assert!(server_tls(&c).unwrap().is_none(), "no mTLS configured");
        c.tls_cert = Some("cert.pem".into());
        c.tls_key = Some("key.pem".into());
        assert!(server_tls(&c).is_err());
        assert!(client_tls(&c).is_err());
    }

    #[test]
    fn token_interceptor() {
        let check = token_check(Some("s3cret".into()));
        let mut ok = Request::new(());
        ok.metadata_mut()
            .insert("authorization", "Bearer s3cret".parse().unwrap());
        assert!(check(ok).is_ok());
        let mut bad = Request::new(());
        bad.metadata_mut()
            .insert("authorization", "Bearer nope".parse().unwrap());
        assert_eq!(check(bad).unwrap_err().code(), tonic::Code::Unauthenticated);
        assert!(check(Request::new(())).is_err());
        assert!(token_check(None)(Request::new(())).is_ok());
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
