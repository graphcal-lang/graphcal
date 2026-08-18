//! End-to-end auto-report assembly tests: source → evaluation → document →
//! HTML/Markdown, mirroring how the CLI shell drives the library.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_eval::eval::compile_and_eval_from_project;
use graphcal_eval::loader::LoadedProject;
use graphcal_report::plot_page::VegaScriptSource;
use graphcal_report::report_html::render_report_html;
use graphcal_report::report_ir::{
    Provenance, ReportDocument, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use graphcal_report::report_markdown::render_report_markdown;

const DELTA_V: &str = "\
/// Dry mass of the stage without propellant.
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
/// Specific impulse of the qualified engine.
param isp: Time = 320.0 s;

const node g0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
/// Achievable delta-v at the current fuel load.
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio) -> km/s;

assert delta_v_positive = @delta_v > 0.0 m/s;

pub index Steps = { S1, S2 };
node per_step: Velocity[Steps] = for s: Steps { @delta_v / 2.0 };

plot dv_plot = {
    mark: line,
    encode: {
        x: for s: Steps { 1.0 },
        y: for s: Steps { @per_step[s] -> km/s },
    },
    title: \"Delta-v per step\",
};
";

fn build_document(source: &str) -> ReportDocument {
    let project = LoadedProject::from_source(source, "deltav.gcl").unwrap();
    let result = compile_and_eval_from_project(&project, &HashMap::new()).unwrap();
    let docs = collect_doc_captions(project.root_file().ast());
    build_report(ReportInputs {
        title: "deltav",
        result: &result,
        docs: &docs,
        provenance: Provenance {
            compiler_version: "0.0.0-test".to_string(),
            sources: vec![SourceDigest {
                name: "deltav.gcl".to_string(),
                sha256: "0".repeat(64),
            }],
            baseline_params: vec![("isp".to_string(), "320 s".to_string())],
            repro_command: "graphcal eval deltav.gcl".to_string(),
        },
    })
    .unwrap()
}

#[test]
fn document_derives_sections_from_the_model_alone() {
    let document = build_document(DELTA_V);
    assert_eq!(
        document
            .params
            .iter()
            .map(|card| card.name.as_str())
            .collect::<Vec<_>>(),
        ["dry_mass", "fuel_mass", "isp"]
    );
    assert!(
        document
            .values
            .iter()
            .map(|card| card.name.as_str())
            .any(|name| name == "delta_v"),
        "nodes must appear as value cards"
    );
    assert_eq!(
        document.params[2].doc.as_deref(),
        Some("Specific impulse of the qualified engine.")
    );
    assert_eq!(document.figures.len(), 1);
    assert_eq!(document.checks.len(), 1);
}

#[test]
fn html_report_is_self_contained_and_deterministic() {
    let document = build_document(DELTA_V);
    let html = render_report_html(&document, VegaScriptSource::Inline);
    let again = render_report_html(&build_document(DELTA_V), VegaScriptSource::Inline);
    assert_eq!(html, again, "report output must be byte-deterministic");

    assert!(html.contains("Specific impulse of the qualified engine."));
    assert!(html.contains("data-decl=\"delta_v\""));
    assert!(html.contains("data-check=\"delta_v_positive\""));
    assert!(html.contains(">PASS<"));
    assert!(html.contains("vegaEmbed"));
    assert!(!html.contains("cdn.jsdelivr.net"));
    assert!(html.contains("sha256:"));
    assert!(html.contains("graphcal eval deltav.gcl"));
    // Display provenance (`-> km/s`) drives the rendered unit.
    assert!(html.contains("km/s"));
}

#[test]
fn vega_scripts_are_omitted_without_figures() {
    let document = build_document("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;");
    let html = render_report_html(&document, VegaScriptSource::Inline);
    assert!(!html.contains("vegaEmbed"));
    assert!(html.contains("data-decl=\"y\""));
}

#[test]
fn markdown_report_carries_values_checks_and_provenance() {
    let document = build_document(DELTA_V);
    let markdown = render_report_markdown(&document);
    assert!(markdown.contains("# deltav"));
    assert!(markdown.contains("## Inputs"));
    assert!(markdown.contains("- `isp` = 320 s — Specific impulse of the qualified engine."));
    assert!(markdown.contains("- PASS `delta_v_positive`"));
    assert!(markdown.contains("- `dv_plot`"));
    assert!(markdown.contains("sha256:"));
}

#[test]
fn doc_captions_are_html_escaped() {
    let source = "\
/// A <script>alert(1)</script> caption.
param x: Dimensionless = 1.0;
node y: Dimensionless = @x;
";
    let document = build_document(source);
    let html = render_report_html(&document, VegaScriptSource::Inline);
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
}

#[test]
fn failed_values_render_as_error_chips_not_a_broken_page() {
    let source = "\
param x: Dimensionless = 0.0;
node bad: Dimensionless = 1.0 / @x;
node good: Dimensionless = 2.0;
assert bad_is_finite = @bad < 10.0;
";
    let project = LoadedProject::from_source(source, "bad.gcl").unwrap();
    let result = compile_and_eval_from_project(&project, &HashMap::new()).unwrap();
    let docs = collect_doc_captions(project.root_file().ast());
    let document = build_report(ReportInputs {
        title: "bad",
        result: &result,
        docs: &docs,
        provenance: Provenance {
            compiler_version: "0.0.0-test".to_string(),
            sources: Vec::new(),
            baseline_params: Vec::new(),
            repro_command: "graphcal eval bad.gcl".to_string(),
        },
    })
    .unwrap();
    let html = render_report_html(&document, VegaScriptSource::Inline);
    assert!(html.contains("error-chip"));
    assert!(
        html.contains("data-decl=\"good\""),
        "healthy values must still render"
    );
}
