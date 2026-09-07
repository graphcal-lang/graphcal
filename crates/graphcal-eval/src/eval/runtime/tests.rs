use super::*;

#[test]
fn plot_properties_preserve_fatal_fact_and_cancellation_classification() {
    let source = "plot measurement = { mark: line { stroke_width: 2.0 }, encode: { x: 1.0, y: 2.0 }, width: 100.0 };";
    let tir = crate::eval::compile_to_tir(source, "plot_property.gcl").unwrap();
    let other = crate::eval::compile_to_tir(source, "other_property.gcl").unwrap();
    let src = NamedSource::new("plot_property.gcl", Arc::new(source.to_owned()));
    let original = &tir.root().plots()[0];
    let owner = tir
        .root()
        .require_bound_decl_identity(&original.name, &src, DiagnosticAnchor::WholeFile)
        .unwrap();
    let ctx = EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        builtin_functions(),
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap()
    .for_decl(&owner);
    let values = RuntimeValueMap::new();
    let presentations = PresentationInstanceMap::new();
    let errors = HashMap::new();
    assert!(evaluate_plot(original, &values, &presentations, &errors, &ctx).is_ok());
    let mut mark = original.clone();
    mark.body.mark_properties[0].value = other.root().plots()[0].body.mark_properties[0]
        .value
        .clone();
    let mut property = original.clone();
    property.body.properties[0].value = other.root().plots()[0].body.properties[0].value.clone();
    for entry in [mark, property] {
        let result = evaluate_plot(&entry, &values, &presentations, &errors, &ctx);
        assert!(
            matches!(
                result,
                Err(PlotEvaluationError::Fatal(
                    GraphcalError::InternalError { .. }
                ))
            ),
            "foreign property fact must stay fatal: {result:?}"
        );
    }
    let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
    let ctx = EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        builtin_functions(),
        cancellation.token(),
    )
    .unwrap()
    .for_decl(&owner);
    cancellation.cancel();
    assert!(matches!(
        eval_plot_property(&original.body.mark_properties[0].value, &values, &ctx),
        Err(PlotEvaluationError::Fatal(GraphcalError::Cancelled(_)))
    ));
}

#[test]
fn composition_properties_preserve_fatal_fact_classification() {
    let source = "plot curve = { mark: line { stroke_width: 2.0 }, encode: { x: 1.0, y: 2.0 } }; figure comparison = { plots: [curve], title: \"Comparison\" }; layer overlay = { plots: [curve], title: \"Overlay\", width: 400.0 };";
    let tir = crate::eval::compile_to_tir(source, "composition.gcl").unwrap();
    let other = crate::eval::compile_to_tir(source, "other.gcl").unwrap();
    let src = NamedSource::new("composition.gcl", Arc::new(source.to_owned()));
    let values = RuntimeValueMap::new();
    let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
    let ctx = EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        builtin_functions(),
        cancellation.token(),
    )
    .unwrap();
    for (original, foreign, names) in [
        (
            &tir.root().figures()[0].fields,
            &other.root().figures()[0].fields,
            &tir.root().figures()[0].plot_names,
        ),
        (
            &tir.root().layers()[0].fields,
            &other.root().layers()[0].fields,
            &tir.root().layers()[0].plot_names,
        ),
    ] {
        assert!(eval_composition_fields(original, names, &values, &ctx).is_ok());
        for position in 0..original.len() {
            let mut fields = original.clone();
            fields[position].value = foreign[position].value.clone();
            let result = eval_composition_fields(&fields, names, &values, &ctx);
            assert!(
                matches!(
                    result,
                    Err(PlotEvaluationError::Fatal(
                        GraphcalError::InternalError { .. }
                    ))
                ),
                "foreign composition property must stay fatal: {result:?}"
            );
        }
        let mut fields = original.clone();
        fields[0].property = tir.root().plots()[0].body.mark_properties[0]
            .property
            .clone();
        assert!(matches!(
            eval_composition_fields(&fields, names, &values, &ctx),
            Err(PlotEvaluationError::Fatal(
                GraphcalError::InternalError { .. }
            ))
        ));
    }
    cancellation.cancel();
    assert!(matches!(
        eval_composition_fields(
            &tir.root().figures()[0].fields,
            &tir.root().figures()[0].plot_names,
            &values,
            &ctx
        ),
        Err(PlotEvaluationError::Fatal(GraphcalError::Cancelled(_)))
    ));
    assert!(matches!(
        eval_composition_fields(
            &tir.root().layers()[0].fields,
            &tir.root().layers()[0].plot_names,
            &values,
            &ctx
        ),
        Err(PlotEvaluationError::Fatal(GraphcalError::Cancelled(_)))
    ));
}

#[test]
fn ordinary_plot_and_composition_property_failures_remain_contained() {
    let result = crate::eval::compile_and_eval("node value: Length = 1.0 m; plot good = { mark: line, encode: { x: 1.0, y: 2.0 } }; plot broken = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } }; plot bad_width = { mark: line, encode: { x: 1.0, y: 2.0 }, width: -1.0 }; figure comparison = { plots: [good], title: \"Valid\" }; layer overlay = { plots: [good], width: 1.0 / 0.0 };").unwrap();
    assert!(result.nodes[0].1.is_ok());
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.figures.len(), 1);
    assert_eq!(result.plot_errors.len(), 3);
    assert!(result.presentation_diagnostics.is_empty());
}
