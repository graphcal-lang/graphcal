//! Self-contained HTML rendering of one [`ReportDocument`].
//!
//! The page is deterministic (no timestamps, no randomness), works from
//! `file://` paths (assets inlined), renders values/checks without
//! JavaScript, and carries stable `data-*` anchors so the hydration layer
//! can patch values in place without re-rendering the document structure.

use std::fmt::Write;

use crate::escape::{escape_json_for_script, html_escape};
use crate::plot_page::VegaScriptSource;
use crate::report_ir::{CardBody, CheckStatus, ReportDocument, ValueCard};
use crate::value_display::{GridTable, ValueBody};

const REPORT_CSS: &str = include_str!("report_style.css");

/// Render the complete standalone report page.
#[must_use]
pub fn render_report_html(document: &ReportDocument, scripts: VegaScriptSource) -> String {
    let mut body = String::new();
    let title = html_escape(&document.title);
    let _ = writeln!(body, "<header><h1>{title}</h1></header>");

    if !document.params.is_empty() {
        body.push_str("<section id=\"inputs\">\n<h2>Inputs</h2>\n<div class=\"cards\">\n");
        for card in &document.params {
            push_value_card(&mut body, card);
        }
        body.push_str("</div>\n</section>\n");
    }

    if !document.values.is_empty() {
        body.push_str("<section id=\"values\">\n<h2>Values</h2>\n<div class=\"cards\">\n");
        for card in &document.values {
            push_value_card(&mut body, card);
        }
        body.push_str("</div>\n</section>\n");
    }

    if !document.figures.is_empty() || !document.plot_errors.is_empty() {
        body.push_str("<section id=\"plots\">\n<h2>Plots</h2>\n");
        body.push_str(
            "<noscript><p class=\"notice\">Charts require JavaScript; values and checks above are complete without it.</p></noscript>\n",
        );
        for error in &document.plot_errors {
            let _ = writeln!(
                body,
                "<p class=\"error-chip\">plot <code>{}</code> not rendered: {}</p>",
                html_escape(&error.name),
                html_escape(&error.message)
            );
        }
        for (index, card) in document.figures.iter().enumerate() {
            let div_id = format!("graphcal-figure-{index}");
            let name = html_escape(&card.figure.name);
            let _ = write!(
                body,
                "<figure class=\"plot\" data-figure=\"{name}\">\n<figcaption><span class=\"figure-name\">{name}</span>"
            );
            if let Some(doc) = &card.doc {
                let _ = write!(body, " — {}", html_escape(doc));
            }
            body.push_str("</figcaption>\n");
            let _ = writeln!(body, "<div id=\"{div_id}\"></div>");
            let spec_json = escape_json_for_script(&card.figure.spec.to_string());
            let _ = writeln!(
                body,
                "<script>vegaEmbed('#{div_id}', {spec_json}, {{\"actions\": false}}).catch(console.error);</script>"
            );
            body.push_str("</figure>\n");
        }
        body.push_str("</section>\n");
    }

    if !document.checks.is_empty() {
        body.push_str("<section id=\"checks\">\n<h2>Checks</h2>\n<ul class=\"checks\">\n");
        for check in &document.checks {
            let (class, label) = match check.status {
                CheckStatus::Pass => ("pass", "PASS"),
                CheckStatus::Fail => ("fail", "FAIL"),
                CheckStatus::Error => ("error", "ERROR"),
            };
            let name = html_escape(&check.name);
            let _ = write!(
                body,
                "<li class=\"check check--{class}\" data-check=\"{name}\"><span class=\"badge\">{label}</span> <code>{name}</code>"
            );
            if let Some(message) = &check.message {
                let _ = write!(
                    body,
                    " <span class=\"check-message\">{}</span>",
                    html_escape(message)
                );
            }
            if !check.affected.is_empty() {
                let affected = check
                    .affected
                    .iter()
                    .map(|name| html_escape(name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = write!(
                    body,
                    " <span class=\"check-affected\">affected: {affected}</span>"
                );
            }
            body.push_str("</li>\n");
        }
        body.push_str("</ul>\n</section>\n");
    }

    push_provenance(&mut body, document);

    let vega_scripts = if document.figures.is_empty() {
        String::new()
    } else {
        crate::plot_page::vega_script_tags(scripts)
    };

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n{vega_scripts}\n<style>{REPORT_CSS}</style>\n</head>\n<body>\n<main>\n{body}</main>\n</body>\n</html>\n"
    )
}

fn push_value_card(out: &mut String, card: &ValueCard) {
    let name = html_escape(&card.name);
    let kind = match card.kind {
        graphcal_eval::eval::DeclType::Const => "const",
        graphcal_eval::eval::DeclType::Param => "param",
        graphcal_eval::eval::DeclType::Node => "node",
    };
    let _ = write!(
        out,
        "<article class=\"card card--{kind}\" data-decl=\"{name}\">\n<h3 class=\"card-name\"><code>{name}</code> <span class=\"card-kind\">{kind}</span></h3>\n"
    );
    if let Some(doc) = &card.doc {
        let _ = writeln!(out, "<p class=\"card-doc\">{}</p>", html_escape(doc));
    }
    match &card.body {
        CardBody::Value(body) => push_value_body(out, body),
        CardBody::Error { message } => {
            let _ = writeln!(
                out,
                "<p class=\"error-chip\" data-role=\"value\">ERROR: {}</p>",
                html_escape(message)
            );
        }
    }
    out.push_str("</article>\n");
}

fn push_value_body(out: &mut String, body: &ValueBody) {
    match body {
        ValueBody::Scalar(display) => {
            let _ = writeln!(
                out,
                "<p class=\"card-value\" data-role=\"value\">{}</p>",
                html_escape(display)
            );
        }
        ValueBody::Entries(entries) => {
            out.push_str("<table class=\"entries\" data-role=\"value\"><tbody>\n");
            for (label, display) in entries {
                let _ = writeln!(
                    out,
                    "<tr><th scope=\"row\"><code>{}</code></th><td>{}</td></tr>",
                    html_escape(label),
                    html_escape(display)
                );
            }
            out.push_str("</tbody></table>\n");
        }
        ValueBody::Grid(grid) => push_grid(out, grid, "data-role=\"value\""),
        ValueBody::Slices(slices) => {
            out.push_str("<div class=\"slices\" data-role=\"value\">\n");
            for (label, grid) in slices {
                let _ = writeln!(
                    out,
                    "<h4 class=\"slice-label\">[{}]</h4>",
                    html_escape(label)
                );
                push_grid(out, grid, "");
            }
            out.push_str("</div>\n");
        }
    }
}

fn push_grid(out: &mut String, grid: &GridTable, attrs: &str) {
    let sep = if attrs.is_empty() { "" } else { " " };
    let _ = write!(
        out,
        "<table class=\"grid\"{sep}{attrs}>\n<thead><tr><th></th>"
    );
    for column in &grid.columns {
        let _ = write!(out, "<th scope=\"col\">{}</th>", html_escape(column));
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for (label, cells) in &grid.rows {
        let _ = write!(out, "<tr><th scope=\"row\">{}</th>", html_escape(label));
        for cell in cells {
            let _ = write!(out, "<td>{}</td>", html_escape(cell));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

fn push_provenance(out: &mut String, document: &ReportDocument) {
    let provenance = &document.provenance;
    out.push_str("<footer id=\"provenance\">\n<h2>Provenance</h2>\n<dl class=\"provenance\">\n");
    let _ = writeln!(
        out,
        "<dt>Compiler</dt><dd>Graphcal {}</dd>",
        html_escape(&provenance.compiler_version)
    );
    out.push_str("<dt>Sources</dt><dd><ul class=\"sources\">\n");
    for source in &provenance.sources {
        let _ = writeln!(
            out,
            "<li><code>{}</code> <span class=\"sha\">sha256:{}</span></li>",
            html_escape(&source.name),
            html_escape(&source.sha256)
        );
    }
    out.push_str("</ul></dd>\n");
    if !provenance.baseline_params.is_empty() {
        out.push_str("<dt>Baseline</dt><dd><ul class=\"baseline\">\n");
        for (name, value) in &provenance.baseline_params {
            let _ = writeln!(
                out,
                "<li><code>{}</code> = {}</li>",
                html_escape(name),
                html_escape(value)
            );
        }
        out.push_str("</ul></dd>\n");
    }
    let _ = writeln!(
        out,
        "<dt>Reproduce</dt><dd><code class=\"repro\">{}</code></dd>",
        html_escape(&provenance.repro_command)
    );
    out.push_str("</dl>\n</footer>\n");
}
