use std::io;
use std::sync::Arc;

use arc_swap::ArcSwap;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[derive(Clone)]
struct GeneratedCert {
    certified_key: Arc<CertifiedKey>,
    cert_der: CertificateDer<'static>,
}

#[derive(Debug)]
struct HotReloadResolver {
    current: ArcSwap<CertifiedKey>,
}

impl HotReloadResolver {
    fn new(certified_key: Arc<CertifiedKey>) -> Self {
        Self {
            current: ArcSwap::new(certified_key),
        }
    }

    fn swap(&self, certified_key: Arc<CertifiedKey>) {
        self.current.store(certified_key);
    }
}

impl ResolvesServerCert for HotReloadResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

#[derive(Debug)]
struct AcceptAnyServerCert;

impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::ED25519,
        ]
    }
}

fn generate_cert(common_name: &str) -> Result<GeneratedCert, Box<dyn std::error::Error>> {
    let key_pair = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["localhost".to_string()])?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;

    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().clone().into_owned();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)?;
    let certified_key = Arc::new(CertifiedKey::new(vec![cert_der.clone()], signing_key));

    Ok(GeneratedCert {
        certified_key,
        cert_der,
    })
}

async fn connect(
    addr: std::net::SocketAddr,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Box<dyn std::error::Error>> {
    let client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from("localhost")?;
    Ok(connector
        .connect(server_name, TcpStream::connect(addr).await?)
        .await?)
}

fn peer_leaf(
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<CertificateDer<'static>, Box<dyn std::error::Error>> {
    let (_, session) = stream.get_ref();
    let cert = session
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or("missing server peer certificate")?;
    Ok(cert.clone().into_owned())
}

async fn echo_one(mut stream: tokio_rustls::server::TlsStream<TcpStream>) -> io::Result<()> {
    let mut buf = [0_u8; 64];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        stream.write_all(&buf[..n]).await?;
        stream.flush().await?;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rustls_resolver_rotation_keeps_live_pty_stream_and_updates_new_handshakes(
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let initial = generate_cert("agentic-hot-reload-before")?;
    let rotated = generate_cert("agentic-hot-reload-after")?;
    let resolver = Arc::new(HotReloadResolver::new(initial.certified_key.clone()));

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver.clone());
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server = tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await?;
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    let _ = echo_one(tls).await;
                }
            });
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    });

    let mut live_pty = connect(addr).await?;
    assert_eq!(peer_leaf(&live_pty)?, initial.cert_der);
    live_pty.write_all(b"pty-before-rotation").await?;
    live_pty.flush().await?;
    let mut before = vec![0_u8; "pty-before-rotation".len()];
    live_pty.read_exact(&mut before).await?;
    assert_eq!(before, b"pty-before-rotation");

    resolver.swap(rotated.certified_key.clone());

    live_pty.write_all(b"pty-after-rotation").await?;
    live_pty.flush().await?;
    let mut after = vec![0_u8; "pty-after-rotation".len()];
    live_pty.read_exact(&mut after).await?;
    assert_eq!(after, b"pty-after-rotation");
    assert_eq!(peer_leaf(&live_pty)?, initial.cert_der);

    let rotated_connection = connect(addr).await?;
    assert_eq!(peer_leaf(&rotated_connection)?, rotated.cert_der);

    server.abort();
    Ok(())
}
