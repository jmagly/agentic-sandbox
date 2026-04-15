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

/// Detect whether the terminal appears to be waiting for user input.
///
/// `screen_text` should be the current visible screen as plain text
/// (newline-separated rows, as returned by `vt100::Screen::contents()`).
///
/// Returns `Some(PromptMatch)` if a known input-waiting pattern is found on
/// the last non-empty line, `None` otherwise.
pub fn detect_prompt(screen_text: &str) -> Option<PromptMatch> {
    let last_line = screen_text.lines().rev().find(|l| !l.trim().is_empty())?;

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
    fn test_empty() {
        assert!(detect_prompt("").is_none());
        assert!(detect_prompt("   \n  \n  ").is_none());
    }
}
