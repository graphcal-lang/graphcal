//! `textDocument/formatting` handler.

use graphcal_compiler::cancellation::CancellationToken;
use tower_lsp::lsp_types::{Position, Range, TextEdit};

use crate::convert::LineIndex;

/// Format a graphcal source string, returning a single whole-document [`TextEdit`]
/// if the formatted output differs from the original.
///
/// Returns `Ok(None)` when the formatted output is identical. Parse and
/// internal formatter failures remain distinct typed errors for the server
/// boundary to handle explicitly.
#[cfg(test)]
fn format_document(source: &str) -> Result<Option<Vec<TextEdit>>, Box<graphcal_fmt::FormatError>> {
    format_document_with(source, |input| {
        graphcal_fmt::format_source(input).map_err(Box::new)
    })
}

pub fn format_document_with_cancellation(
    source: &str,
    cancellation: &CancellationToken,
) -> Result<Option<Vec<TextEdit>>, Box<graphcal_fmt::FormatError>> {
    format_document_with(source, |input| {
        graphcal_fmt::format_source_with_cancellation(input, cancellation).map_err(Box::new)
    })
}

fn format_document_with(
    source: &str,
    render: impl FnOnce(&str) -> Result<String, Box<graphcal_fmt::FormatError>>,
) -> Result<Option<Vec<TextEdit>>, Box<graphcal_fmt::FormatError>> {
    let output = render(source)?;

    if output == source {
        return Ok(None);
    }

    let end = LineIndex::new(source).position(source.len());
    let range = Range::new(Position::new(0, 0), end);

    Ok(Some(vec![TextEdit {
        range,
        new_text: output,
    }]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_parse_error_for_invalid_source() {
        let result = format_document("this is not valid gcl {{{{");
        assert!(matches!(
            result,
            Err(error) if matches!(*error, graphcal_fmt::FormatError::Parse(_))
        ));
    }

    #[test]
    fn propagates_internal_formatter_errors() {
        let result = format_document_with("source", |_| {
            Err(Box::new(graphcal_fmt::FormatError::AstChanged))
        });
        assert!(matches!(
            result,
            Err(error) if matches!(*error, graphcal_fmt::FormatError::AstChanged)
        ));
    }

    #[test]
    fn returns_none_when_already_formatted() {
        let source = "node x: Dimensionless = 1;\n";
        let result = format_document(source).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn returns_edit_when_changed() {
        let source = "node   x:   Dimensionless  =  1;\n";
        let result = format_document(source).unwrap();
        assert!(result.is_some());
        let edits = result.unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].new_text, "node x: Dimensionless = 1;\n");
    }

    #[test]
    fn wraps_long_logical_chains() {
        let source = "pub index Mode = { First, Second, Third, Fourth };\n\
param priority: Int[Mode];\n\
assert priorities = @priority[Mode#First] == 1 && @priority[Mode#Second] == 2 && @priority[Mode#Third] == 3 && @priority[Mode#Fourth] == 4;\n";
        let edits = format_document(source)
            .expect("long chain should format")
            .expect("long chain should produce an edit");
        assert_eq!(edits.len(), 1);
        assert!(
            edits[0]
                .new_text
                .contains("\n    && @priority[Mode#Second] == 2")
        );
    }
}
