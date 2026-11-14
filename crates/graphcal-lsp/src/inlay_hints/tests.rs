use tower_lsp::lsp_types::Range;

use crate::analysis_pipeline::{run_analysis, test_plugin_host};

use crate::analysis_pipeline::untitled_uri;

#[test]
fn inlay_hints_use_constructor_call_syntax_for_algebraic_values() {
    let text = include_str!("../../../../tests/fixtures/valid/tagged_union.gcl");
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );

    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .expect("expected inlay hints for tagged_union fixture");
    let labels: Vec<String> = hints
        .into_iter()
        .filter_map(|hint| match hint.label {
            tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
            tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
        })
        .collect();

    assert!(
        labels
            .iter()
            .any(|label| label.contains("= LowThrust(thrust: 0.5 [N], duration: 3600 [s])")),
        "expected constructor-call hint for `maneuver`, got: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("= TransferResult(dv1: 100 [m/s], dv2: 200 [m/s])")),
        "expected constructor-call hint for `transfer`, got: {labels:?}"
    );
    assert!(
        !labels
            .iter()
            .any(|label| label.contains("LowThrust {") || label.contains("TransferResult {")),
        "constructor hints must not use brace syntax: {labels:?}"
    );
}

#[test]
fn inlay_hints_render_coordinate_keys_with_axis_display_unit() {
    let text = "index TimeStep = range(1.0 h, 3.0 h, step: 1.0 h);\n\
                    node samples: Time[TimeStep] = for t: TimeStep { coord(t) };\n\
                    node peak: Key<TimeStep> = argmax(@samples);\n\
                    node copies: Key<TimeStep>[Fin(2)] = for i: Fin(2) { @peak };\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );

    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .expect("expected inlay hints for coordinate keys");
    let labels = hints
        .into_iter()
        .filter_map(|hint| match hint.label {
            tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
            tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
        })
        .collect::<Vec<_>>();

    assert!(
        labels.iter().any(|label| label == " = 3 [h]"),
        "expected axis display unit on scalar coordinate key: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label == " = { #0: 3 [h], #1: 3 [h] }"),
        "expected axis display unit on indexed coordinate keys: {labels:?}"
    );
}

#[test]
fn inlay_hints_keep_reordered_dynamic_dag_presentations() {
    let text = "index Rate = { Two, Three };\n\
                    index Order = { First, Second };\n\
                    dag units {\n\
                        param rate: Dimensionless;\n\
                        unit DynamicM: Length = (@rate) m;\n\
                        pub node distance: Length = 1.0 DynamicM;\n\
                    }\n\
                    node rates: Dimensionless[Rate] = {\n\
                        Rate#Two: 2.0,\n\
                        Rate#Three: 3.0,\n\
                    };\n\
                    node values: Length[Rate] = for rate: Rate {\n\
                        @units(rate: @rates[rate])::distance\n\
                    };\n\
                    node reversed: Length[Order] = {\n\
                        Order#First: @values[Rate#Three],\n\
                        Order#Second: @values[Rate#Two],\n\
                    };\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );

    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .expect("expected inlay hints for reordered DAG output");
    let labels = hints
        .into_iter()
        .filter_map(|hint| match hint.label {
            tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
            tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
        })
        .collect::<Vec<_>>();

    assert!(
        labels
            .iter()
            .any(|label| label == " = { First: 1 [DynamicM], Second: 1 [DynamicM] }"),
        "expected exact invocation-aware presentation hint: {labels:?}"
    );
}

#[test]
fn inlay_hints_use_declared_scale_datetime_extractors() {
    let text = "node tt: Datetime<TT> = epoch<TT>(\"2024-01-01T00:00:30\");\n\
                    node utc: Datetime = to_utc(@tt);\n\
                    node tt_hour: Int = hour(@tt);\n\
                    node utc_hour: Int = hour(@utc);\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );

    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .expect("expected datetime inlay hints");
    let label_on_line = |line| {
        hints
            .iter()
            .find(|hint| hint.position.line == line)
            .and_then(|hint| match &hint.label {
                tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label.as_str()),
                tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
            })
    };
    assert_eq!(label_on_line(2), Some(" = 0"));
    assert_eq!(label_on_line(3), Some(" = 23"));
}

