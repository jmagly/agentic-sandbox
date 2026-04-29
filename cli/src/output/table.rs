//! Minimal aligned-columns table renderer (no extra dependency).
//!
//! Use for `list` verbs where each row is a tuple of strings. Headers
//! and rows are passed in; widths are computed once over the whole set.
//! For very wide tables the caller should pre-truncate values — this
//! module does not do its own ellipsis logic.

use std::fmt::Write;

pub fn render<R: AsRef<[String]>>(headers: &[&str], rows: &[R]) -> String {
    if rows.is_empty() && headers.is_empty() {
        return String::new();
    }
    let n = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for r in rows {
        let r = r.as_ref();
        for (i, cell) in r.iter().enumerate().take(n) {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }

    let mut out = String::new();
    write_row(&mut out, &widths, &headers.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let _ = writeln!(out, "{}", widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "));
    for r in rows {
        write_row(&mut out, &widths, r.as_ref());
    }
    out
}

fn write_row(out: &mut String, widths: &[usize], cells: &[String]) {
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        if i + 1 == cells.len() {
            // Don't pad the trailing cell — saves trailing whitespace.
            let _ = write!(out, "{}", cell);
        } else {
            let _ = write!(out, "{:width$}  ", cell, width = w);
        }
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_aligned_columns() {
        let rows = vec![
            vec!["a".to_string(), "running".to_string()],
            vec!["bb".to_string(), "stopped".to_string()],
        ];
        let out = render(&["NAME", "STATE"], &rows);
        // Two header lines plus two rows.
        assert_eq!(out.lines().count(), 4);
        // Trailing cell has no padding.
        assert!(out.contains("running"));
        assert!(out.contains("stopped"));
    }

    #[test]
    fn handles_empty_rows() {
        let rows: Vec<Vec<String>> = Vec::new();
        let out = render(&["NAME"], &rows);
        // Header + separator only.
        assert_eq!(out.lines().count(), 2);
    }
}
