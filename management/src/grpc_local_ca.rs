//! Embedded local CA for the gRPC mTLS TCP fallback path.
//!
//! ADR-025 keeps this CA self-contained for local TCP fallback only. It does
//! not run a CA service and does not change the default transport path.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams,
    DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use time::OffsetDateTime;
use x509_parser::extensions::GeneralName;
use zeroize::{Zeroize, Zeroizing};

const ROOT_CERT_FILE: &str = "grpc-local-root-ca.pem";
const ROOT_KEY_FILE: &str = "grpc-local-root-ca-key.pem";
pub const DEFAULT_MACOS_KEYCHAIN_SERVICE: &str = "io.aiwg.agentic-sandbox.grpc-local-ca";

/// Storage boundary for the embedded CA root private key.
///
/// Implementations must never expose key bytes through diagnostics. The
/// filesystem implementation preserves the existing behavior. The macOS
/// implementation uses Security.framework directly, keeping the key out of
/// argv, environment variables, temporary files, and shell output.
pub trait LocalCaRootKeyStore: Send + Sync {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn store(&self, key_pem: &[u8]) -> Result<()>;
    fn filesystem_path(&self) -> Option<PathBuf>;
    fn backend_name(&self) -> &'static str;
}

#[derive(Debug)]
struct FilesystemRootKeyStore {
    path: PathBuf,
}

impl FilesystemRootKeyStore {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl LocalCaRootKeyStore for FilesystemRootKeyStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)
            .with_context(|| format!("reading embedded gRPC CA key {}", self.path.display()))?;
        Ok(Some(Zeroizing::new(bytes)))
    }

    fn store(&self, key_pem: &[u8]) -> Result<()> {
        write_secret(&self.path, key_pem, 0o600)
            .with_context(|| format!("writing embedded gRPC CA key {}", self.path.display()))
    }

    fn filesystem_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn backend_name(&self) -> &'static str {
        "filesystem"
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct MacosKeychainRootKeyStore {
    service: String,
    account: String,
}

#[cfg(target_os = "macos")]
impl MacosKeychainRootKeyStore {
    fn new(service: String, account: String) -> Self {
        Self { service, account }
    }
}

#[cfg(target_os = "macos")]
impl LocalCaRootKeyStore for MacosKeychainRootKeyStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
        use security_framework::passwords::get_generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        match get_generic_password(&self.service, &self.account) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(error) => anyhow::bail!(
                "macOS Keychain local CA root key is unavailable (status {}); \
                 unlock the login Keychain or grant the launchd user service access",
                error.code()
            ),
        }
    }

    fn store(&self, key_pem: &[u8]) -> Result<()> {
        use security_framework::passwords::{set_generic_password_options, PasswordOptions};

        let mut options = PasswordOptions::new_generic_password(&self.service, &self.account);
        options.set_access_synchronized(Some(false));
        options.set_label("Agentic Sandbox local gRPC CA root key");
        options.set_description(
            "Private key for the explicitly selected Agentic Sandbox workstation CA",
        );
        set_generic_password_options(key_pem, options).map_err(|error| {
            anyhow::anyhow!(
                "failed to store macOS Keychain local CA root key (status {}); \
                 the Keychain must be unlocked and available without interactive prompts",
                error.code()
            )
        })
    }

    fn filesystem_path(&self) -> Option<PathBuf> {
        None
    }

    fn backend_name(&self) -> &'static str {
        "macos-keychain"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCaRootKeyStoreConfig {
    Filesystem,
    MacosKeychain { service: String, account: String },
}

impl LocalCaRootKeyStoreConfig {
    pub fn from_values(
        kind: Option<&str>,
        service: Option<&str>,
        account: Option<&str>,
        trust_domain: &str,
    ) -> Result<Self> {
        let kind = kind.unwrap_or("filesystem").trim().to_ascii_lowercase();
        match kind.as_str() {
            "filesystem" => Ok(Self::Filesystem),
            "keychain" | "macos-keychain" => {
                let service = nonempty_or(
                    service,
                    DEFAULT_MACOS_KEYCHAIN_SERVICE,
                    "AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_SERVICE",
                )?;
                let account = nonempty_or(
                    account,
                    &format!("root-key:{trust_domain}"),
                    "AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_ACCOUNT",
                )?;
                Ok(Self::MacosKeychain { service, account })
            }
            other => anyhow::bail!(
                "invalid AGENTIC_GRPC_LOCAL_CA_KEY_STORE `{other}`; expected filesystem or macos-keychain"
            ),
        }
    }

    pub fn from_env(trust_domain: &str) -> Result<Self> {
        let kind = std::env::var("AGENTIC_GRPC_LOCAL_CA_KEY_STORE").ok();
        let service = std::env::var("AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_SERVICE").ok();
        let account = std::env::var("AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_ACCOUNT").ok();
        Self::from_values(
            kind.as_deref(),
            service.as_deref(),
            account.as_deref(),
            trust_domain,
        )
    }

    fn build(&self, dir: &Path) -> Result<Arc<dyn LocalCaRootKeyStore>> {
        match self {
            Self::Filesystem => Ok(Arc::new(FilesystemRootKeyStore::new(
                dir.join(ROOT_KEY_FILE),
            ))),
            Self::MacosKeychain { service, account } => {
                #[cfg(target_os = "macos")]
                {
                    Ok(Arc::new(MacosKeychainRootKeyStore::new(
                        service.clone(),
                        account.clone(),
                    )))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (service, account);
                    anyhow::bail!(
                        "AGENTIC_GRPC_LOCAL_CA_KEY_STORE=macos-keychain is supported only on macOS"
                    )
                }
            }
        }
    }
}

