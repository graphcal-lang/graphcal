use super::*;
use crate::registry::declared_type::IndexTypeRef;
use crate::syntax::dimension::BaseDimId;
use crate::syntax::names::{DeclName, ResolvedName, ScopedName, namespace};
use crate::syntax::parser::Parser;
use crate::syntax::span::Span;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test.gcl", Arc::new(source.to_string()))
}

fn test_dag_id() -> crate::dag_id::DagId {
    crate::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl")).unwrap()
}

fn test_index_ref(name: &str) -> IndexTypeRef {
    IndexTypeRef::with_owner(
        test_dag_id(),
        crate::syntax::names::IndexName::new(name.to_string()),
    )
}

fn check(source: &str) -> Result<HashMap<ScopedName, DeclaredType>, GraphcalError> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
    let src = make_src(source);
    let ir = crate::ir::lower::lower(&file, &src)?;
    let parent_dag_id = test_dag_id();
    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    resolver
        .add_module(parent_dag_id.clone(), &file.declarations)
        .map_err(|err| GraphcalError::InternalError {
            message: format!("test module resolver failed for root module: {err}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    for decl in &file.declarations {
        if let crate::desugar::resolved_ast::DeclKind::Dag(dag) = &decl.kind {
            resolver
                .add_module(parent_dag_id.child(dag.name.value.as_str()), &dag.body)
                .map_err(|err| GraphcalError::InternalError {
                    message: format!(
                        "test module resolver failed for inline dag `{}`: {err}",
                        dag.name.value
                    ),
                    src: src.clone(),
                    span: Span::new(0, 0).into(),
                })?;
        }
    }
    let mut module_types = crate::tir::typed::ModuleTypeRegistry::default();
    module_types
        .insert_graphcal_prelude()
        .map_err(|err| GraphcalError::InternalError {
            message: format!("test module type prelude failed: {err}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    module_types.insert_registry(&parent_dag_id, &ir.registry);
    let mut tir = crate::tir::typed::type_resolve_with_modules(
        ir,
        parent_dag_id.clone(),
        &src,
        &resolver,
        &module_types,
    )?;
    compile_inline_dag_bodies_test(&mut tir, &src, &parent_dag_id, &file.declarations)?;
    check_dimensions_tir(&tir, &src)?;
    tir.build_declared_types(&src)
}

fn module_aware_tir(source: &str) -> (crate::tir::typed::TIR, NamedSource<Arc<String>>) {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
    let src = make_src(source);
    let dag_id =
        crate::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let ir = crate::ir::lower::lower(&file, &src).unwrap();
    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    resolver
        .add_module(dag_id.clone(), &file.declarations)
        .unwrap();
    let mut module_types = crate::tir::typed::ModuleTypeRegistry::default();
    module_types.insert_graphcal_prelude().unwrap();
    module_types.insert_registry(&dag_id, &ir.registry);
    let tir =
        crate::tir::typed::type_resolve_with_modules(ir, dag_id, &src, &resolver, &module_types)
            .unwrap();
    (tir, src)
}

/// Compile each inline dag body in `tir` with no self-import preprocessing.
/// Used by compiler-side integration tests that don't have access to the
/// eval crate's project pipeline.
fn compile_inline_dag_bodies_test(
    tir: &mut crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
    parent_declarations: &[crate::desugar::resolved_ast::Declaration],
) -> Result<(), GraphcalError> {
    let dag_bodies = tir
        .registry
        .dags
        .all_dags()
        .map(|(name, dag)| (name.clone(), dag.body.clone()))
        .collect::<Vec<_>>();

    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    resolver
        .add_module(parent_dag_id.clone(), parent_declarations)
        .map_err(|err| GraphcalError::InternalError {
            message: format!("test module resolver failed for parent module: {err}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    for (name, body) in &dag_bodies {
        resolver
            .add_module(parent_dag_id.child(name.as_str()), body)
            .map_err(|err| GraphcalError::InternalError {
                message: format!("test module resolver failed for inline dag `{name}`: {err}"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
    }
    for (name, body) in &dag_bodies {
        let owner = parent_dag_id.child(name.as_str());
        for decl in body {
            if let crate::desugar::resolved_ast::DeclKind::Import(import) = &decl.kind {
                resolver
                    .register_import(&owner, &import.path, &import.kind, parent_dag_id)
                    .map_err(|err| GraphcalError::InternalError {
                        message: format!(
                            "test module resolver failed to register inline dag import: {err}"
                        ),
                        src: src.clone(),
                        span: Span::new(0, 0).into(),
                    })?;
            }
        }
    }
    let mut module_types = crate::tir::typed::ModuleTypeRegistry::default();
    module_types
        .insert_graphcal_prelude()
        .map_err(|err| GraphcalError::InternalError {
            message: format!("test module type prelude failed: {err}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    module_types.insert_registry(parent_dag_id, &tir.registry);

    for (name, body) in dag_bodies {
        let dag_body_ir = crate::ir::lower::lower_dag_body_to_ir(
            &name,
            &body,
            &tir.registry,
            &crate::ir::resolve::ImportedValueNames::default(),
            HashMap::new(),
            HashMap::new(),
            src,
            parent_dag_id,
        )?;
        let dag_id = parent_dag_id.child(name.as_str());
        let mut compiled_dag = crate::tir::typed::type_resolve_single_with_modules(
            dag_body_ir,
            &dag_id,
            src,
            &resolver,
            &module_types,
        )?;
        compiled_dag.populate_pub_nodes(&body);
        tir.dags.insert(dag_id, compiled_dag);
    }
    Ok(())
}

#[test]
fn cycle_detection_uses_semantic_dependencies() {
    use std::collections::BTreeSet;

    let source = "const node a: Dimensionless = 1.0;\n\
                  const node b: Dimensionless = @a + 1.0;\n\
                  node x: Dimensionless = 1.0;\n\
                  node y: Dimensionless = @x + 1.0;";
    let (mut tir, src) = module_aware_tir(source);
    let dag_id = test_dag_id();

    let a = ResolvedName::from_def(dag_id.clone(), DeclName::new("a"));
    let b = ResolvedName::from_def(dag_id.clone(), DeclName::new("b"));
    let x = ResolvedName::from_def(dag_id.clone(), DeclName::new("x"));
    let y = ResolvedName::<namespace::Decl>::from_def(dag_id, DeclName::new("y"));

    let mut resolved = crate::tir::typed::ResolvedDagDependencies::default();
    resolved.const_deps.insert(a.clone(), BTreeSet::new());
    resolved.const_deps.insert(b, BTreeSet::from([a]));
    resolved.runtime_deps.insert(x.clone(), BTreeSet::new());
    resolved.runtime_deps.insert(y, BTreeSet::from([x]));

    tir.root_mut().semantic.dependencies = resolved;

    check_dimensions_tir(&tir, &src).unwrap();
}

#[test]
fn hir_dim_check_uses_lowered_builtin_function_not_mutated_syntax_callee() {
    let (mut tir, src) = module_aware_tir("node y: Dimensionless = sqrt(4.0);");
    assert!(!tir.root().semantic.expressions.nodes.is_empty());
    tir.root_mut().nodes[0].expr.kind =
        crate::desugar::resolved_ast::ExprKind::StringLiteral("not the HIR".to_string());

    check_dimensions_tir(&tir, &src).unwrap();
}

#[test]
fn hir_dim_check_uses_lexical_local_ids_not_mutated_syntax_names() {
    let (mut tir, src) =
        module_aware_tir("index Phase = { Burn };\nnode y: Phase[Phase] = for p: Phase { p };");
    assert!(!tir.root().semantic.expressions.nodes.is_empty());
    tir.root_mut().nodes[0].expr.kind =
        crate::desugar::resolved_ast::ExprKind::StringLiteral("not the HIR".to_string());

    check_dimensions_tir(&tir, &src).unwrap();
}

#[test]
fn hir_dim_check_uses_lowered_assert_body_not_mutated_syntax_body() {
    let (mut tir, src) = module_aware_tir("assert ok = sqrt(4.0) == 2.0;");
    assert!(!tir.root().semantic.expressions.asserts.is_empty());
    let span = tir.root().asserts[0].span;
    tir.root_mut().asserts[0].body =
        crate::desugar::resolved_ast::AssertBody::Expr(crate::desugar::resolved_ast::Expr::new(
            crate::desugar::resolved_ast::ExprKind::StringLiteral("not the HIR".to_string()),
            span,
        ));

    check_dimensions_tir(&tir, &src).unwrap();
}

#[test]
fn check_dimensionless_const() {
    let types = check("const node g0: Dimensionless = 9.80665;").unwrap();
    assert_eq!(
        types[&ScopedName::local("g0")],
        DeclaredType::Scalar(Dimension::dimensionless())
    );
}

#[test]
fn check_dimensionless_arithmetic() {
    let types = check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
    assert_eq!(
        types[&ScopedName::local("y")],
        DeclaredType::Scalar(Dimension::dimensionless())
    );
}

#[test]
fn check_length_unit_literal() {
    let types = check("param alt: Length = 400.0 km;").unwrap();
    let length = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    assert_eq!(
        types[&ScopedName::local("alt")],
        DeclaredType::Scalar(length)
    );
}

#[test]
fn check_velocity_from_division() {
    let source = "param dist: Length = 100.0 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string())))
    .unwrap();
    assert_eq!(
        types[&ScopedName::local("speed")],
        DeclaredType::Scalar(velocity)
    );
}

#[test]
fn check_add_dimension_mismatch() {
    let source = "param x: Length = 1.0 m;\nparam y: Time = 1.0 s;\nnode z: Length = @x + @y;";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_annotation_mismatch() {
    let source = "param x: Length = 1.0 m;\nnode y: Time = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_duplicate_key() {
    let source = "\
pub index Mode = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Mode.A, Mode.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExpectedFailDuplicateKey { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_foreign_index_key() {
    let source = "\
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Other.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExpectedFailKeyIndexMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_partial_tuple_key() {
    let source = "\
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 2.0 };
#[expected_fail(Mode.A)]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExpectedFailKeyShapeMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_variant_key_on_scalar_assertion() {
    let source = "\
pub index Mode = { A, B };
param lhs: Dimensionless = 1.0;
param rhs: Dimensionless = 2.0;
#[expected_fail(Mode.A)]
assert order = @lhs > @rhs;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExpectedFailNotIndexed { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_blanket_on_indexed_graph_ref() {
    let source = "\
pub index Mode = { A, B };
node flags: Bool[Mode] = for m: Mode { true };
#[expected_fail]
assert order = @flags;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExpectedFailAllOnIndexed { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_conversion_same_dimension() {
    let source =
        "param speed: Velocity = 100.0 m / s;\nnode speed_kmh: Velocity = @speed -> km / hour;";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string())))
    .unwrap();
    assert_eq!(
        types[&ScopedName::local("speed_kmh")],
        DeclaredType::Scalar(velocity)
    );
}

#[test]
fn check_conversion_wrong_dimension() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = @x -> s;";
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::ConversionDimensionMismatch { .. }
    ));
}

#[test]
fn check_sqrt_dimension() {
    let source = "param area: Area = 100.0 m;\nnode side: Length = sqrt(@area);";
    // Note: area should be m^2, but we declared it with m (Length).
    // sqrt(Length) = Length^(1/2) which doesn't match Length.
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DimensionMismatchInAnnotation { .. }
    ));
}

#[test]
fn check_builtin_sin_requires_angle() {
    let source = "param x: Length = 1.0 m;\nnode y: Dimensionless = sin(@x);";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_if_branches_same_dim() {
    let source =
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };";
    check(source).unwrap();
}

#[test]
fn check_if_branches_different_dim() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = if true { @x } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_multiplication_creates_new_dim() {
    let source = "param mass: Mass = 10.0 kg;\nparam accel: Acceleration = 9.8 m / s^2;\nnode force: Force = @mass * @accel;";
    check(source).unwrap();
}

#[test]
fn check_power_with_literal() {
    let source = "param r: Length = 5.0 m;\nnode area: Area = @r ^ 2.0;";
    // Area is Length^2, r^2 = Length^2
    // But we need PI * r^2 for circle area — just testing r^2 = Area
    check(source).unwrap();
}

#[test]
fn check_fn_unknown_function() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = no_such_fn(@x);";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

// --- Indexed type tests ---

#[test]
fn check_indexed_param_map_literal() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string())))
    .unwrap();
    assert_eq!(
        types[&ScopedName::local("dv")],
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Scalar(velocity)),
            index: test_index_ref("Maneuver"),
        }
    );
}

#[test]
fn check_for_comprehension() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node doubled: Velocity[Maneuver] = for m: Maneuver { @dv[m] + @dv[m] };";
    check(source).unwrap();
}

#[test]
fn check_for_comprehension_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node bad: Length[Maneuver] = for m: Maneuver { @dv[m] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_index_access_with_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
param first: Velocity = @dv[Maneuver.Departure];";
    check(source).unwrap();
}

#[test]
fn check_map_literal_missing_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::MissingVariants { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_map_literal_extra_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExtraVariants { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_index_mismatch_in_for() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node bad: Velocity[Phase] = for p: Phase { @dv[p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_sum_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node total_dv: Velocity = sum(@dv);";
    check(source).unwrap();
}

#[test]
fn check_count_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node n: Dimensionless = count(@dv);";
    check(source).unwrap();
}

#[test]
fn check_mean_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node avg_dv: Velocity = mean(@dv);";
    check(source).unwrap();
}

#[test]
fn check_scan() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node cum_dv: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc + val);";
    check(source).unwrap();
}

#[test]
fn check_scan_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 m, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_unknown_index_in_type_annotation() {
    let source = "param x: Velocity[NoSuchIndex] = 1.0 m / s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownIndex { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_for_with_sum() {
    // sum over a for comprehension
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node total: Velocity = sum(for m: Maneuver { @dv[m] });";
    check(source).unwrap();
}

// --- Comparison dimension mismatch ---

#[test]
fn check_comparison_dimension_mismatch() {
    let source = "\
param x: Length = 1.0 m;
param t: Time = 1.0 s;
node bad: Dimensionless = if @x > @t { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Boolean operator dimension errors ---

#[test]
fn check_boolean_and_lhs_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = @x && true;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_boolean_or_rhs_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = true || @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Power / exponent edge cases ---

#[test]
fn check_power_half_exponent() {
    // x ^ 0.5 on dimensionless should work
    let source = "param x: Dimensionless = 4.0;\nnode y: Dimensionless = @x ^ 0.5;";
    check(source).unwrap();
}

#[test]
fn check_power_non_literal_exponent_dimensioned_base() {
    // dimensioned ^ non-literal → NonLiteralExponent
    let source = "\
param x: Length = 1.0 m;
param n: Dimensionless = 2.0;
node bad: Area = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_dimensionless_base_non_literal_exponent() {
    // dimensionless ^ dimensionless (non-literal) → ok
    let source = "\
param x: Dimensionless = 2.0;
param n: Dimensionless = 3.0;
node y: Dimensionless = @x ^ @n;";
    check(source).unwrap();
}

#[test]
fn check_power_bad_fractional_exponent() {
    // x ^ 0.3 → NonLiteralExponent (not 0.5 and not integer)
    let source = "param x: Length = 1.0 m;\nnode bad: Length = @x ^ 0.3;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_dimensioned_exponent() {
    // anything ^ dimensioned → NonLiteralExponent
    let source = "\
param x: Dimensionless = 2.0;
param n: Length = 1.0 m;
node bad: Dimensionless = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_signed_integer_literal_exponent() {
    // x ^ -2 with a dimensioned base should be accepted: `-2` is a
    // compile-time-known signed literal even though it parses as
    // `Unary(Neg, IntLit(2))`. (Issue #579.)
    let source = "\
pub dim InvLengthSquared = Length ^ -2;
param x: Length = 2.0 m;
node y: InvLengthSquared = @x ^ -2.0;";
    check(source).unwrap();
}

#[test]
fn check_power_signed_float_literal_exponent() {
    // Same as above but with a float literal: `-2.0`.
    let source = "param x: Dimensionless = 2.0;\nnode y: Dimensionless = @x ^ -2.0;";
    check(source).unwrap();
}

#[test]
fn check_power_int_chain_constant_folds() {
    // `2 ^ 3 ^ 2` parses right-assoc as `2 ^ (3 ^ 2)`. Float chains were
    // already accepted via the dimensionless ^ dimensionless rule; the Int
    // branch now constant-folds the rhs to `9` so the Int chain symmetrizes.
    // (Issue #578.)
    check("const node i: Int = 2 ^ 3 ^ 2;").unwrap();
}

#[test]
fn check_power_int_chain_with_negative_constant_exponent_rejected() {
    // Constant-folding produces a negative exponent — should be rejected
    // with the Int-specific "non-negative" diagnostic, not "non-literal".
    let err = check("const node bad: Int = 2 ^ (3 - 5);").unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_int_signed_negative_literal_exponent_rejected_with_int_message() {
    // `Int ^ -2` is still rejected (Int^negative would not be Int), but the
    // diagnostic should now be the clearer "non-negative Int exponent" rather
    // than "non-literal exponent". (Issue #579.)
    let source = "param x: Int = 2;\nnode y: Int = @x ^ -2;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- If condition must be dimensionless ---

#[test]
fn check_if_condition_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = if @x { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Unknown dimension in type annotation ---

#[test]
fn check_unknown_dimension_in_type() {
    let source = "param x: NoSuchDimension = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownDimension { .. }),
        "got: {err:?}"
    );
}

// --- expect_scalar error: struct used where scalar expected ---

#[test]
fn check_struct_in_arithmetic() {
    let source = "\
pub type Orbit { Orbit(altitude: Length, speed: Velocity) }
param o: Orbit = Orbit(altitude: 400.0 km, speed: 7.6 km / s);
node bad: Length = @o + 1.0 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- FieldAccess on non-struct ---

#[test]
fn check_field_access_on_scalar() {
    let source = "\
param x: Length = 1.0 m;
node bad: Length = @x.foo;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NotAStruct { .. }),
        "got: {err:?}"
    );
}

// --- Struct extra fields ---

#[test]
fn check_struct_extra_fields() {
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
node o: Orbit = Orbit(altitude: 400.0 km, speed: 7.6 km / s, bonus: 1.0);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExtraFields { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_struct_duplicate_field_initializers() {
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
node o: Orbit = Orbit(altitude: 400.0 km, altitude: 401.0 km, speed: 7.6 km / s);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_match_wildcard_binding_validates_field_name() {
    let source = "\
pub type Maybe { Some(value: Length), None }
param x: Maybe = Some(value: 1.0 m);
node y: Length = match @x { Some(nope: _) => 1.0 m, None => 0.0 m };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownField { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_match_rejects_duplicate_field_bindings() {
    let source = "\
pub type Pair { Pair(a: Length, b: Length) }
param x: Pair = Pair(a: 1.0 m, b: 2.0 m);
node y: Length = match @x { Pair(a: left, a: right) => left + right };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

// --- Block let-binding type annotation mismatch ---

// --- types_match wildcard: mismatched kinds ---

#[test]
fn check_types_match_struct_vs_scalar() {
    // Declared as a struct type but expression evaluates to scalar → mismatch
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
param x: Dimensionless = 1.0;
node o: Orbit = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

// --- ForComp with unknown index ---

#[test]
fn check_for_comp_unknown_index() {
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = for m: NoSuchIndex { @x };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownIndex { .. }),
        "got: {err:?}"
    );
}

// --- Scan body type mismatch ---

#[test]
fn check_scan_body_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver.Departure: 2.46 km / s,
Maneuver.Correction: 0.5 km / s,
Maneuver.Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc * val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Scan on non-indexed value ---

#[test]
fn check_scan_on_scalar() {
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = scan(@x, 0.0, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

// --- Map literal dimension inconsistency ---

#[test]
fn check_map_literal_inconsistent_element_dims() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless[Phase] = {
Phase.Coast: 1.0,
Phase.Burn: 2.0 m,
};";
    let err = check(source).unwrap_err();
    // The map entries have different dimensions: first is Dimensionless, second is Length
    assert!(
        matches!(
            err,
            GraphcalError::DimensionMismatchInAnnotation { .. }
                | GraphcalError::DimensionMismatch { .. }
        ),
        "got: {err:?}"
    );
}

// --- Index access with unknown variant ---

#[test]
fn check_index_access_unknown_variant() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless[Phase] = {
Phase.Coast: 1.0,
Phase.Burn: 2.0,
};
param bad: Dimensionless = @x[Phase.NoSuch];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownVariant { .. }),
        "got: {err:?}"
    );
}

// --- Indexing a non-indexed value ---

#[test]
fn check_index_access_on_scalar() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless = 1.0;
param bad: Dimensionless = @x[Phase.Coast];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

// --- Index access with wrong index name ---

#[test]
fn check_index_access_wrong_index() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Stage = { First, Second };
param x: Dimensionless[Phase] = {
Phase.Coast: 1.0,
Phase.Burn: 2.0,
};
param bad: Dimensionless = @x[Stage.First];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through if/else sub-expressions ---

#[test]
fn check_if_error_in_condition() {
    // Error inside condition sub-expression (unknown unit)
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if (1.0 foobar > 0.0) { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_if_error_in_then_branch() {
    // Error in then-branch sub-expression
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 1.0 foobar } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_if_error_in_else_branch() {
    // Error in else-branch sub-expression
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 0.0 } else { 1.0 foobar };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through convert sub-expression ---

#[test]
fn check_convert_error_in_inner() {
    // Error inside the inner expression of a convert
    let source = "\
node bad: Length = (1.0 foobar) -> m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through block binding ---

// --- Error propagation through field access inner expression ---

#[test]
fn check_field_access_error_in_inner() {
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
node bad: Length = (1.0 foobar).altitude;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through constructor-call field value ---

#[test]
fn check_struct_construction_error_in_field_value() {
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
node o: Orbit = Orbit(altitude: 1.0 foobar, speed: 7.6 km / s);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through for comprehension body ---

#[test]
fn check_for_comp_error_in_body() {
    let source = "\
pub index Phase = { Coast, Burn };
node bad: Dimensionless[Phase] = for p: Phase { 1.0 foobar };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through aggregation arg ---

#[test]
fn check_aggregation_error_in_arg() {
    let source = "\
node bad: Dimensionless = sum(1.0 foobar);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through scan source/init ---

#[test]
fn check_scan_error_in_source() {
    let source = "\
pub index Phase = { Coast, Burn };
node bad: Dimensionless[Phase] = scan(1.0 foobar, 0.0, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through map literal entry ---

#[test]
fn check_map_literal_error_in_entry() {
    let source = "\
pub index Phase = { Coast, Burn };
param bad: Dimensionless[Phase] = {
Phase.Coast: 1.0 foobar,
Phase.Burn: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Map literal with mixed index names ---

#[test]
fn check_map_literal_mixed_index_names() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Stage = { First, Second };
param x: Dimensionless[Phase] = {
Phase.Coast: 1.0,
Stage.Second: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Block let-binding with valid type annotation ---

// -----------------------------------------------------------------------
// Fin(N) type: loop variable bounds checking
// -----------------------------------------------------------------------

#[test]
fn fin_same_size_indexing() {
    // i : Fin(3) indexing into D[3] — 3 <= 3 — safe
    let source = "\
param v: Dimensionless[3] = for i: range(3) { 1.0 };
node w: Dimensionless[3] = for i: range(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_smaller_bound_indexing() {
    // i : Fin(3) indexing into D[5] — 3 <= 5 — safe
    let source = "\
param v: Dimensionless[5] = for i: range(5) { 1.0 };
node w: Dimensionless[3] = for i: range(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_out_of_bounds() {
    // i : Fin(5) indexing into D[3] — 5 > 3 — compile error
    let source = "\
param v: Dimensionless[3] = for i: range(3) { 1.0 };
node w: Dimensionless[5] = for i: range(5) { @v[i] };";
    let err = check(source).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("index out of bounds"),
        "expected bounds error, got: {msg}"
    );
}

#[test]
fn fin_comparison_same_range() {
    // i : Fin(3), j : Fin(3) — i == j is valid
    let source = "\
node m: Dimensionless[3, 3] = for i: range(3), j: range(3) {
    if i == j { 1.0 } else { 0.0 }
};";
    check(source).unwrap();
}

#[test]
fn fin_arithmetic_with_int() {
    // Fin(N) * Dimensionless -> error (Fin is not Scalar)
    // Fin(N) + Fin(M) -> Int (arithmetic), then Int * Dimensionless -> error
    // to_float(i) -> Dimensionless (via Int coercion)
    let source = "\
node v: Dimensionless[3] = for i: range(3) { to_float(i) };";
    check(source).unwrap();
}

// -----------------------------------------------------------------------
// Domain constraint bound dimensions (#438)
// -----------------------------------------------------------------------

#[test]
fn domain_bound_unit_literal_matches() {
    let source = "param m: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;";
    check(source).unwrap();
}

#[test]
fn domain_bound_dimensionless_accepts_int() {
    // Bare Int literal is accepted as a Dimensionless bound (existing behavior).
    let source = "param r: Dimensionless(min: 0, max: 1) = 0.5;";
    check(source).unwrap();
}

#[test]
fn domain_bound_bare_number_on_dimensioned_rejected() {
    // Bare numbers infer as Dimensionless, mismatching Mass.
    // This is the implicit-unit-attachment case from #440.
    let source = "param m: Mass(min: 1.0, max: 100.0 kg) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_bare_int_on_dimensioned_rejected() {
    // Integer literal on a dimensioned scalar should also be rejected.
    let source = "param m: Mass(min: 1, max: 100.0 kg) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_creates_wrong_dimension() {
    // 1.0 m / 1.0 s is Velocity, but the constrained type is Length.
    let source = "param d: Length(min: 1.0 m / 1.0 s) = 5.0 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_inverse_dimension() {
    // 1.0 / 1.0 kg is 1/Mass, not Mass.
    let source = "param x: Mass(min: 1.0 / 1.0 kg) = 5.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_addition_unit_mismatch_in_bound() {
    // 5.0 m + 3.0 s is itself a dimension mismatch inside the bound expression.
    let source = "param t: Time(min: 5.0 m + 3.0 s) = 10.0 s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_convert_preserves_dimension() {
    // Conversion between units of the same dimension is fine.
    let source = "param m: Mass(min: 1.0 kg -> g) = 5.0 kg;";
    check(source).unwrap();
}

#[test]
fn domain_bound_multiplication_creates_correct_dimension() {
    // 10.0 kg * 9.8 m / s^2 is Force; Force(min: ...) accepts it.
    let source = "param f: Force(min: 10.0 kg * 9.8 m / s^2) = 100.0 N;";
    check(source).unwrap();
}

#[test]
fn domain_bound_indexed_dimension_checked() {
    // Constraints on the base of an indexed type are also checked.
    let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity(min: 1.0 m)[Maneuver] = {
Maneuver.Departure: 1.0 m / s,
Maneuver.Correction: 0.5 m / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// -----------------------------------------------------------------------
// Int domain bound must be unitless (#439)
// -----------------------------------------------------------------------

#[test]
fn int_domain_bound_int_literal_accepted() {
    let source = "param n: Int(min: 1, max: 100) = 5;";
    check(source).unwrap();
}

#[test]
fn int_domain_bound_dimensionless_scalar_accepted() {
    let source = "param n: Int(min: 0.0, max: 100.0) = 5;";
    check(source).unwrap();
}

#[test]
fn int_domain_bound_with_unit_rejected() {
    let source = "param n: Int(min: 1.0 kg, max: 10.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_arithmetic_with_unit_rejected() {
    // Arithmetic that produces a dimensioned result is also rejected.
    let source = "param n: Int(min: 1.0 m / 1.0 s) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

// -----------------------------------------------------------------------
// Domain bound dimension checks on const nodes (#441)
// -----------------------------------------------------------------------

#[test]
fn const_domain_bound_dimension_checked() {
    // Const nodes get the same compile-time bound dimension check as params/nodes.
    let source = "const node MAX_M: Mass(min: 1.0 m) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_domain_bound_int_with_unit_rejected() {
    let source = "const node MAX_N: Int(min: 1.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_domain_bound_well_formed_passes_dim_check() {
    // Well-formed const constraint passes dim_check (value-vs-bound check is
    // in exec_plan, not dim_check).
    let source = "const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;";
    check(source).unwrap();
}

// --- Inline DAG invocation (issue #451) ---

const INLINE_DAG_CALL_SCALE: &str = "\
dag scale {
    param factor: Dimensionless;
    param v: Length;
    pub node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node doubled: Length = @scale(factor: 2.0, v: @src).result;
";

#[test]
fn inline_dag_call_basic_returns_output_type() {
    let types = check(INLINE_DAG_CALL_SCALE).unwrap();
    let length = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    assert_eq!(
        types[&ScopedName::local("doubled")],
        DeclaredType::Scalar(length)
    );
}

#[test]
fn inline_dag_call_unknown_dag() {
    let source = "\
param src: Length = 10.0 m;
node y: Length = @nope(v: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("unknown module")),
        "got: {err:?}"
    );
}

#[test]
fn inline_dag_call_unknown_param() {
    let source = "\
dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param src: Length = 10.0 m;
node y: Length = @id_len(bogus: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownLocalRef { .. }),
        "got: {err:?}"
    );
}

#[test]
fn inline_dag_call_missing_binding() {
    let source = "\
dag scale {
    param factor: Dimensionless;
    param v: Length;
    pub node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node y: Length = @scale(v: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::MissingInlineDagBindings { missing, .. } if missing == &vec!["factor".to_string()]),
        "got: {err:?}"
    );
}

#[test]
fn inline_dag_call_unknown_output() {
    let source = "\
dag id_len {
    param v: Length;
    node result: Length = @v;
}

param src: Length = 10.0 m;
node y: Length = @id_len(v: @src).nope;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownLocalRef { .. }),
        "got: {err:?}"
    );
}

#[test]
fn inline_dag_call_arg_dim_mismatch() {
    let source = "\
dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param src: Time = 10.0 s;
node y: Length = @id_len(v: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InlineDagArgDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn inline_dag_call_inside_for_comp_with_loop_var() {
    // Motivating shape: inline call inside a `for` comprehension whose
    // argument references the loop variable via an indexed graph ref.
    let source = "\
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist: Length[Region] = { Region.A: 1.0 m, Region.B: 2.0 m };
node distances: Length[Region] = for r: Region { @id_len(v: @dist[r]).result };
";
    let types = check(source).unwrap();
    let length = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    assert_eq!(
        types[&ScopedName::local("distances")],
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Scalar(length)),
            index: test_index_ref("Region"),
        }
    );
}

#[test]
fn inline_dag_body_dimension_mismatch_caught_at_compile_time() {
    // A dag body that returns a value whose dimension disagrees with its
    // declared node type. The MVP never dim-checked dag body expressions;
    // the compile-pipeline refactor catches it.
    let source = "\
dag bogus {
    param v: Length;
    pub node result: Length = @v + 1.0 s;
}

param src: Length = 10.0 m;
node y: Length = @bogus(v: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "expected DimensionMismatch from inside dag body, got: {err:?}"
    );
}

#[test]
fn inline_dag_indexed_output_type_flows_through() {
    let source = "\
pub index Region = { A, B };

dag doubler {
    import test.{ Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region.A: 1.0 m, Region.B: 3.0 m };
node out: Length = @doubler(v: @dist).result[Region.A];
";
    let types = check(source).unwrap();
    let length = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    assert_eq!(
        types[&ScopedName::local("out")],
        DeclaredType::Scalar(length)
    );
}

#[test]
fn inline_dag_projection_requires_pub() {
    // Projecting a non-`pub` body node is rejected with the same error
    // shape as `include lib_dag(...) { private_result }`.
    let source = "\
dag private_result {
    param v: Length;
    node hidden: Length = @v;
}

param src: Length = 10.0 m;
node y: Length = @private_result(v: @src).hidden;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ImportPrivateItem { .. }),
        "expected ImportPrivateItem for non-pub projection, got: {err:?}"
    );
}

#[test]
fn inline_dag_pub_bind_on_node_rejected_at_parse() {
    // `pub(bind)` on a node is not meaningful — `param` is how you declare
    // a bindable input. The parser rejects this at parse time.
    let source = "\
dag broken {
    param v: Length;
    pub(bind) node result: Length = @v;
}
";
    assert!(Parser::new(source).parse_file().is_err());
}

#[test]
fn inline_dag_self_recursive_cycle_detected() {
    let source = "\
dag loop_self {
    param v: Length;
    pub node result: Length = @loop_self(v: @v).result;
}

param src: Length = 1.0 m;
node y: Length = @loop_self(v: @src).result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::CyclicDependency { .. }),
        "expected CyclicDependency, got: {err:?}"
    );
}

#[test]
fn inline_dag_mutual_recursion_cycle_detected() {
    let source = "\
dag a {
    param v: Length;
    pub node out: Length = @b(v: @v).out;
}

dag b {
    param v: Length;
    pub node out: Length = @a(v: @v).out;
}

param src: Length = 1.0 m;
node y: Length = @a(v: @src).out;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::CyclicDependency { .. }),
        "expected CyclicDependency, got: {err:?}"
    );
}

#[test]
fn inline_dag_body_forward_reference_resolves() {
    // A dag body that references a later node — formerly broken at eval
    // (source-order walk), now works because the dag body is compiled
    // through the same IR path as a file and gets topological ordering.
    let source = "\
dag forward {
    param v: Length;
    pub node b: Length = @a;
    node a: Length = @v;
}

param src: Length = 10.0 m;
node y: Length = @forward(v: @src).b;
";
    // Phase B only covers compile; actual runtime topo-sort is Phase C.
    // Still, compile must accept this program (no dim errors).
    check(source).unwrap();
}

#[test]
fn int_exponent_beyond_i32_is_rejected() {
    // Scalar `^` requires a float-literal exponent, so an Int-typed
    // exponent is rejected by the rhs scalar check before the exponent arm.
    // The arm's former `as i32` wrap is additionally hardened to a
    // DimensionOverflow error, so a huge exponent can never silently wrap
    // even if that earlier check changes.
    let source = "node x: Dimensionless = (2.0 m) ^ 4294967296;";
    assert!(check(source).is_err());
    let negative = "node x: Dimensionless = (2.0 m) ^ -4294967296;";
    assert!(check(negative).is_err());
}

#[test]
fn float_exponent_beyond_i32_errors_with_overflow() {
    // The float-literal arm saturates at i32::MAX before `pow`, which must
    // surface as DimensionOverflow rather than a wrong dimension.
    let source = "node x: Dimensionless = (2.0 m) ^ 4294967296.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionOverflow { .. }),
        "expected DimensionOverflow, got: {err:?}"
    );
}

#[test]
fn negating_a_bool_is_rejected() {
    // Regression: the HIR inference engine accepted `-` on Bool while the
    // syntax-AST engine rejected it — a live divergence between the two,
    // and declaration bodies route through the HIR path.
    let source = "node x: Bool = -(1.0 > 2.0);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "expected DimensionMismatch, got: {err:?}"
    );
}