#[test]
fn inlay_hints_evaluate_valid_datetime_domain_constraints() {
    let text = "node event: Datetime<TT>(\
                    min: epoch<TT>(\"2024-01-01T00:00:00\"), \
                    max: epoch<TT>(\"2024-12-31T23:59:59\")) = \
                    epoch<TT>(\"2024-06-01T12:00:00\");\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );
    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .expect("expected datetime domain inlay hint");
    assert!(hints.iter().any(|hint| {
        matches!(
            &hint.label,
            tower_lsp::lsp_types::InlayHintLabel::String(label)
                if label.contains("2024-06-01T12:00:00 TT")
        )
    }));
}

/// LSP-10 (#2253): only values some inlay hint displays are formatted;
/// include instances the evaluation reports are skipped.
#[test]
fn only_displayed_declarations_have_formatted_values() {
    let text = "param p: Length = 200.0 km;\n\
                    dag inner {\n\
                        param p: Length;\n\
                        pub node twice: Length = 2.0 * @p;\n\
                    }\n\
                    include inner(p: @p)::{ twice };\n\
                    node total: Length = @twice;\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
    let shown = analysis.symbol_table.inlay_hint_declarations();
    assert!(!analysis.eval_values.is_empty());
    assert!(
        analysis
            .eval_values
            .keys()
            .all(|identity| shown.contains(identity)),
        "{:?}",
        analysis.eval_values
    );
}