fn nonempty_or(value: Option<&str>, default: &str, name: &str) -> Result<String> {
    let value = value.unwrap_or(default).trim();
    if value.is_empty() {
        anyhow::bail!("{name} must not be empty");
    }
    if value.contains('\n') || value.contains('\r') {
        anyhow::bail!("{name} must not contain newlines");
    }
    Ok(value.to_string())
}

pub struct EmbeddedGrpcCa {
    dir: PathBuf,
    root_cert_pem: String,
    root_cert: Certificate,
    root_key: KeyPair,
    root_key_path: Option<PathBuf>,
    root_key_store_name: &'static str,
    options: LocalCaOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalCaOptions {
    pub agent_leaf_ttl: Duration,
    pub server_leaf_ttl: Duration,
    pub renew_before: Duration,
}

impl Default for LocalCaOptions {
    fn default() -> Self {
        Self {
            agent_leaf_ttl: Duration::from_secs(24 * 60 * 60),
            server_leaf_ttl: Duration::from_secs(7 * 24 * 60 * 60),
            renew_before: Duration::from_secs(6 * 60 * 60),
        }
    }
}

#[derive(Debug)]
pub struct IssuedAgentLeaf {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Debug)]
pub struct IssuedAgentCertificate {
    pub cert_pem: String,
}

#[derive(Debug)]
pub struct IssuedServerLeaf {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedAgentLeaf {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub spiffe_id: String,
}

impl EmbeddedGrpcCa {
    pub fn load_or_create(dir: impl AsRef<Path>, trust_domain: &str) -> Result<Self> {
        Self::load_or_create_with_options(dir, trust_domain, LocalCaOptions::default())
    }

    pub fn load_or_create_with_options(
        dir: impl AsRef<Path>,
        trust_domain: &str,
        options: LocalCaOptions,
    ) -> Result<Self> {
        let dir = dir.as_ref();
        Self::load_or_create_with_key_store(
            dir,
            trust_domain,
            options,
            Arc::new(FilesystemRootKeyStore::new(dir.join(ROOT_KEY_FILE))),
        )
    }

    pub fn load_or_create_from_env(
        dir: impl AsRef<Path>,
        trust_domain: &str,
        options: LocalCaOptions,
    ) -> Result<Self> {
        let dir = dir.as_ref();
        let key_store = LocalCaRootKeyStoreConfig::from_env(trust_domain)?.build(dir)?;
        Self::load_or_create_with_key_store(dir, trust_domain, options, key_store)
    }

    pub fn load_or_create_with_key_store(
        dir: impl AsRef<Path>,
        trust_domain: &str,
        options: LocalCaOptions,
        key_store: Arc<dyn LocalCaRootKeyStore>,
    ) -> Result<Self> {
        validate_local_ca_options(options)?;
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("creating CA dir {}", dir.display()))?;
        set_mode(&dir, 0o700).with_context(|| format!("chmod 0700 {}", dir.display()))?;

        let cert_path = dir.join(ROOT_CERT_FILE);
        let key_material = key_store.load()?;

        if cert_path.exists() || key_material.is_some() {
            return Self::load_existing(dir, cert_path, key_material, key_store, options);
        }

        let root_key = KeyPair::generate().context("generating embedded gRPC CA key")?;
        let root_params = root_params(trust_domain)?;
        let root_cert = root_params
            .self_signed(&root_key)
            .context("self-signing embedded gRPC CA")?;
        let root_cert_pem = root_cert.pem();
        let root_key_pem = Zeroizing::new(root_key.serialize_pem());

        write_secret(&cert_path, root_cert_pem.as_bytes(), 0o600)
            .with_context(|| format!("writing embedded gRPC CA cert {}", cert_path.display()))?;
        key_store.store(root_key_pem.as_bytes())?;

        Ok(Self {
            dir,
            root_cert_pem,
            root_cert,
            root_key,
            root_key_path: key_store.filesystem_path(),
            root_key_store_name: key_store.backend_name(),
            options,
        })
    }

    fn load_existing(
        dir: PathBuf,
        cert_path: PathBuf,
        root_key_pem: Option<Zeroizing<Vec<u8>>>,
        key_store: Arc<dyn LocalCaRootKeyStore>,
        options: LocalCaOptions,
    ) -> Result<Self> {
        if !cert_path.exists() || root_key_pem.is_none() {
            let key_location = key_store
                .filesystem_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "the configured macOS Keychain item".to_string());
            anyhow::bail!(
                "embedded gRPC CA requires both {} and {}",
                cert_path.display(),
                key_location
            );
        }

        let root_cert_pem = fs::read_to_string(&cert_path)
            .with_context(|| format!("reading embedded gRPC CA cert {}", cert_path.display()))?;
        let mut root_key_pem = root_key_pem.expect("checked above");
        let root_key_text =
            std::str::from_utf8(&root_key_pem).context("embedded gRPC CA key is not UTF-8 PEM")?;
        let root_key = KeyPair::from_pem(root_key_text).context("parsing embedded gRPC CA key")?;
        root_key_pem.zeroize();
        verify_root_key_matches_certificate(&root_cert_pem, &root_key)?;
        let root_params = CertificateParams::from_ca_cert_pem(&root_cert_pem)
            .context("parsing embedded gRPC CA cert")?;
        let root_cert = root_params
            .self_signed(&root_key)
            .context("reconstructing embedded gRPC CA issuer")?;

        set_mode(&cert_path, 0o600)
            .with_context(|| format!("chmod 0600 {}", cert_path.display()))?;
        if let Some(key_path) = key_store.filesystem_path() {
            set_mode(&key_path, 0o600)
                .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
        }

