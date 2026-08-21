//! Node-local callback relay for the disposable Celld qualification fleet.
//!
//! The Worker connects to loopback HTTP inside the Celld node's network
//! namespace. This process forwards the opaque TCP stream to management over
//! private-CA mTLS. It does not parse, log, or retain callback bytes.

use anyhow::{anyhow, Context, Result};
use std::{
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName},
        ClientConfig, RootCertStore,
    },
    TlsConnector,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayConfig {
    listen: SocketAddr,
    target: SocketAddr,
    server_name: String,
    ca: PathBuf,
    cert: PathBuf,
    key: PathBuf,
}

impl RelayConfig {
    fn parse_from<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut listen = None;
        let mut target = None;
        let mut server_name = None;
        let mut ca = None;
        let mut cert = None;
        let mut key = None;
        let mut args = args.into_iter().map(Into::into);
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow!("missing value for {flag}"))?;
            let slot = match flag.as_str() {
                "--listen" => &mut listen,
                "--target" => &mut target,
                "--server-name" => &mut server_name,
                "--ca" => &mut ca,
                "--cert" => &mut cert,
                "--key" => &mut key,
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
        if !listen.ip().is_loopback() || listen.port() == 0 {
            return Err(anyhow!("--listen must be a fixed loopback socket"));
        }
        let target: SocketAddr = target
            .ok_or_else(|| anyhow!("--target is required"))?
            .parse()
            .context("--target must be an IP socket address")?;
        if target.ip().is_unspecified() || target.ip().is_multicast() || target.port() == 0 {
            return Err(anyhow!("--target must be a fixed unicast socket"));
        }
        let server_name = server_name.ok_or_else(|| anyhow!("--server-name is required"))?;
        ServerName::try_from(server_name.clone())
            .map_err(|_| anyhow!("--server-name is not a valid TLS DNS name"))?;
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
            server_name,
            ca: absolute("--ca", ca)?,
            cert: absolute("--cert", cert)?,
            key: absolute("--key", key)?,
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

fn tls_connector(config: &RelayConfig) -> Result<TlsConnector> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = RootCertStore::empty();
    for certificate in load_certs(&config.ca)? {
        roots
            .add(certificate)
            .map_err(|error| anyhow!("invalid relay CA: {error}"))?;
    }
    let tls = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(load_certs(&config.cert)?, load_key(&config.key)?)
        .context("building callback relay mTLS identity")?;
    Ok(TlsConnector::from(Arc::new(tls)))
}

async fn serve(config: RelayConfig) -> Result<()> {
    let connector = tls_connector(&config)?;
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding relay loopback {}", config.listen))?;
    eprintln!("Celld callback relay ready on loopback");
    loop {
        let (mut inbound, peer) = listener.accept().await.context("accepting callback")?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let connector = connector.clone();
        let target = config.target;
        let server_name =
            ServerName::try_from(config.server_name.clone()).expect("validated TLS server name");
        tokio::spawn(async move {
            let result = async {
                let outbound = TcpStream::connect(target)
                    .await
                    .context("connecting management")?;
                let mut outbound = connector
                    .connect(server_name, outbound)
                    .await
                    .context("authenticating management mTLS")?;
                copy_bidirectional(&mut inbound, &mut outbound)
                    .await
                    .context("relaying callback")?;
                Result::<()>::Ok(())
            }
            .await;
            if result.is_err() {
                eprintln!("Celld callback relay connection failed");
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = RelayConfig::parse_from(std::env::args().skip(1))?;
    serve(config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair};
    use std::{fs::write, os::unix::fs::PermissionsExt, time::Duration};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::rustls::{
        pki_types::PrivatePkcs8KeyDer, server::WebPkiClientVerifier, ServerConfig,
    };
    use tokio_rustls::TlsAcceptor;

    fn valid_args() -> Vec<&'static str> {
        vec![
            "--listen",
            "127.0.0.1:8125",
            "--target",
            "172.20.0.1:8122",
            "--server-name",
            "management.internal",
            "--ca",
            "/run/tls/ca.crt",
            "--cert",
            "/run/tls/client.crt",
            "--key",
            "/run/tls/client.key",
        ]
    }

    #[test]
    fn accepts_fixed_loopback_to_unicast_mtls_configuration() {
        let parsed = RelayConfig::parse_from(valid_args()).unwrap();
        assert!(parsed.listen.ip().is_loopback());
        assert_eq!(parsed.server_name, "management.internal");
    }

    #[test]
    fn rejects_non_loopback_listener_and_ambiguous_arguments() {
        let mut exposed = valid_args();
        exposed[1] = "0.0.0.0:8125";
        assert!(RelayConfig::parse_from(exposed).is_err());
        let mut duplicate = valid_args();
        duplicate.extend(["--key", "/tmp/other"]);
        assert!(RelayConfig::parse_from(duplicate).is_err());
    }

    #[tokio::test]
    async fn forwards_opaque_bytes_only_after_private_ca_mtls() {
        let directory = tempfile::tempdir().unwrap();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let mut server_params =
            CertificateParams::new(vec!["management.internal".to_string()]).unwrap();
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server = server_params.signed_by(&server_key, &ca, &ca_key).unwrap();
        let client_key = KeyPair::generate().unwrap();
        let mut client_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client = client_params.signed_by(&client_key, &ca, &ca_key).unwrap();

        let ca_path = directory.path().join("ca.crt");
        let client_path = directory.path().join("client.crt");
        let client_key_path = directory.path().join("client.key");
        write(&ca_path, ca.pem()).unwrap();
        write(&client_path, client.pem()).unwrap();
        write(&client_key_path, client_key.serialize_pem()).unwrap();
        std::fs::set_permissions(&client_key_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut client_roots = RootCertStore::empty();
        client_roots.add(ca.der().clone()).unwrap();
        let verifier = WebPkiClientVerifier::builder(Arc::new(client_roots))
            .build()
            .unwrap();
        let server_tls = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![server.der().clone()],
                PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
            )
            .unwrap();
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let target_task = tokio::spawn(async move {
            let (stream, _) = target.accept().await.unwrap();
            let mut stream = TlsAcceptor::from(Arc::new(server_tls))
                .accept(stream)
                .await
                .unwrap();
            assert!(stream.get_ref().1.peer_certificates().is_some());
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let relay_addr = reservation.local_addr().unwrap();
        drop(reservation);
        let relay_task = tokio::spawn(serve(RelayConfig {
            listen: relay_addr,
            target: target_addr,
            server_name: "management.internal".to_string(),
            ca: ca_path,
            cert: client_path,
            key: client_key_path,
        }));
        let mut inbound = None;
        for _ in 0..100 {
            match TcpStream::connect(relay_addr).await {
                Ok(stream) => {
                    inbound = Some(stream);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        let mut inbound = inbound.expect("relay did not bind");
        inbound.write_all(b"ping").await.unwrap();
        let mut response = [0_u8; 4];
        inbound.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        target_task.await.unwrap();
        relay_task.abort();
    }
}
