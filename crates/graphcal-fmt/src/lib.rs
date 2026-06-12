//! Formatting for `.gcl` sources: parse, pretty-print, and verify the
//! result re-parses to the same syntax tree.
#![expect(
    clippy::result_large_err,
    reason = "FormatError embeds ParseError, which is inherently large and only constructed on the error path"
)]

mod format;

use graphcal_compiler::syntax::ast::{File, FormatEquivalent};
use graphcal_compiler::syntax::parser::{ParseError, Parser};

/// Default line width for formatting.
const LINE_WIDTH: usize = 100;

/// Stack segment size for building, rendering, and dropping the pretty-print
/// document.
///
/// The `RcDoc` tree is as deep as the deepest expression chain in the file,
/// and dropping it recurses inside the `pretty` crate's `Rc` drop glue —
/// third-party code that [`graphcal_compiler::stack::with_stack_growth`]
/// cannot intercept per level. A single large pre-grown segment covers
/// chains of several hundred thousand terms.
const DOC_STACK_SIZE: usize = 64 * 1024 * 1024;

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
    /// The formatted output failed to parse back into an AST.
    ///
    /// Distinct from [`Self::Parse`] (which means the *input* was invalid):
    /// this means the formatter emitted text the parser rejects — a formatter
    /// bug, never a user error. Carried without `#[from]` so it cannot be
    /// conflated with an input parse failure.
    #[error("internal formatter error: formatted output did not re-parse: {0}")]
    Reparse(ParseError),
    /// The formatted output parsed, but into a *different* syntax tree than the
    /// input (ignoring spans).
    ///
    /// Formatting must only change layout, never meaning — so this is always a
    /// formatter bug. Surfacing it as an error (instead of silently emitting
    /// the changed output) upholds the project's safety-over-usability stance:
    /// better to refuse than to quietly rewrite a program.
    #[error("internal formatter error: formatting changed the program's syntax tree")]
    AstChanged,
}

/// Format a `.gcl` source string, returning the formatted output.
///
/// # Errors
///
/// Returns [`FormatError::Parse`] if `source` cannot be parsed,
/// [`FormatError::Render`] if rendering the formatted document fails,
/// or [`FormatError::Utf8`] if the rendered bytes are not valid UTF-8.
///
/// As a self-check, the formatted output is re-parsed and compared against the
/// input AST (ignoring spans). Returns [`FormatError::Reparse`] if the output
/// fails to parse, or [`FormatError::AstChanged`] if it parses into a different
/// tree — both indicate a formatter bug, never invalid input.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    let mut parser = Parser::new(source);
    let file = parser.parse_file()?;
    let metadata = parser.into_source_metadata();

    // Build, render, AND drop the document inside the grown segment: the
    // drop is the deepest recursion (see `DOC_STACK_SIZE`).
    let output = stacker::grow(DOC_STACK_SIZE, || -> Result<Vec<u8>, std::io::Error> {
        let doc = format::format_file(&file, source, &metadata);
        let mut output = Vec::new();
        doc.render(LINE_WIDTH, &mut output)?;
        Ok(output)
    })?;
    let mut result = String::from_utf8(output)?;

    strip_trailing_horizontal_whitespace(&mut result);

    // Ensure trailing newline
    if !result.ends_with('\n') {
        result.push('\n');
    }

    verify_ast_preserved(&file, &result)?;

    Ok(result)
}

/// Confirm that formatting changed only layout, not the program.
///
/// Re-parses the formatted output and checks it yields the same AST as the
/// input, ignoring source spans (see
/// [`FormatEquivalent`](graphcal_compiler::syntax::ast::FormatEquivalent)).
/// Any divergence is a formatter bug, reported as a [`FormatError`] rather than
/// silently returning text whose meaning may differ from the source.
fn verify_ast_preserved(original: &File, formatted: &str) -> Result<(), FormatError> {
    let reparsed = Parser::new(formatted)
        .parse_file()
        .map_err(FormatError::Reparse)?;
    if original.format_equivalent(&reparsed) {
        Ok(())
    } else {
        Err(FormatError::AstChanged)
    }
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
