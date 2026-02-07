//! Secret resolution
//!
//! Resolves secrets from various sources (env, vault, file) at orchestration time.

use std::collections::HashMap;
use std::env;
use tokio::fs;
use tracing::{debug, info};
use reqwest::Client;
use serde::Deserialize;

/// Configuration for HashiCorp Vault
#[derive(Debug, Clone)]
pub struct VaultConfig {
    /// Vault server address (e.g., "https://vault.example.com:8200")
    pub addr: String,
    /// KV mount path (default: "secret")
    pub mount: String,
}

impl VaultConfig {
    /// Create VaultConfig from environment variables
    ///
    /// Requires:
    /// - VAULT_ADDR: Vault server address
    /// - VAULT_MOUNT: (optional) KV mount path, defaults to "secret"
    pub fn from_env() -> Option<Self> {
        let addr = env::var("VAULT_ADDR").ok()?;
        let mount = env::var("VAULT_MOUNT").unwrap_or_else(|_| "secret".to_string());
        Some(Self { addr, mount })
    }
}

/// HashiCorp Vault client for KV v2 secrets
#[derive(Debug, Clone)]
pub struct VaultClient {
    config: VaultConfig,
    http: Client,
    token: String,
}

impl VaultClient {
    /// Create a new Vault client with explicit configuration
    pub fn new(config: VaultConfig, token: String) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            config,
            http,
            token,
        }
    }

    /// Create Vault client from environment variables
    ///
    /// Requires:
    /// - VAULT_ADDR: Vault server address
    /// - VAULT_TOKEN: Vault authentication token
    /// - VAULT_MOUNT: (optional) KV mount path
    pub fn from_env() -> Option<Self> {
        let config = VaultConfig::from_env()?;
        let token = env::var("VAULT_TOKEN").ok()?;
        Some(Self::new(config, token))
    }

    /// Read a secret from Vault KV v2
    ///
    /// # Arguments
    /// * `path` - Secret path (e.g., "myapp/config")
    ///
    /// # Returns
    /// The value of the "value" field, or the first string field found
    pub async fn read_secret(&self, path: &str) -> Result<String, VaultError> {
        self.read_field(path, "value").await
    }

    /// Read a specific field from a Vault secret
    ///
    /// # Arguments
    /// * `path` - Secret path (e.g., "myapp/config")
    /// * `field` - Field name to extract (e.g., "password")
    pub async fn read_field(&self, path: &str, field: &str) -> Result<String, VaultError> {
        // Construct KV v2 URL: {addr}/v1/{mount}/data/{path}
        let url = format!(
            "{}/v1/{}/data/{}",
            self.config.addr.trim_end_matches('/'),
            self.config.mount,
            path.trim_start_matches('/')
        );

        debug!("Fetching Vault secret from {}", url);

        // Make HTTP request
        let response = self
            .http
            .get(&url)
            .header("X-Vault-Token", &self.token)
            .send()
            .await
            .map_err(|e| VaultError::RequestFailed(e.to_string()))?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(VaultError::ApiError(status.as_u16(), error_text));
        }

        // Parse response
        let vault_response: VaultResponse = response
            .json()
            .await
            .map_err(|e| VaultError::ParseError(e.to_string()))?;

        // Extract field from data.data
        vault_response
            .data
            .and_then(|d| d.data)
            .and_then(|fields| fields.get(field).cloned())
            .ok_or_else(|| VaultError::FieldNotFound(path.to_string(), field.to_string()))
    }
}

/// Vault KV v2 response structure
#[derive(Debug, Deserialize)]
struct VaultResponse {
    data: Option<VaultData>,
}

#[derive(Debug, Deserialize)]
struct VaultData {
    data: Option<HashMap<String, String>>,
}

/// Vault errors
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("Vault request failed: {0}")]
    RequestFailed(String),

    #[error("Vault API error ({0}): {1}")]
    ApiError(u16, String),

    #[error("Failed to parse Vault response: {0}")]
    ParseError(String),

    #[error("Field '{1}' not found in secret '{0}'")]
    FieldNotFound(String, String),
}

/// Resolves secrets from various sources
pub struct SecretResolver {
    /// Optional Vault client
    vault_client: Option<VaultClient>,
    /// Cache of resolved secrets
    cache: tokio::sync::RwLock<HashMap<String, String>>,
}

