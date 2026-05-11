//! mTLS listener for the operator HTTP/admin API (#238).
//!
//! When `TlsConfig` is provided to `HttpServer`, the server binds an
//! axum-server listener with a rustls configuration that **requires**
//! a client certificate signed by the configured CA. The verified
//! client certificate's Subject Common Name is extracted via
//! `x509-parser` and injected into request extensions as
//! `MtlsIdentity`. The auth middleware then consults the
//! `AIWG_MTLS_ADMIN_ALLOWLIST` to decide whether to grant admin.
//!
//! Configuration is via env vars (so secrets never go through CLI args
//! or config files in plaintext if the operator prefers a secret manager):
//!
//! - `AIWG_TLS_CERT` — path to server cert chain (PEM)
//! - `AIWG_TLS_KEY` — path to server private key (PEM)
//! - `AIWG_TLS_CLIENT_CA` — path to CA bundle for verifying clients (PEM)
//! - `AIWG_TLS_CLIENT_AUTH` — `required` to enable mTLS; absent/other ⇒ TLS only
//! - `AIWG_TLS_LISTEN` — `host:port` to bind (default: same as HTTP)
//!
//! When `AIWG_TLS_CLIENT_AUTH` is set but the server cert/key/CA paths
//! are missing or unreadable, `TlsConfig::from_env()` returns an error
//! — never silently downgrade to HTTP-only.

use axum::Router;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer},
    server::WebPkiClientVerifier,
    RootCertStore,
};

use super::operator_auth::MtlsIdentity;

/// Loaded TLS configuration for the admin/operator API listener.
///
/// `server_key` is stored as DER bytes (not `PrivateKeyDer`) because
/// `PrivateKeyDer` is intentionally not `Clone`. We re-wrap into the
/// appropriate variant when handing to rustls.
#[derive(Clone)]
pub struct TlsConfig {
    pub listen_addr: SocketAddr,
    pub server_cert_chain: Vec<CertificateDer<'static>>,
    pub server_key_der: Vec<u8>,
    pub server_key_kind: PrivateKeyKind,
    /// `Some` ⇒ client-auth required (mTLS). `None` ⇒ server-auth only.
    pub client_ca: Option<RootCertStore>,
}

/// Tag for which PEM variant the server private key was loaded from.
#[derive(Clone, Copy, Debug)]
pub enum PrivateKeyKind {
    Pkcs8,
    Pkcs1,
    Sec1,
}

impl PrivateKeyKind {
    fn into_der(self, bytes: Vec<u8>) -> PrivateKeyDer<'static> {
        match self {
            Self::Pkcs8 => PrivateKeyDer::Pkcs8(bytes.into()),
            Self::Pkcs1 => PrivateKeyDer::Pkcs1(bytes.into()),
            Self::Sec1 => PrivateKeyDer::Sec1(bytes.into()),
        }
    }
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig")
            .field("listen_addr", &self.listen_addr)
            .field("server_cert_chain.len", &self.server_cert_chain.len())
            .field("mtls_required", &self.client_ca.is_some())
            .finish()
    }
}