        Ok(Self {
            dir,
            root_cert_pem,
            root_cert,
            root_key,
            root_key_path: key_store.filesystem_path(),
            root_key_store_name: key_store.backend_name(),
            options,
        })
    }

    pub fn root_cert_pem(&self) -> &str {
        &self.root_cert_pem
    }

    pub fn root_cert_path(&self) -> PathBuf {
        self.dir.join(ROOT_CERT_FILE)
    }

    pub fn root_key_path(&self) -> Option<&Path> {
        self.root_key_path.as_deref()
    }

    pub fn root_key_store_name(&self) -> &'static str {
        self.root_key_store_name
    }

    pub fn issue_agent_leaf(&self, spiffe_id: &str) -> Result<IssuedAgentLeaf> {
        self.issue_agent_leaf_with_ttl(spiffe_id, self.options.agent_leaf_ttl)
    }

    pub fn issue_agent_leaf_with_ttl(
        &self,
        spiffe_id: &str,
        ttl: Duration,
    ) -> Result<IssuedAgentLeaf> {
        if !spiffe_id.starts_with("spiffe://") {
            anyhow::bail!("agent leaf SPIFFE id must start with spiffe://");
        }

        let leaf_key = KeyPair::generate().context("generating agent mTLS leaf key")?;
        let mut leaf_params = CertificateParams::new(Vec::<String>::new())
            .context("building agent mTLS leaf params")?;
        leaf_params.distinguished_name = DistinguishedName::new();
        leaf_params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into()?));
        leaf_params.is_ca = IsCa::ExplicitNoCa;
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        apply_validity(&mut leaf_params, ttl)?;

        let cert = leaf_params
            .signed_by(&leaf_key, &self.root_cert, &self.root_key)
            .context("signing agent mTLS leaf with embedded gRPC CA")?;

        Ok(IssuedAgentLeaf {
            cert_pem: cert.pem(),
            key_pem: leaf_key.serialize_pem(),
        })
    }

    pub fn issue_server_leaf(&self, dns_name: &str) -> Result<IssuedServerLeaf> {
        self.issue_server_leaf_for_dns_names(&[dns_name.to_string()])
    }

    pub fn issue_server_leaf_with_ttl(
        &self,
        dns_name: &str,
        ttl: Duration,
    ) -> Result<IssuedServerLeaf> {
        self.issue_server_leaf_for_dns_names_with_ttl(&[dns_name.to_string()], ttl)
    }

    pub fn issue_server_leaf_for_dns_names(
        &self,
        dns_names: &[String],
    ) -> Result<IssuedServerLeaf> {
        self.issue_server_leaf_for_dns_names_with_ttl(dns_names, self.options.server_leaf_ttl)
    }

    pub fn issue_server_leaf_for_dns_names_with_ttl(
        &self,
        dns_names: &[String],
        ttl: Duration,
    ) -> Result<IssuedServerLeaf> {
        let dns_names = normalize_server_dns_names(dns_names)?;
        let common_name = dns_names
            .first()
            .cloned()
            .expect("normalized server DNS names are non-empty");

        let leaf_key = KeyPair::generate().context("generating server mTLS leaf key")?;
        let mut leaf_params =
            CertificateParams::new(dns_names).context("building server mTLS leaf params")?;
        leaf_params.distinguished_name = DistinguishedName::new();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        leaf_params.is_ca = IsCa::ExplicitNoCa;
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        apply_validity(&mut leaf_params, ttl)?;

        let cert = leaf_params
            .signed_by(&leaf_key, &self.root_cert, &self.root_key)
            .context("signing server mTLS leaf with embedded gRPC CA")?;

        Ok(IssuedServerLeaf {
            cert_pem: cert.pem(),
            key_pem: leaf_key.serialize_pem(),
        })
    }

    pub fn load_or_issue_server_leaf(
        &self,
        dns_name: &str,
        cert_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<()> {
        self.load_or_issue_server_leaf_for_dns_names(&[dns_name.to_string()], cert_path, key_path)
    }

    pub fn load_or_issue_server_leaf_for_dns_names(
        &self,
        dns_names: &[String],
        cert_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<()> {
        let dns_names = normalize_server_dns_names(dns_names)?;
        let cert_path = cert_path.as_ref().to_path_buf();
        let key_path = key_path.as_ref().to_path_buf();

        match (cert_path.exists(), key_path.exists()) {
            (true, true)
                if !server_leaf_needs_rotation(
                    &cert_path,
                    &dns_names,
                    &self.root_cert_pem,
                    self.options.renew_before,
                )? =>
            {
                set_mode(&cert_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", cert_path.display()))?;
                set_mode(&key_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
            }
            (true, true) => {
                let leaf = self.issue_server_leaf_for_dns_names(&dns_names)?;
                write_secret(&cert_path, leaf.cert_pem.as_bytes(), 0o600).with_context(|| {
                    format!("renewing server mTLS cert {}", cert_path.display())
                })?;
                write_secret(&key_path, leaf.key_pem.as_bytes(), 0o600).with_context(|| {
                    format!("renewing server mTLS private key {}", key_path.display())
                })?;
            }
            (false, false) => {
                ensure_private_parent(&cert_path)?;
                ensure_private_parent(&key_path)?;
                let leaf = self.issue_server_leaf_for_dns_names(&dns_names)?;
                write_secret(&cert_path, leaf.cert_pem.as_bytes(), 0o600)
                    .with_context(|| format!("writing server mTLS cert {}", cert_path.display()))?;
                write_secret(&key_path, leaf.key_pem.as_bytes(), 0o600).with_context(|| {
                    format!("writing server mTLS private key {}", key_path.display())
                })?;
            }
            _ => {
                anyhow::bail!(
                    "server mTLS leaf requires both {} and {}",
                    cert_path.display(),
                    key_path.display()
                );
            }
        }

        Ok(())
    }

    pub fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate> {
        self.issue_agent_certificate_from_csr_with_ttl(
            spiffe_id,
            csr_pem,
            self.options.agent_leaf_ttl,
        )
    }

    pub fn issue_agent_certificate_from_csr_with_ttl(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
        ttl: Duration,
    ) -> Result<IssuedAgentCertificate> {
        validate_spiffe_id(spiffe_id)?;

        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)
            .context("parsing and verifying agent mTLS CSR")?;

        if csr
            .params
            .distinguished_name
            .get(&DnType::CommonName)
            .is_some()
        {
            anyhow::bail!("agent CSR subject common name is not allowed");
        }

        match csr.params.subject_alt_names.as_slice() {
            [SanType::URI(uri)] if uri.as_str() == spiffe_id => {}
            _ => anyhow::bail!("agent CSR must contain exactly one SPIFFE URI-SAN matching token"),
        }

        csr.params.distinguished_name = DistinguishedName::new();
        csr.params.subject_alt_names = vec![SanType::URI(spiffe_id.try_into()?)];
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        apply_validity(&mut csr.params, ttl)?;

        let cert = csr
            .signed_by(&self.root_cert, &self.root_key)
            .context("signing agent mTLS CSR with embedded gRPC CA")?;

        Ok(IssuedAgentCertificate {
            cert_pem: cert.pem(),
        })
    }

    pub fn load_or_issue_agent_leaf(
        &self,
        spiffe_id: &str,
        cert_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<PersistedAgentLeaf> {
        let cert_path = cert_path.as_ref().to_path_buf();
        let key_path = key_path.as_ref().to_path_buf();

        match (cert_path.exists(), key_path.exists()) {
            (true, true)
                if !agent_leaf_needs_renewal(&cert_path, spiffe_id, self.options.renew_before)? =>
            {
                verify_leaf_spiffe_id(&cert_path, spiffe_id)?;
                set_mode(&cert_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", cert_path.display()))?;
                set_mode(&key_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
            }
            (true, true) => {
                verify_leaf_spiffe_id(&cert_path, spiffe_id)?;
                let leaf = self.issue_agent_leaf(spiffe_id)?;
                write_secret(&cert_path, leaf.cert_pem.as_bytes(), 0o600).with_context(|| {
                    format!("renewing agent mTLS leaf cert {}", cert_path.display())
                })?;
                write_secret(&key_path, leaf.key_pem.as_bytes(), 0o600).with_context(|| {
                    format!("renewing agent mTLS leaf key {}", key_path.display())
                })?;
            }
            (false, false) => {
                ensure_private_parent(&cert_path)?;
                ensure_private_parent(&key_path)?;
                let leaf = self.issue_agent_leaf(spiffe_id)?;
                write_secret(&cert_path, leaf.cert_pem.as_bytes(), 0o600).with_context(|| {
                    format!("writing agent mTLS leaf cert {}", cert_path.display())
                })?;
                write_secret(&key_path, leaf.key_pem.as_bytes(), 0o600).with_context(|| {
                    format!("writing agent mTLS leaf key {}", key_path.display())
                })?;
            }
            _ => {
                anyhow::bail!(
                    "agent mTLS leaf requires both {} and {}",
                    cert_path.display(),
                    key_path.display()
                );
            }
        }

        Ok(PersistedAgentLeaf {
            cert_path,
            key_path,
            spiffe_id: spiffe_id.to_string(),
        })
    }
}

fn validate_spiffe_id(spiffe_id: &str) -> Result<()> {
    if !spiffe_id.starts_with("spiffe://") {
        anyhow::bail!("agent leaf SPIFFE id must start with spiffe://");
    }
    Ok(())
}

fn validate_local_ca_options(options: LocalCaOptions) -> Result<()> {
    if options.agent_leaf_ttl.is_zero() {
        anyhow::bail!("agent leaf TTL must be greater than zero");
    }
    if options.server_leaf_ttl.is_zero() {
        anyhow::bail!("server leaf TTL must be greater than zero");
    }
    if options.renew_before >= options.agent_leaf_ttl {
        anyhow::bail!("agent leaf renew_before must be shorter than the agent leaf TTL");
    }
    if options.renew_before >= options.server_leaf_ttl {
        anyhow::bail!("server leaf renew_before must be shorter than the server leaf TTL");
    }
    Ok(())
}

fn apply_validity(params: &mut CertificateParams, ttl: Duration) -> Result<()> {
    if ttl.is_zero() {
        anyhow::bail!("certificate TTL must be greater than zero");
    }
    let now = OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::seconds(60);
    params.not_after = now
        + time::Duration::try_from(ttl).context("certificate TTL is outside supported range")?;
    Ok(())
}

fn root_params(trust_domain: &str) -> Result<CertificateParams> {
    let trust_domain = trust_domain.trim();
    if trust_domain.is_empty() {
        anyhow::bail!("embedded gRPC CA trust domain cannot be empty");
    }

    let mut params =
        CertificateParams::new(Vec::<String>::new()).context("building gRPC CA params")?;
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(
        DnType::CommonName,
        format!("agentic-sandbox gRPC local CA {trust_domain}"),
    );
    params.not_before = OffsetDateTime::now_utc() - time::Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + time::Duration::days(3650);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params)
}

fn verify_root_key_matches_certificate(root_cert_pem: &str, root_key: &KeyPair) -> Result<()> {
    let cert_der = first_cert_der_from_pem(root_cert_pem.as_bytes(), "embedded gRPC CA root")?;
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref())
        .context("parsing embedded gRPC CA root certificate")?;
    if cert.public_key().raw != root_key.public_key_der() {
        anyhow::bail!(
            "embedded gRPC CA root certificate does not match the configured private key"
        );
    }
    Ok(())
}

fn write_secret(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    fs::write(path, bytes)?;
    set_mode(path, mode)?;
    Ok(())
}

fn ensure_private_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    set_mode(parent, 0o700).with_context(|| format!("chmod 0700 {}", parent.display()))?;
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn verify_leaf_spiffe_id(cert_path: &Path, expected_spiffe_id: &str) -> Result<()> {
    with_parsed_first_cert(cert_path, |cert| {
        verify_parsed_leaf_spiffe_id(cert, expected_spiffe_id)
    })
}

fn normalize_server_dns_names(dns_names: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(dns_names.len());
    for dns_name in dns_names {
        let dns_name = dns_name.trim();
        if dns_name.is_empty() {
            anyhow::bail!("server leaf DNS name cannot be empty");
        }
        if !normalized.iter().any(|existing| existing == dns_name) {
            normalized.push(dns_name.to_string());
        }
    }
    if normalized.is_empty() {
        anyhow::bail!("server leaf requires at least one DNS name");
    }
    Ok(normalized)
}

fn agent_leaf_needs_renewal(
    cert_path: &Path,
    expected_spiffe_id: &str,
    renew_before: Duration,
) -> Result<bool> {
    with_parsed_first_cert(cert_path, |cert| {
        verify_parsed_leaf_spiffe_id(cert, expected_spiffe_id)?;
        cert_needs_renewal(cert, renew_before)
    })
}

fn server_leaf_needs_rotation(
    cert_path: &Path,
    expected_dns_names: &[String],
    root_cert_pem: &str,
    renew_before: Duration,
) -> Result<bool> {
    let cert_pem = fs::read(cert_path)
        .with_context(|| format!("reading server mTLS leaf cert {}", cert_path.display()))?;
    let cert_der = match first_cert_der_from_pem(&cert_pem, "server mTLS leaf") {
        Ok(cert_der) => cert_der,
        Err(_) => return Ok(true),
    };
    let (_, cert) = match x509_parser::parse_x509_certificate(cert_der.as_ref()) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(true),
    };
    let root_der = match first_cert_der_from_pem(root_cert_pem.as_bytes(), "embedded gRPC CA") {
        Ok(root_der) => root_der,
        Err(err) => return Err(err),
    };
    let (_, root) = x509_parser::parse_x509_certificate(root_der.as_ref())
        .context("parsing embedded gRPC CA certificate")?;

    if cert.verify_signature(Some(root.public_key())).is_err() {
        return Ok(true);
    }
    let mut actual_dns_names = parsed_leaf_dns_names(&cert)?;
    let mut expected_dns_names = expected_dns_names.to_vec();
    actual_dns_names.sort_unstable();
    actual_dns_names.dedup();
    expected_dns_names.sort_unstable();
    expected_dns_names.dedup();
    if actual_dns_names != expected_dns_names {
        return Ok(true);
    }
    cert_needs_renewal(&cert, renew_before)
}

fn cert_needs_renewal(
    cert: &x509_parser::certificate::X509Certificate<'_>,
    renew_before: Duration,
) -> Result<bool> {
    let now = x509_parser::time::ASN1Time::now().timestamp();
    let not_before = cert.validity().not_before.timestamp();
    let not_after = cert.validity().not_after.timestamp();
    if now < not_before {
        anyhow::bail!("certificate is not valid yet");
    }
    let renew_before = i64::try_from(renew_before.as_secs()).unwrap_or(i64::MAX);
    Ok(not_after <= now.saturating_add(renew_before))
}

fn with_parsed_first_cert<T>(
    cert_path: &Path,
    f: impl FnOnce(&x509_parser::certificate::X509Certificate<'_>) -> Result<T>,
) -> Result<T> {
    let cert_pem = fs::read(cert_path)
        .with_context(|| format!("reading agent mTLS leaf cert {}", cert_path.display()))?;
    let cert_der = first_cert_der_from_pem(&cert_pem, "agent mTLS leaf")?;
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref())
        .context("parsing agent mTLS leaf certificate")?;
    f(&cert)
}

fn first_cert_der_from_pem(
    pem: &[u8],
    label: &str,
) -> Result<rustls::pki_types::CertificateDer<'static>> {
    let mut reader = std::io::BufReader::new(pem);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parsing {label} PEM"))?;
    certs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("{label} cert contains no certificates"))
}

