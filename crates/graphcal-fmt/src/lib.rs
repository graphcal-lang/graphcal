mod format;

use graphcal_syntax::comments::extract_source_metadata;
use graphcal_syntax::parser::Parser;

/// Default line width for formatting.
const LINE_WIDTH: usize = 100;

/// Format a `.gcl` source string, returning the formatted output.
///
/// If the source fails to parse, returns `None`.
#[must_use]
pub fn format_source(source: &str) -> Option<String> {
    let metadata = extract_source_metadata(source);
    let file = Parser::new(source).parse_file().ok()?;
    let doc = format::format_file(&file, source, &metadata);

    let mut output = Vec::new();
    doc.render(LINE_WIDTH, &mut output).ok()?;
    let mut result = String::from_utf8(output).ok()?;

    // Ensure trailing newline
    if !result.ends_with('\n') {
        result.push('\n');
    }

    Some(result)
}
