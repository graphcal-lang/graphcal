use super::*;
use crate::presentation_evidence::{PendingDisplayUnit, PresentationFailure, PresentationInstance};

fn labels(value: &Value) -> Vec<Option<String>> {
    match value {
        Value::Quantity { display_unit, .. } | Value::Complex { display_unit, .. } => {
            vec![display_unit.as_ref().map(|unit| unit.label.clone())]
        }
        Value::Indexed { entries, .. } => entries.values().flat_map(labels).collect(),
        Value::Struct { fields, .. } => fields.values().flat_map(labels).collect(),
        other => panic!("expected presented value, got {other:?}"),
    }
}

#[test]
fn branches_locals_projections_and_repeated_call_sites_keep_selected_evidence() {
    let source = r"
index Mode = { Small, Large };
type Reading { Reading(value: Length), }
dag worker {
    param large: Bool;
    pub node out: Length = if @large { 2000.0 m -> km } else { 3.0 m -> cm };
}
node readings: Reading[Fin(2)] = for i: Fin(2) {
    Reading(value: @worker(large: to_int(i) == 1)::out)
};
node selected: Length = match @readings[1] { Reading(value: local) => local };
node nested: Length = match Mode#Large {
    Mode#Small => 4.0 m -> mm,
    Mode#Large => match @readings[0] { Reading(value: local) => local },
};
node mapped: Length[Mode] = { Mode#Small: @selected, Mode#Large: @nested };
node projected: Length = @mapped[Mode#Large];
";
    let result = compile_and_eval(source).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(
        labels(&find_entry(&result, "readings")),
        [Some("cm".into()), Some("km".into())]
    );
    assert_eq!(
        labels(&find_entry(&result, "selected")),
        [Some("km".into())]
    );
    assert_eq!(labels(&find_entry(&result, "nested")), [Some("cm".into())]);
    assert_eq!(
        labels(&find_entry(&result, "projected")),
        [Some("cm".into())]
    );
    assert_quantity_value(&result, "selected", 2000.0);
}

#[test]
fn scan_and_unfold_transfer_each_step_not_the_initial_presentation() {
    let result = compile_and_eval(r"
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node source: Length[Fin(2)] = table[Fin(2)] { 1.0 m -> cm; 2.0 m -> km; };
node scanned: Length[Fin(2)] = scan(@source, 0.0 m, |acc, val| val);
node unfolded: Length[Step] = unfold(Step, 1.0 m -> mm, |prev, prev_t, t| if coord(t) == 1.0 s { prev -> cm } else { prev -> km });
").unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(
        labels(&find_entry(&result, "scanned")),
        [Some("cm".into()), Some("km".into())]
    );
    assert_eq!(
        labels(&find_entry(&result, "unfolded")),
        [Some("mm".into()), Some("cm".into()), Some("km".into())]
    );
}

#[test]
fn literal_constant_and_conversion_label_overflow_preserve_si() {
    let literal = include_str!("display-label-overflow.gcl");
    let constant = include_str!("constant-display-label-overflow.gcl");
    let converted = literal.replace("= 1.0 identity_scale", "= 1.0 -> identity_scale");
    for source in [literal, constant, &converted] {
        let result = compile_and_eval(source)
            .expect("display formatting must not reject a checked constant or value");
        assert!(
            result.all.iter().all(|(_, value, _)| value.is_ok()),
            "label formatting must not erase SI: {result:?}"
        );
        assert_eq!(
            find_entry(&result, "value").si_value().unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(result.presentation_diagnostics.len(), 1);
        assert!(matches!(
            result.presentation_diagnostics[0].detail.failure,
            PresentationFailure::Formatting {
                error:
                    graphcal_compiler::registry::format::CanonicalUnitFormatError::ExponentOverflow,
                ..
            }
        ));
        assert!(result.has_errors());
    }
    let valid = compile_and_eval("const unit identity_scale: Length/Length = 1.0 m/m; node value: Dimensionless = 1.0 identity_scale^(1/2147483647);").unwrap();
    assert!(!valid.has_errors());
    assert_eq!(
        labels(&find_entry(&valid, "value")),
        [Some("identity_scale^(1/2147483647)".into())]
    );
    let invalid_scale = compile_and_eval("param rate: Dimensionless = 0.0; unit bad: Length = (@rate) m; node value: Length = 1.0 bad;").unwrap();
    assert!(invalid_scale.nodes[0].1.is_err());
    assert!(invalid_scale.presentation_diagnostics.is_empty());
}

#[test]
fn conversion_only_failed_dependency_does_not_poison_si_values() {
    let result = compile_and_eval(include_str!("presentation_failed_dependency.gcl")).unwrap();
    assert!(matches!(
        result.nodes[0].1,
        Err(NodeError::EvalFailed { .. })
    ));
    for (position, name) in [(1, "chosen"), (2, "unchosen")] {
        let value = result.nodes[position]
            .1
            .as_ref()
            .expect("conversion-only dependency must not erase SI");
        assert_eq!(
            value.si_value().unwrap().to_bits(),
            1.0_f64.to_bits(),
            "{name}"
        );
    }
    assert_eq!(result.presentation_diagnostics.len(), 1);
    assert_eq!(
        result.presentation_diagnostics[0].declaration.as_str(),
        "chosen"
    );
}

#[test]
fn borrowed_evidence_projection_is_sparse_broadcast_and_copy_free() {
    use graphcal_compiler::syntax::{index_name::IndexEntryKey, type_name::FieldName};
    let field = FieldName::expect_valid("value");
    let unit = PresentationInstance::Unit {
        label: "m".into(),
        scale: graphcal_compiler::registry::unit::PositiveFiniteScale::new(1.0).unwrap(),
    };
    let zone = PresentationInstance::Timezone(
        graphcal_compiler::registry::time_zone::TimeZoneRegistry::bundled()
            .parse_iana_id("Asia/Tokyo")
            .unwrap(),
    );
    let tree = PresentationInstance::entries([(
        IndexEntryKey::position(0),
        PresentationInstance::fields([(field.clone(), unit.clone())]),
    )]);
    let ((), counts) = crate::pipeline_metrics::measure(|| {
        let selected = tree
            .project_indexes_ref(&[IndexEntryKey::position(0)])
            .unwrap();
        let PresentationInstance::Indexed { entries } = &tree else {
            panic!("indexed evidence");
        };
        assert!(std::ptr::eq(
            selected,
            entries.get(&IndexEntryKey::position(0)).unwrap()
        ));
        let PresentationInstance::Struct { fields } = selected else {
            panic!("struct evidence");
        };
        assert!(std::ptr::eq(
            selected.project_field_ref(&field).unwrap(),
            fields.get(&field).unwrap()
        ));
        assert!(
            selected
                .project_field_ref(&FieldName::expect_valid("missing"))
                .unwrap()
                .is_none()
        );
        assert!(
            tree.project_indexes_ref(&[IndexEntryKey::position(1)])
                .unwrap()
                .is_none()
        );
        assert!(
            selected
                .project_indexes_ref(&[IndexEntryKey::position(0)])
                .is_err()
        );
        assert!(unit.project_field_ref(&field).is_err());
        for scalar in [&unit, &zone, &PresentationInstance::None] {
            assert!(std::ptr::eq(
                scalar,
                scalar
                    .project_indexes_ref(&[IndexEntryKey::position(0), IndexEntryKey::position(2)])
                    .unwrap()
            ));
        }
    });
    assert_eq!(counts.presentation_evidence_copy_nodes, 0);
}

#[test]
fn scan_and_match_copy_only_selected_evidence_subtrees() {
    fn copies(source: &str) -> u64 {
        let loaded = crate::loader::LoadedProject::from_source(source, "copies.gcl").unwrap();
        let prepared = ProjectCompiler::new(&loaded).prepare().unwrap();
        let row = prepared.binding_builder().finish().unwrap();
        let (result, counts) =
            crate::pipeline_metrics::measure(|| prepared.evaluate(&row).unwrap());
        assert!(!result.has_errors(), "{result:?}");
        counts.presentation_evidence_copy_nodes
    }
    let scan = |count| {
        format!(
            "node output: Length[Fin({count})] = scan(for i: Fin({count}) {{ 1.0 m -> km }}, 0.0 m, |acc, val| val);"
        )
    };
    let matched = |count| {
        let fields = (0..count)
            .map(|i| format!("f{i}: Length"))
            .collect::<Vec<_>>()
            .join(", ");
        let values = (0..count)
            .map(|i| format!("f{i}: 1.0 km"))
            .collect::<Vec<_>>()
            .join(", ");
        let pattern = (0..count)
            .map(|i| format!("f{i}: v{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "type Product {{ Product({fields}), }} node output: Length = match Product({values}) {{ Product({pattern}) => v0 }};"
        )
    };
    for (small, large) in [(scan(4), scan(64)), (matched(4), matched(64))] {
        let small = copies(&small);
        let large = copies(&large);
        eprintln!("evidence copy controls: {small} -> {large}");
        assert!(
            small > 0 && large > small,
            "active nonvacuous copy observation"
        );
        assert!(
            large <= 20 * small,
            "projection must not clone all siblings per iteration: {small} -> {large}"
        );
    }
}

#[test]
fn display_dependencies_never_erase_si_or_schedule_unchosen_scales() {
    let result = compile_and_eval(
        r"
node broken: Dimensionless = 1.0 / 0.0;
unit bad: Length = (@broken) m;
node chosen: Length = 1.0 m -> bad;
node unchosen: Length = if true { 1.0 m -> m } else { 1.0 m -> bad };
node computational: Length = 1.0 bad;
node forward: Length = 8.0 m -> later;
unit later: Length = (@rate) m;
node rate: Dimensionless = 4.0;
node self_display: Length = 2.0 m -> own;
unit own: Length = (@self_display / 1.0 m) m;
",
    )
    .unwrap();
    assert_quantity_value(&result, "chosen", 1.0);
    assert_quantity_value(&result, "unchosen", 1.0);
    assert_quantity_value(&result, "forward", 8.0);
    assert_quantity_value(&result, "self_display", 2.0);
    assert!(
        result
            .nodes
            .iter()
            .find(|(name, _)| name == &scoped_name("broken"))
            .unwrap()
            .1
            .is_err()
    );
    assert!(matches!(
        result
            .nodes
            .iter()
            .find(|(name, _)| name == &scoped_name("computational"))
            .unwrap()
            .1,
        Err(NodeError::DependencyFailed { .. })
    ));
    assert_eq!(result.presentation_diagnostics.len(), 1, "{result:?}");
    assert_eq!(
        result.presentation_diagnostics[0].declaration.as_str(),
        "chosen"
    );
    assert!(matches!(
        result.presentation_diagnostics[0].detail.failure,
        PresentationFailure::Scale { .. }
    ));
    assert!(result.has_errors());
}

#[test]
fn failed_display_scales_preserve_nested_leaves_and_plot_channels() {
    let source = r"
param rate: Dimensionless = 0.0;
unit bad: Length = (1.0 / @rate) m;
type Reading { Reading(value: Length), }
node readings: Reading[Fin(2)] = for i: Fin(2) { Reading(value: 6.0 m -> bad) };
node leaves: Length[Fin(2)] = for i: Fin(2) { @readings[i].value };
plot measurement =  { mark: line, encode: { x: table[Fin(2)] { 1.0; 2.0; }, y: @leaves } };
";
    let result = compile_and_eval(source).unwrap();
    assert!(result.all.iter().all(|(_, value, _)| value.is_ok()));
    assert!(result.plot_errors.is_empty(), "{result:?}");
    assert_eq!(result.plots.len(), 1);
    assert_eq!(labels(&find_entry(&result, "readings")), [None, None]);
    assert!(
        result
            .presentation_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.detail.path.len() == 2)
    );
    assert!(
        result
            .presentation_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.channel.is_some())
    );
    let y = result.plots[0]
        .encodings
        .iter()
        .find(|(channel, _)| *channel == graphcal_compiler::syntax::ast::EncodingChannel::Y)
        .unwrap();
    assert!(
        matches!(&y.1, super::super::types::PlotFieldValue::Numbers(values) if values == &[6.0, 6.0])
    );
    assert!(result.has_errors());
}

#[test]
fn prepared_rows_failure_recovery_and_reset_agree_with_fresh_evaluation() {
    let source = "param rate: Dimensionless = 0.0; unit scaled: Length = (@rate) m; param input: Length = 12.0 m -> scaled; node out: Length = @input;";
    let project = crate::loader::LoadedProject::from_source(source, "rows.gcl").unwrap();
    let prepared = ProjectCompiler::new(&project).prepare().unwrap();
    for rate in [None, Some(3.0), Some(0.0), Some(6.0), None] {
        let mut builder = prepared.binding_builder();
        if let Some(rate) = rate {
            builder
                .bind_expression(
                    &DeclName::expect_valid("rate"),
                    &parse_expr(&format!("{rate:.1}")),
                )
                .unwrap();
        }
        let row = builder.finish().unwrap();
        let result = prepared.evaluate(&row).unwrap();
        let fresh_source = source.replace("= 0.0;", &format!("= {:.1};", rate.unwrap_or(0.0)));
        let fresh = compile_and_eval(&fresh_source).unwrap();
        assert_quantity_value(&result, "out", 12.0);
        assert_eq!(result.has_errors(), fresh.has_errors());
        assert_eq!(
            result.presentation_diagnostics.len(),
            fresh.presentation_diagnostics.len()
        );
        assert_eq!(
            find_entry(&result, "out")
                .format_display(Some(&result.base_dim_symbols))
                .unwrap(),
            find_entry(&fresh, "out")
                .format_display(Some(&fresh.base_dim_symbols))
                .unwrap()
        );
    }
    // A supplied closed value carries its own selection, never the default's.
    let mut builder = prepared.binding_builder();
    builder
        .bind_expression(&DeclName::expect_valid("input"), &parse_expr("800.0 cm"))
        .unwrap();
    let result = prepared.evaluate(&builder.finish().unwrap()).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(labels(&find_entry(&result, "out")), [Some("cm".into())]);
}

#[test]
fn caller_owned_forward_display_request_survives_child_call_without_an_environment() {
    let result = compile_and_eval(
        r"
dag identity { param input: Length; pub node output: Length = @input; }
node output: Length = @identity(input: 12.0 m -> scaled)::output;
unit scaled: Length = (@later) m;
node later: Dimensionless = 3.0;
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(
        find_entry(&result, "output")
            .format_display(Some(&result.base_dim_symbols))
            .unwrap(),
        "4 [scaled]"
    );
}

#[test]
fn retained_evidence_scales_with_selected_output_not_call_temporaries() {
    let measured = |temporary, output| {
        let source = format!(
            "dag worker {{ param rate: Dimensionless = 2.0; unit scaled: Length = (@rate) m; node temporary: Length[Fin({temporary})] = for i: Fin({temporary}) {{ 3.0 m -> scaled }}; pub node output: Length[Fin({output})] = for i: Fin({output}) {{ 6.0 m -> scaled }}; }} node result: Length[Fin({output})] = @worker()::output;"
        );
        let project = crate::loader::LoadedProject::from_source(&source, "retention.gcl").unwrap();
        let prepared = ProjectCompiler::new(&project).prepare().unwrap();
        let row = prepared.binding_builder().finish().unwrap();
        let (result, counts) =
            crate::pipeline_metrics::measure(|| prepared.evaluate(&row).unwrap());
        assert!(!result.has_errors(), "{result:?}");
        counts
    };
    let small = measured(2, 2);
    let large = measured(2000, 2);
    let output = measured(2, 200);
    assert!(large.call_frame_value_nodes > small.call_frame_value_nodes + 1900);
    assert_eq!(small.call_output_evidence_nodes, 3);
    assert_eq!(
        large.call_output_evidence_nodes,
        small.call_output_evidence_nodes
    );
    assert_eq!(output.call_output_evidence_nodes, 201);
    eprintln!("retention controls: {small:?}; {large:?}; {output:?}");
}

#[test]
fn constant_aliases_preserve_direct_and_callable_output_evidence() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub const node distance: Length = 1000.0 m -> km;",
            ),
            (
                "main.gcl",
                "import pipeline.lib::{distance as renamed}; dag worker { import pipeline.lib::{distance as local_distance}; pub node result: Length = @local_distance; } node direct: Length = @renamed; node called: Length = @worker()::result;",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(labels(&find_entry(&result, "direct")), [Some("km".into())]);
    assert_eq!(labels(&find_entry(&result, "called")), [Some("km".into())]);
}

#[test]
fn imported_constant_outputs_keep_selected_units_and_display_failures() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "const unit tiny: Length = 1.0e-300 m; pub const node distance: Length = 1000.0 m -> km; pub const node huge: Length = 1.0e300 m -> tiny;",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib; node output: Length = @lib::distance;",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let distance = result
        .consts
        .iter()
        .find(|(name, _)| name.member() == &DeclName::expect_valid("distance"))
        .unwrap()
        .1
        .as_ref()
        .unwrap();
    assert_eq!(labels(distance), [Some("km".into())]);
    let huge = result
        .consts
        .iter()
        .find(|(name, _)| name.member() == &DeclName::expect_valid("huge"))
        .unwrap()
        .1
        .as_ref()
        .unwrap();
    assert_eq!(huge.si_value().unwrap().to_bits(), 1.0e300_f64.to_bits());
    assert!(
        result
            .presentation_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.declaration.as_str() == "huge")
    );
}

