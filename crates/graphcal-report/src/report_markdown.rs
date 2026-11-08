//! Deterministic Markdown rendering of one [`ReportDocument`].
//!
//! Intended for CI diffing (#1153): stable ordering, no timestamps, plain
//! pipe tables. Figures are listed by name only — their data lives in the
//! HTML artifact, and repeating it here would bury value diffs in noise.

use std::fmt::Write;

use crate::report_ir::{CardBody, CheckStatus, ReportDocument, ValueCard};
use crate::value_display::{GridTable, ValueBody};

/// Render the report as Markdown.
#[must_use]
pub fn render_report_markdown(document: &ReportDocument) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", text(&document.title));
    if document.incomplete {
        out.push_str("\n> Model incomplete: unfinished formulas remain.\n");
    }

    if !document.call_notices.is_empty() {
        out.push_str("\n## DAG calls\n\n");
        for notice in &document.call_notices {
            let _ = writeln!(out, "- {}: {}", code(&notice.name), text(&notice.message));
        }
    }

    if !document.params.is_empty() {
        out.push_str("\n## Inputs\n\n");
        for card in &document.params {
            push_card(&mut out, card);
        }
    }

    if !document.values.is_empty() {
        out.push_str("\n## Values\n\n");
        for card in &document.values {
            push_card(&mut out, card);
        }
    }

    if !document.figures.is_empty() || !document.plot_errors.is_empty() {
        out.push_str("\n## Plots\n\n");
        for card in &document.figures {
            match &card.doc {
                Some(doc) => {
                    let _ = writeln!(out, "- {} — {}", code(&card.figure.name), text(doc));
                }
                None => {
                    let _ = writeln!(out, "- {}", code(&card.figure.name));
                }
            }
        }
        for error in &document.plot_errors {
            let status = if error.incomplete {
                "INCOMPLETE"
            } else {
                "ERROR"
            };
            let _ = writeln!(
                out,
                "- {} — {status}: {}",
                code(&error.name),
                text(&error.message)
            );
        }
    }

    if !document.presentation_diagnostics.is_empty() {
        out.push_str("\n## Presentation diagnostics\n\n");
        for diagnostic in &document.presentation_diagnostics {
            let _ = writeln!(out, "- {}", text(diagnostic));
        }
    }

    if !document.checks.is_empty() {
        out.push_str("\n## Checks\n\n");
        for check in &document.checks {
            let label = match check.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Fail => "FAIL",
                CheckStatus::Error => "ERROR",
                CheckStatus::Blocked => "BLOCKED",
            };
            let _ = write!(out, "- {label} {}", code(&check.name));
            if let Some(message) = &check.message {
                let _ = write!(out, " — {}", text(message));
            }
            if !check.affected.is_empty() {
                let affected = check
                    .affected
                    .iter()
                    .map(|name| code(name))
                    .collect::<Vec<_>>();
                let _ = write!(out, " (affected: {})", affected.join(", "));
            }
            out.push('\n');
        }
    }

    let provenance = &document.provenance;
    out.push_str("\n## Provenance\n\n");
    let _ = writeln!(
        out,
        "- Compiler: Graphcal {}",
        text(&provenance.compiler_version)
    );
    for source in &provenance.sources {
        let _ = writeln!(
            out,
            "- Source: {} sha256:{}",
            code(&source.name),
            text(&source.sha256)
        );
    }
    for (name, value) in &provenance.baseline_params {
        let _ = writeln!(out, "- Baseline: {} = {}", code(name), text(value));
    }
    let _ = writeln!(out, "- Reproduce: {}", code(&provenance.repro_command));
    out
}

fn push_card(out: &mut String, card: &ValueCard) {
    match &card.body {
        CardBody::Value(ValueBody::Scalar(display)) => {
            let _ = write!(out, "- {} = {}", code(&card.name), text(display));
            push_doc_suffix(out, card);
        }
        CardBody::Value(ValueBody::Entries(entries)) => {
            let _ = write!(out, "- {}", code(&card.name));
            push_doc_suffix(out, card);
            for (label, display) in entries {
                let _ = writeln!(out, "    - {} = {}", code(label), text(display));
            }
        }
        CardBody::Value(ValueBody::Grid(grid)) => {
            let _ = write!(out, "- {}", code(&card.name));
            push_doc_suffix(out, card);
            out.push('\n');
            push_grid(out, grid);
        }
        CardBody::Value(ValueBody::Slices(slices)) => {
            let _ = write!(out, "- {}", code(&card.name));
            push_doc_suffix(out, card);
            for (label, grid) in slices {
                let _ = writeln!(out, "\n    \\[{}\\]\n", text(label));
                push_grid(out, grid);
            }
        }
        CardBody::Incomplete { message } => {
            let _ = write!(out, "- {} = {}", code(&card.name), text(message));
            push_doc_suffix(out, card);
        }
        CardBody::Error { message } => {
            let _ = write!(out, "- {} = ERROR: {}", code(&card.name), text(message));
            push_doc_suffix(out, card);
        }
    }
}