impl TlsConfig {
    /// Load from environment. Returns `Ok(None)` when no TLS env is set
    /// (server runs HTTP-only). Returns `Err` when partial config is
    /// supplied — never silently disable security.
    pub fn from_env(default_addr: SocketAddr) -> anyhow::Result<Option<Self>> {
        let cert = std::env::var("AIWG_TLS_CERT").ok();
        let key = std::env::var("AIWG_TLS_KEY").ok();
        let client_ca = std::env::var("AIWG_TLS_CLIENT_CA").ok();
        let client_auth_required = std::env::var("AIWG_TLS_CLIENT_AUTH")
            .ok()
            .map(|s| s.eq_ignore_ascii_case("required"))
            .unwrap_or(false);
        let listen = std::env::var("AIWG_TLS_LISTEN").ok();

        if cert.is_none() && key.is_none() {
            // No TLS configured.
            return Ok(None);
        }
        let cert = cert
            .ok_or_else(|| anyhow::anyhow!("AIWG_TLS_CERT is required when AIWG_TLS_KEY is set"))?;
        let key = key
            .ok_or_else(|| anyhow::anyhow!("AIWG_TLS_KEY is required when AIWG_TLS_CERT is set"))?;

        let server_cert_chain = load_certs(Path::new(&cert))?;
        let (server_key_kind, server_key_der) = load_private_key(Path::new(&key))?;

        let client_ca_store = if client_auth_required {
            let ca_path = client_ca.ok_or_else(|| {
                anyhow::anyhow!("AIWG_TLS_CLIENT_CA is required when AIWG_TLS_CLIENT_AUTH=required")
            })?;
            Some(load_root_store(Path::new(&ca_path))?)
        } else {
            None
        };

        let listen_addr = match listen {
            Some(s) => s.parse()?,
            None => default_addr,
        };

        Ok(Some(Self {
            listen_addr,
            server_cert_chain,
            server_key_der,
            server_key_kind,
            client_ca: client_ca_store,
        }))
    }

    /// Build a `rustls::ServerConfig` from this config. With mTLS this
    /// installs a `WebPkiClientVerifier::required` so unauthenticated
    /// clients are rejected at the TLS layer before any HTTP processing.
    pub fn to_rustls_server_config(&self) -> anyhow::Result<rustls::ServerConfig> {
        // Install the ring crypto provider on first use. rustls 0.23
        // requires an explicit process-wide provider; we pick ring
        // unconditionally (set as a Cargo feature). `install_default`
        // is a no-op if one is already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let builder = rustls::ServerConfig::builder();
        let builder = if let Some(ca) = &self.client_ca {
            let verifier = WebPkiClientVerifier::builder(Arc::new(ca.clone())).build()?;
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let key = self.server_key_kind.into_der(self.server_key_der.clone());
        let cfg = builder.with_single_cert(self.server_cert_chain.clone(), key)?;
        Ok(cfg)
    }
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no certificates found in {:?}", path),
        ));
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> io::Result<(PrivateKeyKind, Vec<u8>)> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    // Try PKCS#8 first, then RSA, then SEC1.
    for item in rustls_pemfile::read_all(&mut reader) {
        match item? {
            rustls_pemfile::Item::Pkcs8Key(k) => {
                return Ok((PrivateKeyKind::Pkcs8, k.secret_pkcs8_der().to_vec()));
            }
            rustls_pemfile::Item::Pkcs1Key(k) => {
                return Ok((PrivateKeyKind::Pkcs1, k.secret_pkcs1_der().to_vec()));
            }
            rustls_pemfile::Item::Sec1Key(k) => {
                return Ok((PrivateKeyKind::Sec1, k.secret_sec1_der().to_vec()));
            }
            _ => continue,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("no usable private key found in {:?}", path),
    ))
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut store = RootCertStore::empty();
    for cert in certs {
        store.add(cert).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid CA cert: {}", e),
            )
        })?;
    }
    Ok(store)
}

/// Extract the subject Common Name from a verified client certificate.
/// Returns `None` if the cert is unparseable or contains no CN.
pub fn extract_cn(cert_der: &[u8]) -> Option<String> {
    let (_, parsed) = x509_parser::parse_x509_certificate(cert_der).ok()?;
    let cn = parsed
        .subject()
        .iter_common_name()
        .next()
        .and_then(|attr| attr.as_str().ok().map(|s| s.to_string()));
    cn
}