impl SecretResolver {
    pub fn new() -> Self {
        let vault_client = VaultClient::from_env();
        if vault_client.is_some() {
            info!("Vault client initialized successfully");
        } else {
            debug!("Vault client not configured (VAULT_ADDR/VAULT_TOKEN not set)");
        }

        Self {
            vault_client,
            cache: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Resolve a secret from the specified source
    ///
    /// Sources:
    /// - `env`: Read from environment variable
    /// - `file`: Read from file path
    /// - `vault`: Read from HashiCorp Vault
    ///
    /// # Vault path format
    /// Vault keys support "path:field" format. Examples:
    /// - "myapp/db" → reads "value" field from "myapp/db"
    /// - "myapp/db:password" → reads "password" field from "myapp/db"
    pub async fn resolve(&self, source: &str, key: &str) -> Result<String, SecretError> {
        // Check cache first
        let cache_key = format!("{}:{}", source, key);
        {
            let cache = self.cache.read().await;
            if let Some(value) = cache.get(&cache_key) {
                return Ok(value.clone());
            }
        }

        // Resolve based on source
        let value = match source {
            "env" => self.resolve_from_env(key)?,
            "file" => self.resolve_from_file(key).await?,
            "vault" => self.resolve_from_vault(key).await?,
            _ => return Err(SecretError::UnknownSource(source.to_string())),
        };

        // Cache the result
        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, value.clone());
        }

        Ok(value)
    }

    /// Resolve secret from environment variable
    fn resolve_from_env(&self, key: &str) -> Result<String, SecretError> {
        env::var(key).map_err(|_| SecretError::NotFound(format!("env:{}", key)))
    }

    /// Resolve secret from file
    async fn resolve_from_file(&self, path: &str) -> Result<String, SecretError> {
        fs::read_to_string(path)
            .await
            .map(|s| s.trim().to_string())
            .map_err(|e| SecretError::NotFound(format!("file:{} ({})", path, e)))
    }

    /// Resolve secret from HashiCorp Vault
    async fn resolve_from_vault(&self, key: &str) -> Result<String, SecretError> {
        let client = self
            .vault_client
            .as_ref()
            .ok_or(SecretError::VaultNotConfigured)?;

        // Parse path:field format if present
        let (path, field) = if let Some((p, f)) = key.split_once(':') {
            (p, f)
        } else {
            (key, "value")
        };

        client
            .read_field(path, field)
            .await
            .map_err(|e| SecretError::VaultError(e.to_string()))
    }

    /// Clear the cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        debug!("Secret cache cleared");
    }

    /// Remove a specific secret from cache
    pub async fn invalidate(&self, source: &str, key: &str) {
        let cache_key = format!("{}:{}", source, key);
        let mut cache = self.cache.write().await;
        cache.remove(&cache_key);
        debug!("Invalidated secret: {}", cache_key);
    }

    /// Resolve multiple secrets at once
    pub async fn resolve_all(
        &self,
        secrets: &[(String, String, String)], // (name, source, key)
    ) -> Result<HashMap<String, String>, SecretError> {
        let mut result = HashMap::new();

        for (name, source, key) in secrets {
            let value = self.resolve(source, key).await?;
            result.insert(name.clone(), value);
        }

        Ok(result)
    }
}

