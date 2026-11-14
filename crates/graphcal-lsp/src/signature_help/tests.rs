use crate::analysis_pipeline::{run_analysis, test_plugin_host};

use tower_lsp::lsp_types::Url;

#[test]
fn signature_help_resolves_alias_qualified_extern_calls() {
    let uri = Url::parse("file:///extern-signature-help.gcl").unwrap();
    let declarations = concat!(
        "import plugin \"graphcal:demo\" as demo {\n",
        "    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;\n",
        "}\n",
    );
    let analysis = run_analysis(
        &uri,
        &format!("{declarations}node y: Dimensionless = 1.0;\n"),
        &[],
        test_plugin_host(),
    );
    let active = |call: &str| {
        let source = format!("{declarations}node y: Dimensionless = {call}");
        crate::signature_help::signature_help(&analysis, &source, source.len())
            .map(|help| (help.signatures[0].label.clone(), help.active_parameter))
    };
    let label = "fn demo::lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D";
    assert_eq!(active("demo::lerp("), Some((label.to_string(), Some(0))));
    assert_eq!(
        active("demo::lerp(1.0, 2.0, "),
        Some((label.to_string(), Some(2)))
    );
    // The extern leaf alone, a wrong alias, or a dotted owner is no match.
    assert_eq!(active("lerp("), None);
    assert_eq!(active("other::lerp("), None);
    assert_eq!(active("x.demo::lerp("), None);
    // Bare builtins still resolve.
    assert_eq!(
        active("sqrt(").map(|(label, _)| label),
        Some("fn sqrt<D: Dim>(x: D) -> D^(1/2)".to_string())
    );
}
