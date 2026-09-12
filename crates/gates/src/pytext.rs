//! The small text facts the Python gates relied on, kept identical.
//!
//! These are the pieces where Rust's standard library and Python's differ and a
//! port would silently move a verdict: `str.splitlines` (which splits on eight
//! separators, not one), `str.strip`/`str.isspace` (Unicode, plus `\x1c`-`\x1f`
//! which Rust calls control characters), the `errors="replace"` decode, and
//! Python's `%r` on a path, which one error line prints. Nothing here is
//! general-purpose; it exists so `provenance_check.py`'s line predicates and
//! `journal_gate.py`'s `splitlines`-based scan keep their verdicts.

use std::fs;
use std::path::Path;

/// `str.isspace()`: Unicode whitespace, and `\x1c`-`\x1f`, which Python counts
/// as space and `char::is_whitespace` does not.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// `str.strip()` with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// The separators `str.splitlines()` splits on, after universal-newline
/// translation has already folded `\r` and `\r\n` to `\n`.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines()`: a trailing separator yields no empty final line, and the
/// empty string yields no lines at all.
pub fn splitlines(s: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if is_line_break(c) {
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// `open(path, "r", encoding="utf-8", errors="replace")`: universal newlines
/// (`\r\n` and `\r` become `\n`) and one replacement character per bad byte.
pub fn read_text(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Python `%s` on a list of strings.
pub fn py_list_repr_str(items: &[String]) -> String {
    format!(
        "[{}]",
        items.iter().map(|item| py_repr(item)).collect::<Vec<_>>().join(", ")
    )
}

/// Python `%s` on a list of literal strings.
pub fn py_list_repr_lit(items: &[&str]) -> String {
    format!(
        "[{}]",
        items.iter().map(|item| py_repr(item)).collect::<Vec<_>>().join(", ")
    )
}

/// Python `%r` for the strings the gates interpolate into an error line: a path
/// with no quote, backslash or control character renders in single quotes.
pub fn py_repr(s: &str) -> String {
    let needs_double = s.contains('\'') && !s.contains('"');
    let quote = if needs_double { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
