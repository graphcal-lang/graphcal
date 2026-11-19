//! Test helper applying editor edits to a source text.

use tower_lsp::lsp_types::TextEdit;

/// `source` with the non-overlapping `edits` applied.
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only helper shared with other modules' tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) fn apply_text_edits(source: &str, edits: &[TextEdit]) -> String {
    let mut edits: Vec<_> = edits
        .iter()
        .map(|edit| {
            (
                crate::convert::position_to_byte_offset(source, edit.range.start),
                crate::convert::position_to_byte_offset(source, edit.range.end),
                edit.new_text.as_str(),
            )
        })
        .collect();
    edits.sort_unstable_by_key(|edit| std::cmp::Reverse(edit.0));
    edits.into_iter().fold(
        source.to_string(),
        |mut output, (start, end, replacement)| {
            output.replace_range(start..end, replacement);
            output
        },
    )
}