#[test]
fn inlay_hints_show_no_instance_values_on_dag_body_declarations() {
    // An inline `dag` body is a template: each include instantiates it
    // with its own bindings, so its declarations have no value of their
    // own — even when one selective include's private scope is displayed
    // under the DAG's own name.
    let text = "param p: Length = 200.0 km;\n\
                    dag inner {\n\
                        param p: Length;\n\
                        pub node twice: Length = 2.0 * @p;\n\
                    }\n\
                    include inner(p: @p)::{ twice };\n\
                    node total: Length = @twice;\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics
    );
    let hints = crate::inlay_hints::inlay_hints(
        &analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .unwrap_or_default();
    let labels_on_line = |line| {
        hints
            .iter()
            .filter(|hint| hint.position.line == line)
            .filter_map(|hint| match &hint.label {
                tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label.as_str()),
                tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(labels_on_line(0), [" = 200 [km]"]);
    // The DAG requires `p`, so like a library file it shows types only.
    assert_eq!(labels_on_line(2), [": Length"], "DAG param `p`");
    assert_eq!(labels_on_line(3), [": Length"], "DAG node `twice`");
    assert_eq!(labels_on_line(6), [" = 400000 [m]"]);
}

/// Inlay-hint labels by line, in position order.
fn hint_labels(analysis: &crate::analysis::AnalysisResult) -> Vec<(u32, String)> {
    crate::inlay_hints::inlay_hints(
        analysis,
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
        ),
    )
    .unwrap_or_default()
    .into_iter()
    .filter_map(|hint| match hint.label {
        tower_lsp::lsp_types::InlayHintLabel::String(label) => Some((hint.position.line, label)),
        tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
    })
    .collect()
}

/// `body` written inside `dag inner { ... }` after `prefix` gets, line for
/// line, the inlay hints and runtime diagnostics it gets as a file.
/// Returns the number of runtime diagnostics, so callers can check the
/// comparison is not vacuous.
fn assert_inline_dag_matches_file(prefix: &str, body: &str) -> usize {
    let standalone = run_analysis(&untitled_uri(), body, &[], test_plugin_host());
    let prefix_lines = u32::try_from(prefix.lines().count()).unwrap() + 1;
    assert!(body.ends_with('\n'));
    let inline_text = format!("{prefix}dag inner {{\n{body}}}\n");
    let inline = run_analysis(&untitled_uri(), &inline_text, &[], test_plugin_host());

    let body_lines = u32::try_from(body.lines().count()).unwrap();
    let in_body = |line: u32| (prefix_lines..prefix_lines + body_lines).contains(&line);
    let inline_hints = hint_labels(&inline)
        .into_iter()
        .filter(|(line, _)| in_body(*line))
        .map(|(line, label)| (line - prefix_lines, label))
        .collect::<Vec<_>>();
    assert!(!inline_hints.is_empty(), "{inline_text}");
    assert_eq!(inline_hints, hint_labels(&standalone), "{inline_text}");

    let runtime = |analysis: &crate::analysis::AnalysisResult, shift: u32| {
        analysis.diagnostics[&untitled_uri()]
            .iter()
            .filter(|diagnostic| {
                matches!(
                    &diagnostic.code,
                    Some(tower_lsp::lsp_types::NumberOrString::String(code)) if code == "graphcal::E001"
                )
            })
            .map(|diagnostic| {
                (
                    diagnostic.range.start.line - shift,
                    diagnostic.message.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let failures = runtime(&standalone, 0);
    assert_eq!(runtime(&inline, prefix_lines), failures, "{inline_text}");
    failures.len()
}

#[test]
fn inline_dags_get_the_inlay_hints_of_a_file() {
    let bodies = [
        // Every param has a default: values.
        "param p: Length = 100.0 km;\npub node twice: Length = 2.0 * @p;\n",
        // A required param: types only, as in a library file.
        "param p: Length = 100.0 km;\nparam q: Length;\npub node total: Length = @p + @q;\n",
        // A runtime failure on the defaults lands on its declaration.
        "param d: Dimensionless = 0.0;\npub node r: Dimensionless = 1.0 / @d;\n",
        // An include inside the DAG: the DAG's own declarations only.
        "dag leaf {\n    param x: Length = 1.0 m;\n    pub node y: Length = 3.0 * @x;\n}\ninclude leaf(x: 2.0 m)::{ y };\npub node z: Length = @y;\n",
    ];
    let failures = bodies
        .iter()
        .map(|body| {
            let in_file = assert_inline_dag_matches_file("", body);
            // The root being a library does not change what the body shows.
            let in_library = assert_inline_dag_matches_file("param root_input: Length;\n", body);
            assert_eq!(in_file, in_library);
            in_file
        })
        .collect::<Vec<_>>();
    assert_eq!(failures, [0, 0, 1, 0]);
}

#[test]
fn inline_dag_defaults_ignore_include_bindings() {
    let text = "dag inner {\n\
                        param p: Length = 100.0 km;\n\
                        pub node twice: Length = 2.0 * @p;\n\
                    }\n\
                    include inner(p: 5.0 km)::{ twice };\n\
                    node total: Length = @twice;\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
    assert_eq!(
        hint_labels(&analysis),
        [
            (1, " = 100 [km]".to_string()),
            (2, " = 200000 [m]".to_string()),
            (5, " = 10000 [m]".to_string()),
        ]
    );
}

#[test]
fn nested_inline_dags_get_their_own_defaults() {
    let text = "dag shell {\n\
                        param a: Length = 1.0 m;\n\
                        dag inner {\n\
                            param b: Length = 2.0 m;\n\
                            pub node c: Length = 3.0 * @b;\n\
                        }\n\
                        pub node d: Length = 4.0 * @a;\n\
                    }\n";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
    assert_eq!(
        hint_labels(&analysis),
        [
            (1, " = 1 [m]".to_string()),
            (3, " = 2 [m]".to_string()),
            (4, " = 6 [m]".to_string()),
            (6, " = 4 [m]".to_string()),
        ]
    );
}