impl Default for SecretResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Secret resolution errors
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("Unknown secret source: {0}")]
    UnknownSource(String),

    #[error("Secret not found: {0}")]
    NotFound(String),

    #[error("Vault not configured (VAULT_ADDR and VAULT_TOKEN required)")]
    VaultNotConfigured,

    #[error("Vault error: {0}")]
    VaultError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Environment Variable Tests =====

    #[tokio::test]
    async fn test_resolve_from_env() {
        env::set_var("TEST_SECRET_123", "test_value");

        let resolver = SecretResolver::new();
        let result = resolver.resolve("env", "TEST_SECRET_123").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test_value");

        env::remove_var("TEST_SECRET_123");
    }

    #[tokio::test]
    async fn test_resolve_missing_env() {
        let resolver = SecretResolver::new();
        let result = resolver.resolve("env", "NONEXISTENT_SECRET_XYZ").await;

        assert!(result.is_err());
        match result {
            Err(SecretError::NotFound(msg)) => {
                assert!(msg.contains("env:NONEXISTENT_SECRET_XYZ"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_caching() {
        env::set_var("CACHED_SECRET", "cached_value");

        let resolver = SecretResolver::new();

        // First resolution
        let _ = resolver.resolve("env", "CACHED_SECRET").await;

        // Change the env var
        env::set_var("CACHED_SECRET", "new_value");

        // Should return cached value
        let result = resolver.resolve("env", "CACHED_SECRET").await.unwrap();
        assert_eq!(result, "cached_value");

        // Clear cache and resolve again
        resolver.clear_cache().await;
        let result = resolver.resolve("env", "CACHED_SECRET").await.unwrap();
        assert_eq!(result, "new_value");

        env::remove_var("CACHED_SECRET");
    }

    #[tokio::test]
    async fn test_invalidate_specific_secret() {
        env::set_var("SECRET_A", "value_a");
        env::set_var("SECRET_B", "value_b");

        let resolver = SecretResolver::new();

        // Resolve both
        let _ = resolver.resolve("env", "SECRET_A").await;
        let _ = resolver.resolve("env", "SECRET_B").await;

        // Change both
        env::set_var("SECRET_A", "new_a");
        env::set_var("SECRET_B", "new_b");

        // Invalidate only A
        resolver.invalidate("env", "SECRET_A").await;

        // A should be refreshed, B should be cached
        let result_a = resolver.resolve("env", "SECRET_A").await.unwrap();
        let result_b = resolver.resolve("env", "SECRET_B").await.unwrap();

        assert_eq!(result_a, "new_a");
        assert_eq!(result_b, "value_b");

        env::remove_var("SECRET_A");
        env::remove_var("SECRET_B");
    }

    // ===== Vault Configuration Tests =====

    #[test]
    fn test_vault_config_from_env() {
        env::set_var("VAULT_ADDR", "https://vault.example.com:8200");
        env::set_var("VAULT_MOUNT", "kv");

        let config = VaultConfig::from_env();
        assert!(config.is_some());

        let config = config.unwrap();
        assert_eq!(config.addr, "https://vault.example.com:8200");
        assert_eq!(config.mount, "kv");

        env::remove_var("VAULT_ADDR");
        env::remove_var("VAULT_MOUNT");
    }

    #[test]
    fn test_vault_config_default_mount() {
        env::set_var("VAULT_ADDR", "https://vault.example.com:8200");
        env::remove_var("VAULT_MOUNT");

        let config = VaultConfig::from_env();
        assert!(config.is_some());

        let config = config.unwrap();
        assert_eq!(config.mount, "secret");

        env::remove_var("VAULT_ADDR");
    }

    #[test]
    fn test_vault_config_missing_addr() {
        env::remove_var("VAULT_ADDR");
        env::remove_var("VAULT_MOUNT");

        let config = VaultConfig::from_env();
        assert!(config.is_none());
    }

    // ===== Vault Client Tests (with mock server) =====

    #[cfg(feature = "integration-tests")]
    #[tokio::test]
    async fn test_vault_client_read_secret() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path, header};

        let mock_server = MockServer::start().await;

        // Mock Vault KV v2 response
        let response_body = serde_json::json!({
            "data": {
                "data": {
                    "value": "secret_password"
                }
            }
        });

        Mock::given(method("GET"))
            .and(path("/v1/secret/data/myapp/db"))
            .and(header("X-Vault-Token", "test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let config = VaultConfig {
            addr: mock_server.uri(),
            mount: "secret".to_string(),
        };
        let client = VaultClient::new(config, "test-token".to_string());

        let result = client.read_secret("myapp/db").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "secret_password");
    }

    #[cfg(feature = "integration-tests")]
    #[tokio::test]
    async fn test_vault_client_read_field() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path, header};

        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "data": {
                "data": {
                    "username": "admin",
                    "password": "secret123"
                }
            }
        });

        Mock::given(method("GET"))
            .and(path("/v1/secret/data/myapp/db"))
            .and(header("X-Vault-Token", "test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let config = VaultConfig {
            addr: mock_server.uri(),
            mount: "secret".to_string(),
        };
        let client = VaultClient::new(config, "test-token".to_string());

        let username = client.read_field("myapp/db", "username").await;
        let password = client.read_field("myapp/db", "password").await;

        assert!(username.is_ok());
        assert_eq!(username.unwrap(), "admin");
        assert!(password.is_ok());
        assert_eq!(password.unwrap(), "secret123");
    }

    #[cfg(feature = "integration-tests")]
    #[tokio::test]
    async fn test_vault_client_field_not_found() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "data": {
                "data": {
                    "value": "test"
                }
            }
        });

        Mock::given(method("GET"))
            .and(path("/v1/secret/data/myapp/db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let config = VaultConfig {
            addr: mock_server.uri(),
            mount: "secret".to_string(),
        };
        let client = VaultClient::new(config, "test-token".to_string());

        let result = client.read_field("myapp/db", "nonexistent").await;
        assert!(result.is_err());
        match result {
            Err(VaultError::FieldNotFound(path, field)) => {
                assert_eq!(path, "myapp/db");
                assert_eq!(field, "nonexistent");
            }
            _ => panic!("Expected FieldNotFound error"),
        }
    }

    #[cfg(feature = "integration-tests")]
    #[tokio::test]
    async fn test_vault_client_api_error() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/secret/data/forbidden"))
            .respond_with(ResponseTemplate::new(403).set_body_string("permission denied"))
            .mount(&mock_server)
            .await;

        let config = VaultConfig {
            addr: mock_server.uri(),
            mount: "secret".to_string(),
        };
        let client = VaultClient::new(config, "bad-token".to_string());

        let result = client.read_secret("forbidden").await;
        assert!(result.is_err());
        match result {
            Err(VaultError::ApiError(status, _)) => {
                assert_eq!(status, 403);
            }
            _ => panic!("Expected ApiError"),
        }
    }

    // ===== SecretResolver Vault Integration Tests =====

    #[tokio::test]
    async fn test_resolve_vault_not_configured() {
        env::remove_var("VAULT_ADDR");
        env::remove_var("VAULT_TOKEN");

        let resolver = SecretResolver::new();
        let result = resolver.resolve("vault", "myapp/db").await;

        assert!(result.is_err());
        match result {
            Err(SecretError::VaultNotConfigured) => {}
            _ => panic!("Expected VaultNotConfigured error"),
        }
    }

    #[cfg(feature = "integration-tests")]
    #[tokio::test]
    async fn test_resolve_vault_path_field_format() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "data": {
                "data": {
                    "username": "dbuser",
                    "password": "dbpass"
                }
            }
        });

        Mock::given(method("GET"))
            .and(path("/v1/secret/data/myapp/db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        env::set_var("VAULT_ADDR", mock_server.uri());
        env::set_var("VAULT_TOKEN", "test-token");
        env::set_var("VAULT_MOUNT", "secret");

        let resolver = SecretResolver::new();

        // Test default field (value would fail, but we have specific fields)
        let result = resolver.resolve("vault", "myapp/db:username").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "dbuser");

        // Test specific field
        let result = resolver.resolve("vault", "myapp/db:password").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "dbpass");

        env::remove_var("VAULT_ADDR");
        env::remove_var("VAULT_TOKEN");
        env::remove_var("VAULT_MOUNT");
    }

    // ===== Multi-source Resolution Tests =====

    #[tokio::test]
    async fn test_resolve_all() {
        env::set_var("API_KEY", "key123");
        env::set_var("DB_PASS", "pass456");

        let resolver = SecretResolver::new();
        let secrets = vec![
            ("api_key".to_string(), "env".to_string(), "API_KEY".to_string()),
            ("db_pass".to_string(), "env".to_string(), "DB_PASS".to_string()),
        ];

        let result = resolver.resolve_all(&secrets).await;
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.get("api_key").unwrap(), "key123");
        assert_eq!(resolved.get("db_pass").unwrap(), "pass456");

        env::remove_var("API_KEY");
        env::remove_var("DB_PASS");
    }

    #[tokio::test]
    async fn test_unknown_source() {
        let resolver = SecretResolver::new();
        let result = resolver.resolve("unknown", "key").await;

        assert!(result.is_err());
        match result {
            Err(SecretError::UnknownSource(source)) => {
                assert_eq!(source, "unknown");
            }
            _ => panic!("Expected UnknownSource error"),
        }
    }

    // ===== File Source Tests =====

    #[tokio::test]
    async fn test_resolve_from_file() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "file_secret_value\n").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let resolver = SecretResolver::new();
        let result = resolver.resolve("file", path).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "file_secret_value");
    }

    #[tokio::test]
    async fn test_resolve_missing_file() {
        let resolver = SecretResolver::new();
        let result = resolver.resolve("file", "/nonexistent/path/to/secret").await;

        assert!(result.is_err());
        match result {
            Err(SecretError::NotFound(msg)) => {
                assert!(msg.contains("file:/nonexistent/path/to/secret"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }
}
