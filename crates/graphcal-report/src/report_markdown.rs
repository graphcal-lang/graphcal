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
    let _ = writeln!(out, "# {}", document.title);

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
                    let _ = writeln!(out, "- `{}` — {}", card.figure.name, doc);
                }
                None => {
                    let _ = writeln!(out, "- `{}`", card.figure.name);
                }
            }
        }
        for error in &document.plot_errors {
            let _ = writeln!(out, "- `{}` — ERROR: {}", error.name, error.message);
        }
    }

    if !document.checks.is_empty() {
        out.push_str("\n## Checks\n\n");
        for check in &document.checks {
            let label = match check.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Fail => "FAIL",
                CheckStatus::Error => "ERROR",
            };
            let _ = write!(out, "- {label} `{}`", check.name);
            if let Some(message) = &check.message {
                let _ = write!(out, " — {message}");
            }
            if !check.affected.is_empty() {
                let _ = write!(out, " (affected: {})", check.affected.join(", "));
            }
            out.push('\n');
        }
    }

    let provenance = &document.provenance;
    out.push_str("\n## Provenance\n\n");
    let _ = writeln!(out, "- Compiler: Graphcal {}", provenance.compiler_version);
    for source in &provenance.sources {
        let _ = writeln!(out, "- Source: `{}` sha256:{}", source.name, source.sha256);
    }
    for (name, value) in &provenance.baseline_params {
        let _ = writeln!(out, "- Baseline: `{name}` = {value}");
    }
    let _ = writeln!(out, "- Reproduce: `{}`", provenance.repro_command);
    out
}

fn push_card(out: &mut String, card: &ValueCard) {
    match &card.body {
        CardBody::Value(ValueBody::Scalar(display)) => {
            let _ = write!(out, "- `{}` = {display}", card.name);
            push_doc_suffix(out, card);
        }
        CardBody::Value(ValueBody::Entries(entries)) => {
            let _ = write!(out, "- `{}`", card.name);
            push_doc_suffix(out, card);
            for (label, display) in entries {
                let _ = writeln!(out, "    - `{label}` = {display}");
            }
        }
        CardBody::Value(ValueBody::Grid(grid)) => {
            let _ = write!(out, "- `{}`", card.name);
            push_doc_suffix(out, card);
            out.push('\n');
            push_grid(out, grid);
        }
        CardBody::Value(ValueBody::Slices(slices)) => {
            let _ = write!(out, "- `{}`", card.name);
            push_doc_suffix(out, card);
            for (label, grid) in slices {
                let _ = writeln!(out, "\n    [{label}]\n");
                push_grid(out, grid);
            }
        }
        CardBody::Error { message } => {
            let _ = write!(out, "- `{}` = ERROR: {message}", card.name);
            push_doc_suffix(out, card);
        }
    }
}

fn push_doc_suffix(out: &mut String, card: &ValueCard) {
    match &card.doc {
        Some(doc) => {
            let _ = writeln!(out, " — {}", doc.replace('\n', " "));
        }
        None => out.push('\n'),
    }
}

fn push_grid(out: &mut String, grid: &GridTable) {
    let _ = write!(out, "    | |");
    for column in &grid.columns {
        let _ = write!(out, " {column} |");
    }
    out.push('\n');
    let _ = write!(out, "    |---|");
    for _ in &grid.columns {
        out.push_str("---|");
    }
    out.push('\n');
    for (label, cells) in &grid.rows {
        let _ = write!(out, "    | {label} |");
        for cell in cells {
            let _ = write!(out, " {cell} |");
        }
        out.push('\n');
    }
}
