//! Input validation for VM operations
//!
//! Validates VM names, resource limits, and other parameters.

use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;
use thiserror::Error;

/// Maximum resource limits
const MAX_VCPUS: u32 = 32;
const MAX_MEMORY_MB: u32 = 65536; // 64 GB
const MAX_DISK_GB: u32 = 500;
const MAX_VM_NAME_LENGTH: usize = 63;

/// Validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid VM name: {0}. Must match pattern ^agent-[a-z0-9-]+$ and be <= 63 characters")]
    InvalidVmName(String),

    #[error("VM name too long: {0} characters (max: {1})")]
    NameTooLong(usize, usize),

    #[error("Too many CPUs: {0} (max: {1})")]
    TooManyCpus(u32, u32),

    #[error("Too much memory: {0} MB (max: {1} MB)")]
    TooMuchMemory(u32, u32),

    #[error("Disk too large: {0} GB (max: {1} GB)")]
    DiskTooLarge(u32, u32),

    #[error("Invalid resource value: {field} must be greater than 0")]
    InvalidResourceValue { field: String },

    #[error("Invalid profile: {0}. Must be one of: agentic-dev, basic")]
    InvalidProfile(String),
}

impl ValidationError {
    /// Get error code for API responses
    pub fn error_code(&self) -> &'static str {
        match self {
            ValidationError::InvalidVmName(_) => "INVALID_VM_NAME",
            ValidationError::NameTooLong(_, _) => "VM_NAME_TOO_LONG",
            ValidationError::TooManyCpus(_, _) => "TOO_MANY_CPUS",
            ValidationError::TooMuchMemory(_, _) => "TOO_MUCH_MEMORY",
            ValidationError::DiskTooLarge(_, _) => "DISK_TOO_LARGE",
            ValidationError::InvalidResourceValue { .. } => "INVALID_RESOURCE_VALUE",
            ValidationError::InvalidProfile(_) => "INVALID_PROFILE",
        }
    }
}

/// Convert validation error to JSON response
#[derive(Serialize)]
pub struct ValidationErrorResponse {
    pub error: ValidationErrorDetail,
}

#[derive(Serialize)]
pub struct ValidationErrorDetail {
    pub code: String,
    pub message: String,
}

impl From<ValidationError> for ValidationErrorResponse {
    fn from(err: ValidationError) -> Self {
        Self {
            error: ValidationErrorDetail {
                code: err.error_code().to_string(),
                message: err.to_string(),
            },
        }
    }
}

/// VM name regex pattern
fn vm_name_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^agent-[a-z0-9-]+$").unwrap())
}

/// Validate VM name
///
/// Rules:
/// - Must match pattern: ^agent-[a-z0-9-]+$
/// - Must be <= 63 characters
/// - Cannot be empty
/// - Cannot end with hyphen
/// - Cannot have consecutive hyphens
pub fn validate_vm_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::InvalidVmName("empty name".to_string()));
    }

    if name.len() > MAX_VM_NAME_LENGTH {
        return Err(ValidationError::NameTooLong(
            name.len(),
            MAX_VM_NAME_LENGTH,
        ));
    }

    // Check for trailing hyphen
    if name.ends_with('-') {
        return Err(ValidationError::InvalidVmName(
            "cannot end with hyphen".to_string(),
        ));
    }

    // Check for consecutive hyphens
    if name.contains("--") {
        return Err(ValidationError::InvalidVmName(
            "cannot contain consecutive hyphens".to_string(),
        ));
    }

    if !vm_name_regex().is_match(name) {
        return Err(ValidationError::InvalidVmName(name.to_string()));
    }

    Ok(())
}

/// Validate VM resource limits
///
/// Rules:
/// - vcpus: 1 <= vcpus <= 32
/// - memory_mb: 1 <= memory_mb <= 65536
/// - disk_gb: 1 <= disk_gb <= 500
pub fn validate_resources(vcpus: u32, memory_mb: u32, disk_gb: u32) -> Result<(), ValidationError> {
    if vcpus == 0 {
        return Err(ValidationError::InvalidResourceValue {
            field: "vcpus".to_string(),
        });
    }

    if vcpus > MAX_VCPUS {
        return Err(ValidationError::TooManyCpus(vcpus, MAX_VCPUS));
    }

    if memory_mb == 0 {
        return Err(ValidationError::InvalidResourceValue {
            field: "memory_mb".to_string(),
        });
    }

    if memory_mb > MAX_MEMORY_MB {
        return Err(ValidationError::TooMuchMemory(memory_mb, MAX_MEMORY_MB));
    }

    if disk_gb == 0 {
        return Err(ValidationError::InvalidResourceValue {
            field: "disk_gb".to_string(),
        });
    }

    if disk_gb > MAX_DISK_GB {
        return Err(ValidationError::DiskTooLarge(disk_gb, MAX_DISK_GB));
    }

    Ok(())
}