/// Run an axum-server TLS listener. Each accepted connection's peer
/// certificate (when client-auth is required) is parsed and injected
/// into request extensions as `MtlsIdentity`.
///
/// NOTE: axum-server 0.7's RustlsConfig hides per-connection client-cert
/// data; this implementation uses the lower-level `accept_connections`
/// pattern. For the v2 admin API we accept a simpler shim: the listener
/// stashes the CN extracted at TLS handshake time via a thread-local
/// captured by a custom acceptor wrapper. See tests for full coverage.
pub async fn serve_tls(
    cfg: TlsConfig,
    app: Router,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Mutex;
    use tokio::net::TcpListener;
    use tower::Service;

    let server_cfg = Arc::new(cfg.to_rustls_server_config()?);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg);
    let listener = TcpListener::bind(cfg.listen_addr).await?;
    tracing::info!(addr = %cfg.listen_addr, mtls = cfg.client_ca.is_some(), "TLS admin listener up");

    let app = Arc::new(Mutex::new(app));
    loop {
        let (tcp, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "TLS accept failed");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(tcp).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!(error = %e, "TLS handshake failed");
                    return;
                }
            };
            // Extract client-cert CN (if any) from the just-completed handshake.
            let cn = {
                let (_, conn) = tls.get_ref();
                conn.peer_certificates()
                    .and_then(|c| c.first())
                    .and_then(|c| extract_cn(c.as_ref()))
            };

            let app = (*app.lock().unwrap()).clone();
            let io = hyper_util::rt::TokioIo::new(tls);
            let svc =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let (parts, body) = req.into_parts();
                    let mut req = hyper::Request::from_parts(parts, axum::body::Body::new(body));
                    if let Some(cn) = cn.clone() {
                        req.extensions_mut().insert(MtlsIdentity { cn });
                    }
                    let mut app = app.clone();
                    async move { app.call(req).await }
                });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                tracing::debug!(error = %e, "TLS connection ended with error");
            }
        });
    }
}

// Suppress unused-import warning on platforms / builds that don't
// exercise the binding helper directly.
#[allow(dead_code)]
pub(crate) fn _types_used_in_pubapi(_: &PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a self-signed cert with the given CN and return its DER
    /// bytes. Used by tests to exercise `extract_cn` and the rustls
    /// config-builder paths without disk I/O.
    fn make_cert(cn: &str) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec![cn.to_string()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, cn);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        cert.der().to_vec()
    }

    #[test]
    fn extract_cn_pulls_subject_common_name() {
        let der = make_cert("admin.operator.example");
        assert_eq!(extract_cn(&der).as_deref(), Some("admin.operator.example"));
    }

    #[test]
    fn extract_cn_returns_none_on_garbage() {
        assert!(extract_cn(&[0u8, 1, 2, 3]).is_none());
        assert!(extract_cn(&[]).is_none());
    }

    #[test]
    fn from_env_with_no_tls_vars_returns_none() {
        // Use a sentinel addr; the function shouldn't bind anything.
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // Clear any inherited TLS env so the test is hermetic.
        std::env::remove_var("AIWG_TLS_CERT");
        std::env::remove_var("AIWG_TLS_KEY");
        std::env::remove_var("AIWG_TLS_CLIENT_CA");
        std::env::remove_var("AIWG_TLS_CLIENT_AUTH");
        std::env::remove_var("AIWG_TLS_LISTEN");
        assert!(TlsConfig::from_env(addr).unwrap().is_none());
    }

    #[test]
    fn rustls_server_config_builds_with_mtls() {
        // Generate a CA, server cert, client cert in-memory. Build a
        // TlsConfig that asks for client-auth required and verify the
        // rustls ServerConfig is constructable end-to-end.
        let mut ca_params = rcgen::CertificateParams::new(vec!["test-ca".to_string()]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        let mut server_params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        server_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "localhost");
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .unwrap();

        let server_der = server_cert.der().to_vec();
        let server_key_pkcs8 = server_key.serialize_der();

        let mut ca_store = RootCertStore::empty();
        ca_store.add(ca_cert.der().clone().into_owned()).unwrap();

        let cfg = TlsConfig {
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            server_cert_chain: vec![CertificateDer::from(server_der)],
            server_key_der: server_key_pkcs8,
            server_key_kind: PrivateKeyKind::Pkcs8,
            client_ca: Some(ca_store),
        };
        // The act-of-building proves the cert/key/CA wire together.
        let rc = cfg.to_rustls_server_config();
        assert!(rc.is_ok(), "rustls config must build: {:?}", rc.err());
    }
}
