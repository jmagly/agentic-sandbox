//! Prompt detection heuristics for PTY output.
//!
//! Shared between the HITL emitter (#133) and the screen observer (#136).
//! All heuristics operate on plain text (after VT sequences have been stripped
//! or the screen has been rendered to a string).

/// Result of a prompt detection scan
#[derive(Debug, Clone)]
pub struct PromptMatch {
    /// The line or fragment that matched
    pub text: String,
    /// Heuristic confidence: 0.0–1.0
    pub confidence: f32,
}

/// Strip ANSI escape sequences (CSI, OSC, simple ESC X) from a string.
///
/// Used to sanitize raw PTY output before it is shown in non-terminal UI
/// (HITL popup, REST responses) where escape codes would render as garbled
/// text such as `[K[K` or `[33;31H`.
pub fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                b'[' => {
                    // CSI: ESC [ params terminator (0x40..=0x7E)
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: ESC ] ... BEL or ESC \
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                b'P' | b'X' | b'^' | b'_' => {
                    // DCS / SOS / PM / APC: terminated by ST (ESC \) or BEL
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Two-byte ESC sequence (e.g. ESC =, ESC >, ESC (B)
                    i += 2;
                    if matches!(next, b'(' | b')' | b'*' | b'+') && i < bytes.len() {
                        i += 1;
                    }
                }
            }
        } else if b == 0x1b {
            // Trailing lone ESC
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    // Also drop bare control chars commonly left behind (BEL, NUL).
    out.retain(|&c| c != 0x07 && c != 0x00);
    String::from_utf8_lossy(&out).into_owned()
}

/// Detect whether the terminal appears to be waiting for user input.
///
/// `screen_text` should be the current visible screen as plain text
/// (newline-separated rows, as returned by `vt100::Screen::contents()`).
///
/// Returns `Some(PromptMatch)` if a known input-waiting pattern is found on
/// the last non-empty line, `None` otherwise.
pub fn detect_prompt(screen_text: &str) -> Option<PromptMatch> {
    // Defensive: callers may pass raw PTY bytes that still contain VT
    // sequences. Strip them so detection and the returned text are clean.
    let cleaned = strip_ansi(screen_text);
    let last_line = cleaned.lines().rev().find(|l| !l.trim().is_empty())?;

    // Match against the raw line (preserving trailing space so patterns like
    // "❯ " and "$ " work correctly), but return the trimmed text.
    let trimmed = last_line.trim_end();

    // High-confidence patterns — explicit choice or question markers
    let high_confidence: &[(&str, f32)] = &[
        // Claude Code interactive prompts
        ("❯ ", 0.95),
        ("Human: ", 0.95),
        // Generic Y/N choice patterns
        ("(y/n)", 0.95),
        ("(yes/no)", 0.95),
        ("[Y/n]", 0.95),
        ("[y/N]", 0.95),
        ("[Y/N]", 0.95),
        // "Do you want to proceed"
        ("Do you want to proceed", 0.90),
        ("Would you like to", 0.90),
        ("Are you sure", 0.90),
        ("Press any key", 0.85),
        ("Press ENTER", 0.85),
        ("press enter", 0.85),
    ];

    for (pattern, confidence) in high_confidence {
        if last_line.contains(pattern) {
            return Some(PromptMatch {
                text: trimmed.to_string(),
                confidence: *confidence,
            });
        }
    }

    // Medium-confidence — shell prompt endings and question marks
    let medium_confidence: &[(&str, f32)] = &[("$ ", 0.60), ("# ", 0.60), ("> ", 0.55)];

    for (pattern, confidence) in medium_confidence {
        if last_line.ends_with(pattern) {
            return Some(PromptMatch {
                text: trimmed.to_string(),
                confidence: *confidence,
            });
        }
    }

    // Low-confidence — line ends with a question mark
    if trimmed.ends_with('?') {
        return Some(PromptMatch {
            text: trimmed.to_string(),
            confidence: 0.40,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_prompt() {
        let text = "Some output\n❯ ";
        let m = detect_prompt(text).unwrap();
        assert!(m.confidence >= 0.90);
    }

    #[test]
    fn test_yn_choice() {
        let text = "Delete 3 files? (y/n)";
        let m = detect_prompt(text).unwrap();
        assert!(m.confidence >= 0.90);
    }

    #[test]
    fn test_shell_prompt() {
        let text = "agent@vm:~$ ";
        let m = detect_prompt(text).unwrap();
        assert!(m.confidence >= 0.55);
    }

    #[test]
    fn test_no_prompt() {
        let text = "Building project...\nCompiling main.rs";
        assert!(detect_prompt(text).is_none());
    }

    #[test]
    fn test_strip_ansi_csi() {
        let raw = "\x1b[K\x1b[K Add a custom script? [y/N]: \x1b[1;34r\x1b[33;31H";
        assert_eq!(strip_ansi(raw), " Add a custom script? [y/N]: ");
    }

    #[test]
    fn test_strip_ansi_osc_and_bel() {
        let raw = "\x1b]0;title\x07hello\x1b]2;more\x1b\\world";
        assert_eq!(strip_ansi(raw), "helloworld");
    }

    #[test]
    fn test_detect_prompt_strips_escapes() {
        let raw = "\x1b[K\x1b[K Add a custom script? [y/N]: \x1b[1;34r\x1b[33;31H";
        let m = detect_prompt(raw).unwrap();
        assert!(m.confidence >= 0.90);
        assert!(!m.text.contains('\x1b'));
        assert!(!m.text.contains("[1;34r"));
        assert!(m.text.contains("Add a custom script?"));
    }

    #[test]
    fn test_empty() {
        assert!(detect_prompt("").is_none());
        assert!(detect_prompt("   \n  \n  ").is_none());
    }
}