fn push_doc_suffix(out: &mut String, card: &ValueCard) {
    match &card.doc {
        Some(doc) => {
            let _ = writeln!(out, " — {}", text(doc));
        }
        None => out.push('\n'),
    }
}

fn push_grid(out: &mut String, grid: &GridTable) {
    let _ = write!(out, "    | |");
    for column in &grid.columns {
        let _ = write!(out, " {} |", text(column));
    }
    out.push('\n');
    let _ = write!(out, "    |---|");
    for _ in &grid.columns {
        out.push_str("---|");
    }
    out.push('\n');
    for (label, cells) in &grid.rows {
        let _ = write!(out, "    | {} |", text(label));
        for cell in cells {
            let _ = write!(out, " {} |", text(cell));
        }
        out.push('\n');
    }
}

/// Free text as inline Markdown that renders as exactly that text.
///
/// Line breaks collapse to spaces, so text never starts a line and cannot
/// open a heading, list item, quote or table row; every character with an
/// inline meaning (emphasis, links, code, HTML, entities, table cells) is
/// backslash-escaped.
fn text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for word in raw.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        for ch in word.chars() {
            if matches!(
                ch,
                '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '|' | '~' | '#' | '!'
            ) {
                out.push('\\');
            }
            out.push(ch);
        }
    }
    escape_leading_block_marker(out)
}

/// Escape a block marker at the start of `text`: an item written after a
/// list marker (`- {text}`) would otherwise open a nested list item or a
/// thematic break. Only a marker followed by a space or the end counts, so
/// `-3 m` stays as it is.
fn escape_leading_block_marker(text: String) -> String {
    let marker_ends = |rest: &str| rest.is_empty() || rest.starts_with(' ');
    // `- `, `+ `, and a `---`-style thematic break (spaces allowed).
    if let Some(rest) = text.strip_prefix(['-', '+']) {
        let thematic = text.starts_with('-')
            && text.chars().filter(|ch| *ch == '-').count() >= 3
            && text.chars().all(|ch| ch == '-' || ch == ' ');
        if marker_ends(rest) || thematic {
            return format!("\\{text}");
        }
    }
    // `1. ` and `1) `: an ordered-list marker is at most nine digits.
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let (number, rest) = text.split_at(digits);
        if let Some(after) = rest.strip_prefix(['.', ')'])
            && marker_ends(after)
        {
            return format!("{number}\\{rest}");
        }
    }
    text
}

/// Text as one inline code span: delimited by a backtick run longer than any
/// inside it, padded when it starts or ends with a backtick, and on one line.
fn code(raw: &str) -> String {
    let content = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let longest = content
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.saturating_add(1));
    let pad = if content.starts_with('`') || content.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{content}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_collapses_lines_and_escapes_inline_syntax() {
        assert_eq!(
            text("ok\n## Checks\n- PASS `x` <!-- a|b & *c*"),
            "ok \\#\\# Checks - PASS \\`x\\` \\<\\!-- a\\|b \\& \\*c\\*"
        );
        assert_eq!(text("3.5 km/s"), "3.5 km/s");
    }

    #[test]
    fn text_never_opens_a_list_item_or_thematic_break() {
        assert_eq!(text("- PASS everything"), "\\- PASS everything");
        assert_eq!(text("+ x"), "\\+ x");
        assert_eq!(text("-"), "\\-");
        assert_eq!(text("---"), "\\---");
        assert_eq!(text("- - -"), "\\- - -");
        assert_eq!(text("1. x"), "1\\. x");
        assert_eq!(text("12) x"), "12\\) x");
        assert_eq!(text("-3 m"), "-3 m");
        assert_eq!(text("1.5 m"), "1.5 m");
        assert_eq!(text("2026-01-01T00:00:00Z"), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn code_spans_survive_backticks_and_line_breaks() {
        assert_eq!(code("speed"), "`speed`");
        assert_eq!(code("a`b"), "``a`b``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(code("a\nb"), "`a b`");
    }
}
