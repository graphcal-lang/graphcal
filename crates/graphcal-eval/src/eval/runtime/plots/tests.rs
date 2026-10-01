use std::sync::Arc;

use super::*;

/// The unit of the declaration `owner` of `tir`'s root.
fn root_unit<'t>(
    tir: &'t graphcal_compiler::tir::typed::CheckedTir,
    owner: &ResolvedDeclName,
) -> DeclarationBody<'t> {
    tir.declaration_body(owner).unwrap()
}

#[test]
fn plot_properties_preserve_cancellation_classification() {
    let source = "plot measurement = { mark: line { stroke_width: 2.0 }, encode: { x: 1.0, y: 2.0 }, width: 100.0 };";
    let (tir, src, sources) = crate::test_tir::checked_tir_from_source(source).unwrap();
    let unit = root_unit(
        &tir,
        &tir.root()
            .body_for_test()
            .plots()
            .next()
            .unwrap()
            .identity(),
    );
    let plot = unit.plot().unwrap();
    let ctx = EvalSession::provisional_constants(
        &tir,
        src,
        &sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    );
    let values = RuntimeValueMap::new();
    let presentations = crate::runtime_presentation::ResolvedPresentedMap::new();
    let frame_presentations = PendingPresentedMap::new();
    let errors = HashMap::new();
    let evaluated = EvaluatedRoot {
        values: &values,
        errors: &errors,
        presentations: &presentations,
        frame_presentations: &frame_presentations,
    };
    assert!(evaluate_plot(unit, plot, evaluated, &ctx).is_ok());
    let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
    let ctx = EvalSession::provisional_constants(&tir, src, &sources, cancellation.token());
    cancellation.cancel();
    assert!(matches!(
        eval_plot_property(
            plot.map(|plot| &*plot.body.mark_properties[0].value),
            &values,
            &ctx
        ),
        Err(PlotEvaluationError::Fatal(Outcome::Cancelled))
    ));
}

/// A plot's property trees are read only from its own unit, so a property
/// from another program cannot reach its evaluation; the internal error a
/// missing tree would raise stays fatal rather than being reported on the
/// plot.
#[test]
fn internal_errors_abort_plot_evaluation() {
    let src = graphcal_compiler::source_registry::SourceRegistry::new()
        .register("plot_property.gcl", Arc::new(String::new()));
    assert!(matches!(
        PlotEvaluationError::from(GraphcalError::internal_error(
            "missing checked expression",
            src,
            DiagnosticAnchor::WholeFile,
        )),
        PlotEvaluationError::Fatal(Outcome::Failed(GraphcalError::InternalError { .. }))
    ));
}

#[test]
fn composition_properties_preserve_cancellation_classification() {
    let source = "plot curve = { mark: line { stroke_width: 2.0 }, encode: { x: 1.0, y: 2.0 } }; figure comparison = { plots: [curve], title: \"Comparison\" }; layer overlay = { plots: [curve], title: \"Overlay\", width: 400.0 };";
    let (tir, src, sources) = crate::test_tir::checked_tir_from_source(source).unwrap();
    let values = RuntimeValueMap::new();
    let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
    let ctx = EvalSession::provisional_constants(&tir, src, &sources, cancellation.token());
    let figure = tir.root().body_for_test().figures().next().unwrap();
    let layer = tir.root().body_for_test().layers().next().unwrap();
    let compositions = [
        (
            root_unit(&tir, &figure.identity())
                .figure()
                .unwrap()
                .map(|figure| figure.fields.as_slice()),
            &figure.plot_names,
        ),
        (
            root_unit(&tir, &layer.identity())
                .layer()
                .unwrap()
                .map(|layer| layer.fields.as_slice()),
            &layer.plot_names,
        ),
    ];
    for (fields, names) in compositions {
        assert!(eval_composition_fields(fields, names, &values, &ctx).is_ok());
    }
    cancellation.cancel();
    for (fields, names) in compositions {
        assert!(matches!(
            eval_composition_fields(fields, names, &values, &ctx),
            Err(PlotEvaluationError::Fatal(Outcome::Cancelled))
        ));
    }
}
