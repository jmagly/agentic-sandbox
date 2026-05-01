//! Key:value block renderer for `get` verbs.
//!
//! Aligned, compact, and stable. Use for inspect commands where the
//! output is a single record. Multi-record output goes through
//! `output::table` instead.

use std::fmt::Write;

pub fn render(pairs: &[(&str, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let key_w = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (k, v) in pairs {
        let _ = writeln!(out, "{:width$}  {}", k, v, width = key_w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_keys_to_widest() {
        let pairs = [
            ("name", "agent-01".into()),
            ("ip_address", "192.168.122.5".into()),
        ];
        let out = render(&pairs);
        // Keys padded to width of "ip_address" (10).
        assert!(out.lines().next().unwrap().starts_with("name      "));
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let out = render(&[]);
        assert!(out.is_empty());
    }
}