fn verify_parsed_leaf_spiffe_id(
    cert: &x509_parser::certificate::X509Certificate<'_>,
    expected_spiffe_id: &str,
) -> Result<()> {
    let san = cert
        .subject_alternative_name()
        .context("parsing agent mTLS leaf SAN")?
        .ok_or_else(|| anyhow::anyhow!("agent mTLS leaf certificate has no SAN"))?;
    let uris: Vec<_> = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::URI(uri) => Some(*uri),
            _ => None,
        })
        .collect();
    if uris != [expected_spiffe_id] {
        anyhow::bail!(
            "agent mTLS leaf certificate SPIFFE URI-SAN mismatch: expected {expected_spiffe_id}"
        );
    }
    Ok(())
}

#[cfg(test)]
fn parsed_leaf_has_dns_name(
    cert: &x509_parser::certificate::X509Certificate<'_>,
    expected_dns_name: &str,
) -> Result<bool> {
    Ok(parsed_leaf_dns_names(cert)?
        .iter()
        .any(|name| name == expected_dns_name))
}

fn parsed_leaf_dns_names(
    cert: &x509_parser::certificate::X509Certificate<'_>,
) -> Result<Vec<String>> {
    let san = cert
        .subject_alternative_name()
        .context("parsing server mTLS leaf SAN")?
        .ok_or_else(|| anyhow::anyhow!("server mTLS leaf certificate has no SAN"))?;
    Ok(san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::DNSName(name) => Some((*name).to_string()),
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryRootKeyStore {
        key: Mutex<Option<Vec<u8>>>,
    }

    impl Drop for MemoryRootKeyStore {
        fn drop(&mut self) {
            if let Ok(key) = self.key.get_mut() {
                if let Some(key) = key.as_mut() {
                    key.zeroize();
                }
            }
        }
    }

    impl LocalCaRootKeyStore for MemoryRootKeyStore {
        fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
            Ok(self
                .key
                .lock()
                .unwrap()
                .as_ref()
                .map(|key| Zeroizing::new(key.clone())))
        }

        fn store(&self, key_pem: &[u8]) -> Result<()> {
            *self.key.lock().unwrap() = Some(key_pem.to_vec());
            Ok(())
        }

        fn filesystem_path(&self) -> Option<PathBuf> {
            None
        }

        fn backend_name(&self) -> &'static str {
            "synthetic-keychain"
        }
    }

    struct UnavailableRootKeyStore;

    impl LocalCaRootKeyStore for UnavailableRootKeyStore {
        fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
            anyhow::bail!("synthetic keychain is locked")
        }

        fn store(&self, _key_pem: &[u8]) -> Result<()> {
            anyhow::bail!("synthetic keychain is locked")
        }

        fn filesystem_path(&self) -> Option<PathBuf> {
            None
        }

        fn backend_name(&self) -> &'static str {
            "synthetic-keychain"
        }
    }

    #[test]
    fn embedded_ca_persists_root_with_private_modes() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();

        assert!(ca.root_cert_pem().contains("BEGIN CERTIFICATE"));
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(ca.root_cert_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(ca.root_key_path().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let reloaded =
            EmbeddedGrpcCa::load_or_create(dir.path(), "ignored-after-first-create").unwrap();
        assert_eq!(ca.root_cert_pem(), reloaded.root_cert_pem());
    }

    #[test]
    fn explicit_key_store_selection_preserves_filesystem_default() {
        assert_eq!(
            LocalCaRootKeyStoreConfig::from_values(None, None, None, "sandbox.agentic.local")
                .unwrap(),
            LocalCaRootKeyStoreConfig::Filesystem
        );
        assert_eq!(
            LocalCaRootKeyStoreConfig::from_values(
                Some("macos-keychain"),
                Some("io.aiwg.synthetic"),
                Some("root-key:test"),
                "sandbox.agentic.local"
            )
            .unwrap(),
            LocalCaRootKeyStoreConfig::MacosKeychain {
                service: "io.aiwg.synthetic".to_string(),
                account: "root-key:test".to_string(),
            }
        );
    }

    #[test]
    fn key_store_backend_keeps_root_private_key_off_filesystem_and_preserves_spiffe_issuance() {
        let dir = tempfile::tempdir().unwrap();
        let key_store = Arc::new(MemoryRootKeyStore::default());
        let ca = EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "sandbox-test.agentic.local",
            LocalCaOptions::default(),
            key_store.clone(),
        )
        .unwrap();
        let root_cert = ca.root_cert_pem().to_string();
        assert_eq!(ca.root_key_store_name(), "synthetic-keychain");
        assert!(ca.root_key_path().is_none());
        assert!(!dir.path().join(ROOT_KEY_FILE).exists());

        let reloaded = EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "ignored-after-first-create",
            LocalCaOptions::default(),
            key_store,
        )
        .unwrap();
        assert_eq!(root_cert, reloaded.root_cert_pem());

        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();
        let issued = reloaded
            .issue_agent_certificate_from_csr(spiffe_id, &csr)
            .unwrap();
        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(!issued.cert_pem.contains("PRIVATE KEY"));
    }

    #[test]
    fn key_store_selection_never_silently_migrates_filesystem_root_key() {
        let dir = tempfile::tempdir().unwrap();
        EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        assert!(dir.path().join(ROOT_KEY_FILE).exists());

        let error = match EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "sandbox-test.agentic.local",
            LocalCaOptions::default(),
            Arc::new(MemoryRootKeyStore::default()),
        ) {
            Ok(_) => panic!("explicit key-store switch must not migrate filesystem key material"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("requires both"));
        assert!(dir.path().join(ROOT_KEY_FILE).exists());
    }

    #[test]
    fn unavailable_key_store_fails_closed_without_filesystem_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let error = match EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "sandbox-test.agentic.local",
            LocalCaOptions::default(),
            Arc::new(UnavailableRootKeyStore),
        ) {
            Ok(_) => panic!("locked key store must fail closed"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("synthetic keychain is locked"));
        assert!(!dir.path().join(ROOT_KEY_FILE).exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn synthetic_macos_keychain_round_trip_and_spiffe_continuity() {
        if std::env::var("AGENTIC_RUN_MACOS_KEYCHAIN_TEST").as_deref() != Ok("1") {
            eprintln!(
                "skipping synthetic Keychain test; set AGENTIC_RUN_MACOS_KEYCHAIN_TEST=1 on an isolated macOS user"
            );
            return;
        }

        use security_framework::passwords::delete_generic_password;

        struct SyntheticKeychainCleanup {
            service: String,
            account: String,
            active: bool,
        }
        impl Drop for SyntheticKeychainCleanup {
            fn drop(&mut self) {
                if self.active {
                    let _ = delete_generic_password(&self.service, &self.account);
                }
            }
        }

        let id = uuid::Uuid::now_v7();
        let service = format!("io.aiwg.agentic-sandbox.test.{id}");
        let account = format!("synthetic-root-key:{id}");
        let mut cleanup = SyntheticKeychainCleanup {
            service: service.clone(),
            account: account.clone(),
            active: true,
        };
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MacosKeychainRootKeyStore::new(service, account));

        let ca = EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "synthetic.agentic.invalid",
            LocalCaOptions::default(),
            store.clone(),
        )
        .unwrap();
        assert_eq!(ca.root_key_store_name(), "macos-keychain");
        assert!(ca.root_key_path().is_none());
        assert!(!dir.path().join(ROOT_KEY_FILE).exists());

        let reloaded = EmbeddedGrpcCa::load_or_create_with_key_store(
            dir.path(),
            "synthetic.agentic.invalid",
            LocalCaOptions::default(),
            store,
        )
        .unwrap();
        assert_eq!(ca.root_cert_pem(), reloaded.root_cert_pem());
        let spiffe_id = format!("spiffe://synthetic.agentic.invalid/agent/{id}");
        let leaf = reloaded.issue_agent_leaf(&spiffe_id).unwrap();
        assert!(leaf.cert_pem.contains("BEGIN CERTIFICATE"));
        delete_generic_password(&cleanup.service, &cleanup.account)
            .expect("delete the synthetic Keychain item created by this test");
        cleanup.active = false;
    }

    #[test]
    fn agent_leaf_has_single_spiffe_uri_san_and_no_subject_cn() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let leaf = ca.issue_agent_leaf(spiffe_id).unwrap();

        assert!(leaf.key_pem.contains("BEGIN PRIVATE KEY"));
        let mut reader = std::io::BufReader::new(leaf.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let cert_der = certs.first().unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        assert_eq!(cert.subject().iter_common_name().count(), 0);

        let san = cert.subject_alternative_name().unwrap().unwrap();
        let uris: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::URI(uri) => Some(*uri),
                _ => None,
            })
            .collect();
        assert_eq!(uris, vec![spiffe_id]);
    }

    #[test]
    fn server_leaf_has_dns_san_for_agent_server_name_validation() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();

        let leaf = ca.issue_server_leaf("host.internal").unwrap();

        assert!(leaf.key_pem.contains("BEGIN PRIVATE KEY"));
        let mut reader = std::io::BufReader::new(leaf.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let cert_der = certs.first().unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        let san = cert.subject_alternative_name().unwrap().unwrap();
        let dns_names: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(name) => Some(*name),
                _ => None,
            })
            .collect();
        assert_eq!(dns_names, vec!["host.internal"]);
    }

    #[test]
    fn server_leaf_supports_multiple_dns_sans() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let names = vec!["host.docker.internal".to_string(), "localhost".to_string()];

        let leaf = ca.issue_server_leaf_for_dns_names(&names).unwrap();

        let mut reader = std::io::BufReader::new(leaf.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(certs[0].as_ref()).unwrap();
        assert_eq!(parsed_leaf_dns_names(&cert).unwrap(), names);
    }

    #[test]
    fn agent_leaf_rejects_non_spiffe_identity() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();

        let err = ca.issue_agent_leaf("https://not-spiffe").unwrap_err();

        assert!(err.to_string().contains("must start with spiffe://"));
    }

    #[test]
    fn csr_signing_returns_leaf_for_requested_spiffe_without_private_key() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let issued = ca
            .issue_agent_certificate_from_csr(spiffe_id, &csr)
            .unwrap();

        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(!issued.cert_pem.contains("PRIVATE KEY"));
        let mut reader = std::io::BufReader::new(issued.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let cert_der = certs.first().unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        assert_eq!(cert.subject().iter_common_name().count(), 0);

        let san = cert.subject_alternative_name().unwrap().unwrap();
        let uris: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::URI(uri) => Some(*uri),
                _ => None,
            })
            .collect();
        assert_eq!(uris, vec![spiffe_id]);
    }

    #[test]
    fn csr_signing_rejects_spiffe_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names.push(SanType::URI(
            "spiffe://sandbox-test.agentic.local/agent/a"
                .try_into()
                .unwrap(),
        ));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let err = ca
            .issue_agent_certificate_from_csr("spiffe://sandbox-test.agentic.local/agent/b", &csr)
            .unwrap_err();

        assert!(err.to_string().contains("matching token"));
    }

    #[test]
    fn partial_ca_material_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(ROOT_CERT_FILE), "not a cert").unwrap();

        let err = match EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local") {
            Err(err) => err,
            Ok(_) => panic!("partial embedded CA material should fail closed"),
        };

        assert!(err.to_string().contains("requires both"));
    }

    #[test]
    fn load_or_issue_agent_leaf_persists_and_reuses_private_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let leaf_dir = dir.path().join("leaf");
        let cert_path = leaf_dir.join("agent.pem");
        let key_path = leaf_dir.join("agent-key.pem");
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let persisted = ca
            .load_or_issue_agent_leaf(spiffe_id, &cert_path, &key_path)
            .unwrap();

        assert_eq!(persisted.cert_path, cert_path);
        assert_eq!(persisted.key_path, key_path);
        assert_eq!(
            fs::metadata(&leaf_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&persisted.cert_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&persisted.key_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let cert_before = fs::read_to_string(&persisted.cert_path).unwrap();
        let reloaded = ca
            .load_or_issue_agent_leaf(spiffe_id, &persisted.cert_path, &persisted.key_path)
            .unwrap();
        assert_eq!(reloaded.spiffe_id, spiffe_id);
        assert_eq!(
            cert_before,
            fs::read_to_string(&persisted.cert_path).unwrap()
        );
    }

    #[test]
    fn load_or_issue_agent_leaf_renews_when_inside_renewal_window() {
        let dir = tempfile::tempdir().unwrap();
        let options = LocalCaOptions {
            agent_leaf_ttl: Duration::from_secs(3),
            server_leaf_ttl: Duration::from_secs(30),
            renew_before: Duration::from_secs(2),
        };
        let ca = EmbeddedGrpcCa::load_or_create_with_options(
            dir.path().join("ca"),
            "sandbox-test.agentic.local",
            options,
        )
        .unwrap();
        let cert_path = dir.path().join("leaf/agent.pem");
        let key_path = dir.path().join("leaf/agent-key.pem");
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        ca.load_or_issue_agent_leaf(spiffe_id, &cert_path, &key_path)
            .unwrap();
        let first_cert = fs::read_to_string(&cert_path).unwrap();
        let first_not_after = cert_not_after_unix(&cert_path);
        std::thread::sleep(Duration::from_secs(2));
        ca.load_or_issue_agent_leaf(spiffe_id, &cert_path, &key_path)
            .unwrap();

        let renewed_cert = fs::read_to_string(&cert_path).unwrap();
        assert_ne!(first_cert, renewed_cert);
        assert!(cert_not_after_unix(&cert_path) > first_not_after);
    }

    #[test]
    fn load_or_issue_server_leaf_rotates_when_existing_leaf_signed_by_old_ca() {
        let dir = tempfile::tempdir().unwrap();
        let old_ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("old-ca"), "sandbox-test.agentic.local")
                .unwrap();
        let new_ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("new-ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("server/server.pem");
        let key_path = dir.path().join("server/server-key.pem");

        old_ca
            .load_or_issue_server_leaf("host.docker.internal", &cert_path, &key_path)
            .unwrap();
        let old_cert = fs::read_to_string(&cert_path).unwrap();

        new_ca
            .load_or_issue_server_leaf("host.docker.internal", &cert_path, &key_path)
            .unwrap();

        let rotated_cert = fs::read_to_string(&cert_path).unwrap();
        assert_ne!(old_cert, rotated_cert);
        assert_server_leaf_signed_by(&cert_path, new_ca.root_cert_pem(), "host.docker.internal");
    }

    #[test]
    fn load_or_issue_server_leaf_rotates_when_existing_leaf_has_wrong_dns_name() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("server/server.pem");
        let key_path = dir.path().join("server/server-key.pem");

        ca.load_or_issue_server_leaf("localhost", &cert_path, &key_path)
            .unwrap();
        let localhost_cert = fs::read_to_string(&cert_path).unwrap();

        ca.load_or_issue_server_leaf("host.docker.internal", &cert_path, &key_path)
            .unwrap();

        let docker_host_cert = fs::read_to_string(&cert_path).unwrap();
        assert_ne!(localhost_cert, docker_host_cert);
        assert_server_leaf_signed_by(&cert_path, ca.root_cert_pem(), "host.docker.internal");
    }

    #[test]
    fn load_or_issue_server_leaf_rotates_when_dns_name_set_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("server/server.pem");
        let key_path = dir.path().join("server/server-key.pem");

        ca.load_or_issue_server_leaf("host.docker.internal", &cert_path, &key_path)
            .unwrap();
        let single_name_cert = fs::read_to_string(&cert_path).unwrap();
        let names = vec!["host.docker.internal".to_string(), "localhost".to_string()];

        ca.load_or_issue_server_leaf_for_dns_names(&names, &cert_path, &key_path)
            .unwrap();

        let multi_name_cert = fs::read_to_string(&cert_path).unwrap();
        assert_ne!(single_name_cert, multi_name_cert);
        assert_server_leaf_signed_by(&cert_path, ca.root_cert_pem(), "host.docker.internal");
        assert_server_leaf_signed_by(&cert_path, ca.root_cert_pem(), "localhost");
    }

    #[test]
    fn csr_signing_uses_short_lived_agent_leaf_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let options = LocalCaOptions {
            agent_leaf_ttl: Duration::from_secs(60),
            server_leaf_ttl: Duration::from_secs(120),
            renew_before: Duration::from_secs(30),
        };
        let ca = EmbeddedGrpcCa::load_or_create_with_options(
            dir.path(),
            "sandbox-test.agentic.local",
            options,
        )
        .unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let issued = ca
            .issue_agent_certificate_from_csr(spiffe_id, &csr)
            .unwrap();

        let cert_path = dir.path().join("issued.pem");
        fs::write(&cert_path, issued.cert_pem).unwrap();
        let remaining =
            cert_not_after_unix(&cert_path) - x509_parser::time::ASN1Time::now().timestamp();
        assert!((1..=60).contains(&remaining), "remaining={remaining}");
    }

    #[test]
    fn partial_agent_leaf_material_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("agent.pem");
        let key_path = dir.path().join("agent-key.pem");
        fs::write(&cert_path, "not a cert").unwrap();

        let err = ca
            .load_or_issue_agent_leaf(
                "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1",
                &cert_path,
                &key_path,
            )
            .unwrap_err();

        assert!(err.to_string().contains("requires both"));
    }

    #[test]
    fn existing_agent_leaf_identity_mismatch_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("agent.pem");
        let key_path = dir.path().join("agent-key.pem");
        ca.load_or_issue_agent_leaf(
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1",
            &cert_path,
            &key_path,
        )
        .unwrap();

        let err = ca
            .load_or_issue_agent_leaf(
                "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d2",
                &cert_path,
                &key_path,
            )
            .unwrap_err();

        assert!(err.to_string().contains("SPIFFE URI-SAN mismatch"));
    }

    fn cert_not_after_unix(path: &Path) -> i64 {
        with_parsed_first_cert(path, |cert| Ok(cert.validity().not_after.timestamp())).unwrap()
    }

    fn assert_server_leaf_signed_by(
        cert_path: &Path,
        root_cert_pem: &str,
        expected_dns_name: &str,
    ) {
        let cert_pem = fs::read(cert_path).unwrap();
        let cert_der = first_cert_der_from_pem(&cert_pem, "server mTLS leaf").unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        let root_der =
            first_cert_der_from_pem(root_cert_pem.as_bytes(), "embedded gRPC CA").unwrap();
        let (_, root) = x509_parser::parse_x509_certificate(root_der.as_ref()).unwrap();

        cert.verify_signature(Some(root.public_key())).unwrap();
        assert!(parsed_leaf_has_dns_name(&cert, expected_dns_name).unwrap());
    }
}
