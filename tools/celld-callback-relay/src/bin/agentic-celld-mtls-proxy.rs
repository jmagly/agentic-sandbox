//! Private mTLS front door for the disposable Celld qualification fleet.
//!
//! The proxy terminates a run-private management client certificate and
//! forwards opaque bytes only to Celld's node-loopback plaintext listener.
//! It pins the exact expected client certificate in addition to validating the
//! private CA, so another certificate from the same fixture CA cannot cross
//! the management boundary.

use anyhow::{anyhow, Context, Result};
use std::{
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{io::copy_bidirectional, net::TcpListener, net::TcpStream};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer},
        server::WebPkiClientVerifier,
        RootCertStore, ServerConfig,
    },
    server::TlsStream,
    TlsAcceptor,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyConfig {
    listen: SocketAddr,
    target: SocketAddr,
    ca: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    client_cert: PathBuf,
}

impl ProxyConfig {
    fn parse_from<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut listen = None;
        let mut target = None;
        let mut ca = None;
        let mut cert = None;
        let mut key = None;
        let mut client_cert = None;
        let mut args = args.into_iter().map(Into::into);
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow!("missing value for {flag}"))?;
            let slot = match flag.as_str() {
                "--listen" => &mut listen,
                "--target" => &mut target,
                "--ca" => &mut ca,
                "--cert" => &mut cert,
                "--key" => &mut key,
                "--client-cert" => &mut client_cert,
                _ => return Err(anyhow!("unknown argument {flag}")),
            };
            if slot.replace(value).is_some() {
                return Err(anyhow!("duplicate argument {flag}"));
            }
        }
        let listen: SocketAddr = listen
            .ok_or_else(|| anyhow!("--listen is required"))?
            .parse()
            .context("--listen must be an IP socket address")?;
        if listen.ip().is_loopback() || listen.ip().is_multicast() || listen.port() == 0 {
            return Err(anyhow!("--listen must be a fixed private-namespace socket"));
        }
        let target: SocketAddr = target
            .ok_or_else(|| anyhow!("--target is required"))?
            .parse()
            .context("--target must be an IP socket address")?;
        if !target.ip().is_loopback() || target.port() == 0 {
            return Err(anyhow!("--target must be a fixed node-loopback socket"));
        }
        let absolute = |name: &str, value: Option<String>| -> Result<PathBuf> {
            let path = PathBuf::from(value.ok_or_else(|| anyhow!("{name} is required"))?);
            if !path.is_absolute() {
                return Err(anyhow!("{name} must be absolute"));
            }
            Ok(path)
        };
        Ok(Self {
            listen,
            target,
            ca: absolute("--ca", ca)?,
            cert: absolute("--cert", cert)?,
            key: absolute("--key", key)?,
            client_cert: absolute("--client-cert", client_cert)?,
        })
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?,
    );
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parsing {}", path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("{} contains no certificates", path.display()));
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "{} is not a regular private-key file",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(anyhow!("{} is group/world accessible", path.display()));
        }
    }
    let mut reader = BufReader::new(
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?,
    );
    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("parsing {}", path.display()))?
        .ok_or_else(|| anyhow!("{} contains no private key", path.display()))
}

fn tls_acceptor(config: &ProxyConfig) -> Result<(TlsAcceptor, CertificateDer<'static>)> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = RootCertStore::empty();
    for certificate in load_certs(&config.ca)? {
        roots
            .add(certificate)
            .map_err(|error| anyhow!("invalid proxy client CA: {error}"))?;
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .context("building required proxy client verifier")?;
    let tls = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(load_certs(&config.cert)?, load_key(&config.key)?)
        .context("building Celld proxy server identity")?;
    let mut expected = load_certs(&config.client_cert)?;
    if expected.len() != 1 {
        return Err(anyhow!(
            "expected management client identity must contain one certificate"
        ));
    }
    Ok((TlsAcceptor::from(Arc::new(tls)), expected.remove(0)))
}

fn exact_leaf_certificate(
    certificates: Option<&[CertificateDer<'_>]>,
    expected: &CertificateDer<'_>,
) -> bool {
    matches!(
        certificates,
        Some(certificates) if certificates.first().map(|certificate| certificate.as_ref()) == Some(expected.as_ref())
    )
}

fn exact_client_present(stream: &TlsStream<TcpStream>, expected: &CertificateDer<'static>) -> bool {
    exact_leaf_certificate(stream.get_ref().1.peer_certificates(), expected)
}

async fn proxy_connection(
    inbound: TcpStream,
    acceptor: TlsAcceptor,
    expected_client: CertificateDer<'static>,
    target: SocketAddr,
) -> Result<()> {
    let mut inbound = acceptor
        .accept(inbound)
        .await
        .context("authenticating management mTLS")?;
    if !exact_client_present(&inbound, &expected_client) {
        return Err(anyhow!(
            "management client certificate is not the exact run identity"
        ));
    }
    let mut outbound = TcpStream::connect(target)
        .await
        .context("connecting node-loopback Celld listener")?;
    copy_bidirectional(&mut inbound, &mut outbound)
        .await
        .context("relaying private Celld transport")?;
    Ok(())
}

async fn serve(config: ProxyConfig) -> Result<()> {
    let (acceptor, expected_client) = tls_acceptor(&config)?;
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding private Celld proxy {}", config.listen))?;
    eprintln!("Celld private mTLS proxy ready");
    loop {
        let (inbound, _) = listener.accept().await.context("accepting management")?;
        let acceptor = acceptor.clone();
        let expected_client = expected_client.clone();
        let target = config.target;
        tokio::spawn(async move {
            if proxy_connection(inbound, acceptor, expected_client, target)
                .await
                .is_err()
            {
                eprintln!("Celld private mTLS proxy connection denied");
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ProxyConfig::parse_from(std::env::args().skip(1))?;
    serve(config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_args() -> Vec<&'static str> {
        vec![
            "--listen",
            "0.0.0.0:8443",
            "--target",
            "127.0.0.1:8080",
            "--ca",
            "/run/tls/ca.crt",
            "--cert",
            "/run/tls/server.crt",
            "--key",
            "/run/tls/server.key",
            "--client-cert",
            "/run/tls/management-client.crt",
        ]
    }

    #[test]
    fn accepts_private_tls_to_node_loopback_configuration() {
        let parsed = ProxyConfig::parse_from(valid_args()).unwrap();
        assert!(parsed.listen.ip().is_unspecified());
        assert!(parsed.target.ip().is_loopback());
    }

    #[test]
    fn rejects_plaintext_exposure_and_non_loopback_targets() {
        let mut loopback_listener = valid_args();
        loopback_listener[1] = "127.0.0.1:8443";
        assert!(ProxyConfig::parse_from(loopback_listener).is_err());
        let mut remote_plaintext = valid_args();
        remote_plaintext[3] = "172.30.0.10:8080";
        assert!(ProxyConfig::parse_from(remote_plaintext).is_err());
        let mut duplicate = valid_args();
        duplicate.extend(["--client-cert", "/run/tls/other.crt"]);
        assert!(ProxyConfig::parse_from(duplicate).is_err());
    }

    #[test]
    fn accepts_only_the_exact_expected_management_leaf() {
        let expected = CertificateDer::from(vec![1_u8, 2, 3]);
        let alternate = CertificateDer::from(vec![1_u8, 2, 4]);
        assert!(exact_leaf_certificate(Some(&[expected.clone()]), &expected));
        assert!(!exact_leaf_certificate(Some(&[alternate]), &expected));
        assert!(!exact_leaf_certificate(None, &expected));
    }
}
