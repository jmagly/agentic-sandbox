//! Provider resolution for managed task execution (issue #5).
//!
//! G1 scope: routing guard only. The `claude` executor remains the sole
//! registrant; `dsh` and future providers register here as their executors
//! land (see docs/proposals/provider-executor-generalization-plan.md).
//! Resolution is total: absent manifest field defaults to `claude`, and any
//! unregistered provider fails closed with a structured error rather than
//! silently falling back.

/// Task-capable executors registered with the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
}

impl Provider {
    /// Resolve the manifest-declared `provider` field. Absent defaults to
    /// `claude` for backward compatibility with existing task manifests.
    pub fn resolve(provider: Option<&str>) -> Result<Self, UnsupportedProvider> {
        match provider {
            None | Some("claude") => Ok(Self::Claude),
            Some(other) => Err(UnsupportedProvider(other.to_string())),
        }
    }
}

/// A manifest declared a provider with no registered executor.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unsupported task provider: {0}")]
pub struct UnsupportedProvider(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_provider_defaults_to_claude() {
        assert_eq!(Provider::resolve(None), Ok(Provider::Claude));
    }

    #[test]
    fn explicit_claude_resolves() {
        assert_eq!(Provider::resolve(Some("claude")), Ok(Provider::Claude));
    }

    #[test]
    fn unregistered_provider_fails_closed() {
        let err = Provider::resolve(Some("dsh")).unwrap_err();
        assert_eq!(err.0, "dsh");
        assert!(err.to_string().contains("unsupported task provider: dsh"));
    }

    #[test]
    fn empty_string_is_not_claude() {
        assert!(Provider::resolve(Some("")).is_err());
    }
}