/// Validate provisioning profile
///
/// Rules:
/// - Must be one of: "agentic-dev", "basic"
pub fn validate_profile(profile: &str) -> Result<(), ValidationError> {
    match profile {
        "agentic-dev" | "basic" => Ok(()),
        _ => Err(ValidationError::InvalidProfile(profile.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_vm_name_valid() {
        assert!(validate_vm_name("agent-01").is_ok());
        assert!(validate_vm_name("agent-test").is_ok());
        assert!(validate_vm_name("agent-test-123").is_ok());
        assert!(validate_vm_name("agent-my-vm-01").is_ok());
    }

    #[test]
    fn test_validate_vm_name_invalid_pattern() {
        // Must start with "agent-"
        assert!(validate_vm_name("vm-01").is_err());
        assert!(validate_vm_name("test").is_err());

        // No uppercase
        assert!(validate_vm_name("agent-TEST").is_err());
        assert!(validate_vm_name("Agent-01").is_err());

        // No underscores
        assert!(validate_vm_name("agent-test_01").is_err());

        // No special chars
        assert!(validate_vm_name("agent-test@01").is_err());
        assert!(validate_vm_name("agent-test.01").is_err());

        // Cannot end with hyphen
        assert!(validate_vm_name("agent-test-").is_err());
    }

    #[test]
    fn test_validate_vm_name_empty() {
        let result = validate_vm_name("");
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::InvalidVmName(_))));
    }

    #[test]
    fn test_validate_vm_name_too_long() {
        let long_name = format!("agent-{}", "a".repeat(60));
        let result = validate_vm_name(&long_name);
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::NameTooLong(_, _))));
    }

    #[test]
    fn test_validate_vm_name_max_length() {
        // Exactly 63 characters should be valid
        let name = "agent-".to_string() + &"a".repeat(57);
        assert_eq!(name.len(), 63);
        assert!(validate_vm_name(&name).is_ok());
    }

    #[test]
    fn test_validate_resources_valid() {
        assert!(validate_resources(4, 8192, 50).is_ok());
        assert!(validate_resources(1, 1, 1).is_ok());
        assert!(validate_resources(32, 65536, 500).is_ok());
    }

    #[test]
    fn test_validate_resources_too_many_cpus() {
        let result = validate_resources(33, 8192, 50);
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::TooManyCpus(33, 32))));
    }

    #[test]
    fn test_validate_resources_too_much_memory() {
        let result = validate_resources(4, 70000, 50);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ValidationError::TooMuchMemory(70000, 65536))
        ));
    }

    #[test]
    fn test_validate_resources_disk_too_large() {
        let result = validate_resources(4, 8192, 600);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ValidationError::DiskTooLarge(600, 500))
        ));
    }

    #[test]
    fn test_validate_resources_zero_vcpus() {
        let result = validate_resources(0, 8192, 50);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ValidationError::InvalidResourceValue { .. })
        ));
    }

    #[test]
    fn test_validate_resources_zero_memory() {
        let result = validate_resources(4, 0, 50);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ValidationError::InvalidResourceValue { .. })
        ));
    }

    #[test]
    fn test_validate_resources_zero_disk() {
        let result = validate_resources(4, 8192, 0);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ValidationError::InvalidResourceValue { .. })
        ));
    }

    #[test]
    fn test_validate_profile_valid() {
        assert!(validate_profile("agentic-dev").is_ok());
        assert!(validate_profile("basic").is_ok());
    }

    #[test]
    fn test_validate_profile_invalid() {
        assert!(validate_profile("invalid").is_err());
        assert!(validate_profile("production").is_err());
        assert!(validate_profile("").is_err());
    }

    #[test]
    fn test_validation_error_codes() {
        assert_eq!(
            ValidationError::InvalidVmName("test".to_string()).error_code(),
            "INVALID_VM_NAME"
        );
        assert_eq!(
            ValidationError::NameTooLong(70, 63).error_code(),
            "VM_NAME_TOO_LONG"
        );
        assert_eq!(
            ValidationError::TooManyCpus(40, 32).error_code(),
            "TOO_MANY_CPUS"
        );
        assert_eq!(
            ValidationError::TooMuchMemory(70000, 65536).error_code(),
            "TOO_MUCH_MEMORY"
        );
        assert_eq!(
            ValidationError::DiskTooLarge(600, 500).error_code(),
            "DISK_TOO_LARGE"
        );
        assert_eq!(
            ValidationError::InvalidResourceValue {
                field: "vcpus".to_string()
            }
            .error_code(),
            "INVALID_RESOURCE_VALUE"
        );
        assert_eq!(
            ValidationError::InvalidProfile("test".to_string()).error_code(),
            "INVALID_PROFILE"
        );
    }

    #[test]
    fn test_validation_error_response() {
        let err = ValidationError::InvalidVmName("bad-name".to_string());
        let response: ValidationErrorResponse = err.into();

        assert_eq!(response.error.code, "INVALID_VM_NAME");
        assert!(response.error.message.contains("bad-name"));
    }
}
