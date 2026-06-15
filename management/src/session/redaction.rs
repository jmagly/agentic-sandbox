use std::sync::OnceLock;

use regex::Regex;

const REDACTION_MARKER: &str = "[REDACTED:pty-secret]";

static SECRET_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

pub fn redact_pty_bytes(data: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    let mut redacted = text.into_owned();
    for pattern in secret_patterns() {
        redacted = pattern
            .replace_all(&redacted, REDACTION_MARKER)
            .into_owned();
    }
    redacted.into_bytes()
}

fn secret_patterns() -> &'static [Regex] {
    SECRET_PATTERNS.get_or_init(|| {
        [
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            r"sk-ant-[A-Za-z0-9_-]{16,}",
            r"sk-[A-Za-z0-9_-]{16,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"gh[pousr]_[A-Za-z0-9_]{20,}",
            r"(?i)bearer[[:space:]]+[A-Za-z0-9._~+/=-]{16,}",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("valid PTY redaction pattern"))
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_provider_secret_shapes() {
        let raw = b"openai=sk-testsecret0000000000 github=ghp_abcdefghijklmnopqrstuvwxyz bearer Bearer abcdefghijklmnop private -----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----";
        let redacted = String::from_utf8(redact_pty_bytes(raw)).unwrap();
        assert!(!redacted.contains("sk-testsecret"));
        assert!(!redacted.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(!redacted.contains("Bearer abcdefghijklmnop"));
        assert!(!redacted.contains("BEGIN OPENSSH PRIVATE KEY"));
        assert!(redacted.contains(REDACTION_MARKER));
    }
}