#[test]
fn inline_computation_and_domain_failures_remain_value_failures() {
    let result = compile_and_eval(r"
dag broken_call { node broken: Dimensionless = 1.0 / 0.0; unit bad: Length = (@broken) m; pub node output: Length = 1.0 m -> bad; }
dag bounded { param amount: Length(min: 2.0 m); pub node output: Length = @amount -> km; }
node failed_call: Length = @broken_call()::output;
node failed_domain: Length = @bounded(amount: 1.0 m)::output;
node independent: Length = 3.0 m;
").unwrap();
    assert!(
        result
            .nodes
            .iter()
            .filter(|(name, _)| name != &scoped_name("independent"))
            .all(|(_, value)| value.is_err())
    );
    assert_quantity_value(&result, "independent", 3.0);
    assert!(result.presentation_diagnostics.is_empty());
}

#[test]
fn nested_presentation_computation_abort_classification_is_not_contained() {
    let source = "param rate: Dimensionless = 2.0; unit scaled: Length = (@rate) m; node value: Length = 1.0 m -> scaled;";
    let tir = compile_to_tir(source, "classification.gcl").unwrap();
    let src =
        miette::NamedSource::new("classification.gcl", std::sync::Arc::new(source.to_owned()));
    let declaration = tir
        .root()
        .bound_decl_identity(&scoped_name("value"))
        .unwrap();
    let graphcal_compiler::hir::ExprKind::Convert { target, .. } =
        tir.root().value_expr(declaration).unwrap().kind()
    else {
        panic!("conversion");
    };
    let context = |token| {
        crate::eval_expr::EvalContext::provisional_constants(
            &tir,
            tir.root_dag_id(),
            &src,
            graphcal_compiler::registry::builtins::builtin_functions(),
            token,
        )
        .unwrap()
    };
    let evidence = |unit| {
        PresentationInstance::Pending(Box::new(PendingDisplayUnit {
            owner: tir.root_dag_id().clone(),
            source: src.clone(),
            unit,
        }))
    };
    let mut unknown = target.clone();
    unknown.terms[0].name.value = graphcal_compiler::hir::expr::ResolvedUnitRef::new(
        unknown.terms[0].name.value.spelling().clone(),
        graphcal_compiler::syntax::dimension::ResolvedUnitName::from_def(
            tir.root_dag_id().clone(),
            graphcal_compiler::syntax::dimension::UnitName::expect_valid("missing"),
        ),
    );
    let values = crate::execution_facts::RuntimeValueMap::new();
    assert!(matches!(
        crate::eval_expr::presentation::resolve(
            evidence(unknown),
            &values,
            &context(graphcal_compiler::cancellation::CancellationToken::unbounded())
        ),
        Err(GraphcalError::InternalError { .. })
    ));
    // The outer presentation checkpoint succeeds; the unit-body evaluator cancels.
    assert!(matches!(crate::eval_expr::presentation::resolve(evidence(target.clone()), &values, &context(graphcal_compiler::cancellation::CancellationToken::cancel_after_successful_checkpoints(1))), Err(GraphcalError::Cancelled(_))));
}

#[test]
fn presentation_invariants_and_cancellation_are_never_notices() {
    let source = "node value: Length = 1.0 m -> km;";
    let tir = compile_to_tir(source, "classification.gcl").unwrap();
    let src =
        miette::NamedSource::new("classification.gcl", std::sync::Arc::new(source.to_owned()));
    let declaration = tir
        .root()
        .bound_decl_identity(&scoped_name("value"))
        .unwrap();
    let graphcal_compiler::hir::ExprKind::Convert { target, .. } =
        tir.root().value_expr(declaration).unwrap().kind()
    else {
        panic!("conversion");
    };
    let pending = |owner| {
        PresentationInstance::Pending(Box::new(PendingDisplayUnit {
            owner,
            source: src.clone(),
            unit: target.clone(),
        }))
    };
    let values = crate::execution_facts::RuntimeValueMap::new();
    let context = crate::eval_expr::EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        graphcal_compiler::registry::builtins::builtin_functions(),
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap();
    let wrong_owner = graphcal_compiler::dag_id::DagId::from_virtual_relative_path(
        std::path::Path::new("missing.gcl"),
    )
    .unwrap();
    assert!(matches!(
        crate::eval_expr::presentation::resolve(pending(wrong_owner), &values, &context),
        Err(GraphcalError::InternalError { .. })
    ));
    let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
    let context = crate::eval_expr::EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        graphcal_compiler::registry::builtins::builtin_functions(),
        cancellation.token(),
    )
    .unwrap();
    cancellation.cancel();
    assert!(matches!(
        crate::eval_expr::presentation::resolve(
            pending(tir.root_dag_id().clone()),
            &values,
            &context
        ),
        Err(GraphcalError::Cancelled(_))
    ));
}
