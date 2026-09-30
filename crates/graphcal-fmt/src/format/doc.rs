//! Layout helpers over `pretty` documents shared by every formatter module.

use pretty::RcDoc;

/// Indentation step used everywhere in formatter docs. Typed as `isize`
/// because `pretty::RcDoc::nest` takes `isize`; the few `repeat` sites
/// cast through `usize` explicitly.
pub const INDENT: isize = 4;

pub(super) fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

pub(super) fn pad_left_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{}", " ".repeat(padding), text)
}

pub(super) fn pad_right_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{}", text, " ".repeat(padding))
}

pub(super) fn text_with_hardlines(text: &str) -> RcDoc<'static> {
    RcDoc::intersperse(
        text.split('\n').map(|line| {
            if line.is_empty() {
                RcDoc::nil()
            } else {
                RcDoc::text(line.to_string())
            }
        }),
        RcDoc::hardline(),
    )
}

/// Prepend leading comments before a doc. Returns the doc unchanged if
/// there are no comments. Like Gleam's `commented()` helper.
pub fn prepend_comments(leading: Option<RcDoc<'static>>, doc: RcDoc<'static>) -> RcDoc<'static> {
    match leading {
        Some(leading) => leading.append(doc),
        None => doc,
    }
}

/// Pretty-printer pattern: try `single` on one line, otherwise lay `multi`
/// out. Both branches are required because `pretty::RcDoc::group` cannot
/// switch between fundamentally different shapes (e.g., adding hardlines).
pub fn flat_alt_group(single: RcDoc<'static>, multi: RcDoc<'static>) -> RcDoc<'static> {
    multi.flat_alt(single).group()
}

/// Wrap a possibly-multiline child document in soft parentheses.
///
/// Delimited expression contexts must not append a child directly after the
/// opening delimiter (for example, `sum(` + `for ... { ... }`): if the child
/// later chooses a multiline layout, its internal hardlines inherit the wrong
/// indentation anchor. This combinator centralizes the safe layout invariant:
/// the body is preceded and followed by `line_()`, so it stays inline when the
/// whole group fits but moves to its own indented line whenever any nested
/// hardline or width break makes the group multiline.
pub fn soft_parenthesized(body: RcDoc<'static>) -> RcDoc<'static> {
    RcDoc::text("(")
        .append(RcDoc::line_().append(body).nest(INDENT))
        .append(RcDoc::line_())
        .append(RcDoc::text(")"))
        .group()
}

/// Format a comma-separated argument list in soft parentheses.
///
/// Empty argument lists are always emitted as the atomic token pair `()`.
/// Without this special case, a long callee can force `soft_parenthesized(nil)`
/// into the multiline layout and produce a visually empty parenthesis block:
/// `foo(\n\n)`. List formatters should use this helper instead of building
/// parenthesized comma lists by hand so empty delimiter pairs stay stable in
/// every surrounding layout.
pub fn soft_parenthesized_list(
    items: Vec<RcDoc<'static>>,
    trailing_comma_when_multiline: bool,
) -> RcDoc<'static> {
    if items.is_empty() {
        return RcDoc::text("()");
    }

    let body = RcDoc::intersperse(items, RcDoc::text(",").append(RcDoc::line()));
    let body = if trailing_comma_when_multiline {
        body.append(RcDoc::text(",").flat_alt(RcDoc::nil()))
    } else {
        body
    };

    soft_parenthesized(body)
}

/// Format a non-empty comma-separated list in parentheses using an explicitly
/// expanded layout.
///
/// This is the forced counterpart to [`soft_parenthesized_list`], used when
/// source syntax such as a magic trailing comma requests multiline output even
/// though the list would fit on one line.
pub fn multiline_parenthesized_list(
    items: Vec<RcDoc<'static>>,
    trailing_comma: bool,
) -> RcDoc<'static> {
    if items.is_empty() {
        return RcDoc::text("()");
    }

    let body = RcDoc::intersperse(items, RcDoc::text(",").append(RcDoc::hardline()));
    let body = if trailing_comma {
        body.append(RcDoc::text(","))
    } else {
        body
    };

    RcDoc::text("(")
        .append(RcDoc::hardline().append(body).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text(")"))
}

/// Render an `RcDoc` to a string (for measuring column widths).
pub fn render_doc_to_string(doc: &RcDoc<'static>) -> String {
    let mut buf = Vec::new();
    // Use a large width so we get single-line rendering for cell values.
    let _ = doc.render(1000, &mut buf);
    // The doc is built from valid UTF-8 source slices and string literals,
    // so render output is always valid UTF-8 — panic loudly if that
    // invariant is ever violated rather than silently producing "".
    #[expect(
        clippy::expect_used,
        reason = "doc bytes are always valid UTF-8 by construction"
    )]
    String::from_utf8(buf).expect("rendered doc must be valid UTF-8")
}

#[cfg(test)]
mod tests {
    use pretty::RcDoc;

    use super::{display_width, pad_left_to_width, pad_right_to_width, text_with_hardlines};

    #[test]
    fn alignment_helpers_use_display_width_not_bytes() {
        assert_eq!(display_width("界"), 2);
        assert_eq!("界".len(), 3);
        assert_eq!(pad_left_to_width("界", 4), "  界");
        assert_eq!(pad_right_to_width("界", 4), "界  ");
    }

    #[test]
    fn multiline_text_uses_hardlines_for_nesting() {
        let doc = RcDoc::text("{")
            .append(
                RcDoc::hardline()
                    .append(text_with_hardlines("a\nb"))
                    .nest(4),
            )
            .append(RcDoc::hardline())
            .append(RcDoc::text("}"));
        let mut out = Vec::new();
        doc.render(80, &mut out).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered, "{\n    a\n    b\n}");
    }
}
