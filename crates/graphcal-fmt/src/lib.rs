mod format;

use graphcal_compiler::syntax::parser::{ParseError, Parser};

/// Default line width for formatting.
const LINE_WIDTH: usize = 100;

/// Error returned when [`format_source`] cannot produce formatted output.
///
/// Each variant reflects a distinct failure mode — callers can report them
/// specifically instead of conflating them into a single "parse error".
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// The source failed to parse.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// Rendering the formatted document to bytes failed (should not happen
    /// with an in-memory `Vec<u8>` writer; kept as a typed variant so future
    /// writer backends can surface real I/O errors without widening the API).
    #[error("failed to render formatted output: {0}")]
    Render(#[from] std::io::Error),
    /// The rendered bytes were not valid UTF-8 (should not happen — the
    /// renderer emits UTF-8 — but we keep the variant explicit so we never
    /// silently hide the case).
    #[error("formatted output was not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Format a `.gcl` source string, returning the formatted output.
///
/// # Errors
///
/// Returns [`FormatError::Parse`] if `source` cannot be parsed,
/// [`FormatError::Render`] if rendering the formatted document fails,
/// or [`FormatError::Utf8`] if the rendered bytes are not valid UTF-8.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    let mut parser = Parser::new(source);
    let file = parser.parse_file()?;
    let metadata = parser.into_source_metadata();
    let doc = format::format_file(&file, source, &metadata);

    let mut output = Vec::new();
    doc.render(LINE_WIDTH, &mut output)?;
    let mut result = String::from_utf8(output)?;

    strip_trailing_horizontal_whitespace(&mut result);

    // Ensure trailing newline
    if !result.ends_with('\n') {
        result.push('\n');
    }

    Ok(result)
}

fn strip_trailing_horizontal_whitespace(s: &mut String) {
    let mut stripped = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(without_newline) => {
                stripped.push_str(without_newline.trim_end_matches([' ', '\t']));
                stripped.push('\n');
            }
            None => stripped.push_str(line.trim_end_matches([' ', '\t'])),
        }
    }
    *s = stripped;
}
