//! Escaping helpers for embedding untrusted strings in generated HTML.

/// HTML-escape a string to prevent XSS when interpolated into HTML content.
#[must_use]
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a JSON string for safe embedding inside an HTML `<script>` element.
///
/// `serde_json` does not escape `</`, so a user-controlled string containing
/// `</script>` would close the script tag. Replace `<` with `\u003c` to
/// neutralize any `</script>` or `<!--` sequences in the JSON payload.
#[must_use]
pub fn escape_json_for_script(s: &str) -> String {
    s.replace('<', r"\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_handles_critical_characters() {
        assert_eq!(
            html_escape("<img src=x onerror=alert(1)>"),
            "&lt;img src=x onerror=alert(1)&gt;"
        );
        assert_eq!(html_escape("\"'&"), "&quot;&#x27;&amp;");
    }

    #[test]
    fn script_escape_neutralizes_closing_tags() {
        assert_eq!(
            escape_json_for_script("</script><script>"),
            r"\u003c/script>\u003cscript>"
        );
    }
}
