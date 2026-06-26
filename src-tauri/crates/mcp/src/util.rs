//! Small output-parity helpers shared across the tool clusters.

/// Python `repr()` of a string, for `!r` error-message parity. Python defaults to single
/// quotes, switching to double only when the string holds a `'` but no `"`. Rust's `{:?}`
/// always uses double quotes, so it diverges byte-for-byte on every id in an error message.
pub fn pyrepr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
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
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::pyrepr;

    #[test]
    fn pyrepr_matches_python_repr() {
        assert_eq!(pyrepr("arxiv:1234.5678"), "'arxiv:1234.5678'");
        assert_eq!(pyrepr("O'Brien"), "\"O'Brien\"");
        assert_eq!(pyrepr("a'b\"c"), "'a\\'b\"c'");
    }
}
