use super::*;
use crate::dimension::BaseDimId;
use crate::registry::declared_type::{DeclaredGenericArg, IndexTypeRef, StructTypeRef};
use crate::syntax::decl_name::DeclName;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::parser::Parser;
use crate::syntax::span::Span;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test.gcl", Arc::new(source.to_string()))
}

fn test_dag_id() -> crate::dag_id::DagId {
    crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap()
}

fn test_index_ref(name: &str) -> IndexTypeRef {
    IndexTypeRef::with_owner(
        test_dag_id(),
        crate::syntax::index_name::IndexName::expect_valid(name.to_string()),
    )
}

fn check(source: &str) -> Result<HashMap<ScopedName, DeclaredType>, GraphcalError> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = desugared;
    let src = make_src(source);
    let (ir, parent_registry) =
        crate::ir::lower::lower_with_frontend_registry_for_test(&file, &src)?;
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
        if let crate::desugar::desugared_ast::DeclKind::Dag(dag) = &decl.kind {
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
    let mut project_types = crate::tir::typed::ProjectTypeStore::default();
    project_types
        .insert_graphcal_prelude()
        .map_err(|err| GraphcalError::InternalError {
            message: format!("test module type prelude failed: {err}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    project_types
        .insert_local_hir(&ir)
        .map_err(|error| GraphcalError::InternalError {
            message: format!("test HIR type store failed: {error}"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    let mut builder = crate::tir::typed::type_resolve_builder_with_modules_and_cancellation(
        ir,
        &src,
        &resolver,
        &project_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )?;
    compile_inline_dag_bodies_test(
        &mut builder,
        &src,
        &parent_dag_id,
        &file.declarations,
        &parent_registry,
    )?;
    let mut tir = builder.finish();
    check_dimensions_tir(&mut tir, &src)?;
    tir.build_declared_types(&src)
}

fn module_aware_tir(source: &str) -> (crate::tir::typed::TIR, NamedSource<Arc<String>>) {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = desugared;
    let src = make_src(source);
    let ir = crate::ir::lower::lower(&file, &src).unwrap();
    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    resolver
        .add_module(ir.dag_id().clone(), &file.declarations)
        .unwrap();
    let mut project_types = crate::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_local_hir(&ir).unwrap();
    let tir =
        crate::tir::typed::type_resolve_with_modules(ir, &src, &resolver, &project_types).unwrap();
    (tir, src)
}

fn model_port_application(
    source: &str,
) -> (
    crate::tir::typed::TIR,
    NamedSource<Arc<String>>,
    StructTypeRef,
    Vec<DeclaredGenericArg>,
) {
    let (tir, src) = module_aware_tir(source);
    let declared_types = tir.build_declared_types(&src).unwrap();
    let DeclaredType::Struct(identity, generic_args) =
        &declared_types[&ScopedName::parse("port").unwrap()]
    else {
        panic!("expected `port` to be a concrete model struct");
    };
    (tir, src, identity.clone(), generic_args.clone())
}

/// Compile each inline dag body in `tir` with no self-import preprocessing.
/// Used by compiler-side integration tests that don't have access to the
/// eval crate's project pipeline.
fn compile_inline_dag_bodies_test(
    tir: &mut crate::tir::typed::TirBuilder,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
    parent_declarations: &[crate::desugar::desugared_ast::Declaration],
    parent_registry: &crate::registry::types::Registry,
) -> Result<(), GraphcalError> {
    let dag_bodies = parent_declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            crate::desugar::desugared_ast::DeclKind::Dag(dag) => {
                Some((dag.name.value.clone(), dag.body.clone()))
            }
            _ => None,
        })
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
            if let crate::desugar::desugared_ast::DeclKind::Import(import) = &decl.kind {
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
    let mut project_types = tir.project_type_store().clone();

    for (name, body) in dag_bodies {
        let dag_body_ir = crate::ir::lower::lower_dag_body_to_ir(
            name.as_str(),
            &body,
            parent_registry,
            &resolver,
            &crate::ir::resolve::ImportedValueNames::default(),
            HashMap::new(),
            src,
            parent_dag_id,
        )?;
        project_types
            .insert_local_hir(&dag_body_ir)
            .map_err(|error| GraphcalError::InternalError {
                message: format!("test inline HIR type store failed: {error}"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        let compiled_dag = crate::tir::typed::type_resolve_single_with_modules(
            dag_body_ir,
            src,
            &resolver,
            &project_types,
        )?;
        tir.insert_dag(compiled_dag)
            .map_err(|error| GraphcalError::InternalError {
                message: error.to_string(),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
    }
    Ok(())
}

#[test]
fn override_dependency_summary_collects_each_default_nominal_use_once() {
    let source = r"
pub(bind) type Record { Record(x: Dimensionless) }
pub type Fixed { Fixed(x: Dimensionless) }
pub(bind) index Axis = { A, B };
pub index FixedAxis = { A, B };
pub type Wrapper<T: Type, I: Index> { Wrapper(value: T, values: Dimensionless[I]) }

param record: Record = Record(x: 1.0);
param fixed: Fixed = Fixed(x: 2.0);
param values: Dimensionless[Axis] = { Axis.A: 3.0, Axis.B: 4.0 };
param fixed_values: Dimensionless[FixedAxis] = {
    FixedAxis.A: 5.0,
    FixedAxis.B: 6.0,
};
param dependent: Dimensionless = @record.x + @fixed.x + match Axis.A {
    Axis.A => 1.0,
    Axis.B => 2.0,
};
param wrapped: Wrapper<Record, Axis> =
    Wrapper<Record, Axis>(value: @record, values: @values);
param fixed_wrapped: Wrapper<Fixed, FixedAxis> =
    Wrapper<Fixed, FixedAxis>(value: @fixed, values: @fixed_values);
";
    let (mut tir, src) = module_aware_tir(source);
    check_dimensions_tir(&mut tir, &src).unwrap();

    let summary = collect_override_dependency_summary(&tir, &src).unwrap();
    let owner = test_dag_id();
    let record = NominalOverrideIdentity::Type(ResolvedStructTypeName::from_def(
        owner.clone(),
        crate::syntax::type_name::StructTypeName::expect_valid("Record"),
    ));
    let axis = NominalOverrideIdentity::Index(ResolvedIndexName::from_def(
        owner.clone(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    ));
    let dependencies = |param: &str| {
        summary.get(&ResolvedDeclName::from_def(
            owner.clone(),
            DeclName::expect_valid(param),
        ))
    };

    assert_eq!(
        dependencies("record"),
        Some(&HashSet::from([record.clone()]))
    );
    assert_eq!(dependencies("values"), Some(&HashSet::from([axis.clone()])));
    assert_eq!(
        dependencies("dependent"),
        Some(&HashSet::from([record.clone(), axis.clone()]))
    );
    assert_eq!(
        dependencies("wrapped"),
        Some(&HashSet::from([record, axis]))
    );
    assert_eq!(dependencies("fixed"), None);
    assert_eq!(dependencies("fixed_values"), None);
    assert_eq!(dependencies("fixed_wrapped"), None);
}

#[test]
fn override_dependency_summary_observes_cancellation() {
    let source = r"
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record = Record(x: 1.0);
";
    let (tir, src) = module_aware_tir(source);
    let cancellation = crate::cancellation::CancellationSource::new();
    cancellation.cancel();

    assert!(matches!(
        collect_override_dependency_summary_with_cancellation(&tir, &src, &cancellation.token(),),
        Err(GraphcalError::Cancelled(_))
    ));
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

    let a = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("a"));
    let b = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("b"));
    let x = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("x"));
    let y = ResolvedDeclName::from_def(dag_id, DeclName::expect_valid("y"));

    let mut resolved = crate::tir::typed::ResolvedDagDependencies::default();
    resolved.const_deps.insert(a.clone(), BTreeSet::new());
    resolved.const_deps.insert(b, BTreeSet::from([a]));
    resolved.runtime_deps.insert(x.clone(), BTreeSet::new());
    resolved.runtime_deps.insert(y, BTreeSet::from([x]));

    tir.root_mut().semantic.dependencies = resolved;

    check_dimensions_tir(&mut tir, &src).unwrap();
}

#[test]
fn node_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir("node y: Dimensionless = sqrt(4.0);");
    tir.root_mut().nodes[0].expr.kind =
        crate::hir::ExprKind::StringLiteral("not dimensionless".to_string());

    assert!(check_dimensions_tir(&mut tir, &src).is_err());
}

#[test]
fn indexed_node_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir(
        "index Phase = { Burn };\n\
         node y: Dimensionless[Phase] = for p: Phase { match p { Phase.Burn => 1.0 } };",
    );
    tir.root_mut().nodes[0].expr.kind =
        crate::hir::ExprKind::StringLiteral("not indexed".to_string());

    assert!(check_dimensions_tir(&mut tir, &src).is_err());
}

#[test]
fn assert_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir("assert ok = sqrt(4.0) == 2.0;");
    let span = tir.root().asserts[0].span;
    tir.root_mut().asserts[0].body = crate::hir::AssertBody::Expr(crate::hir::Expr::new(
        crate::hir::ExprKind::StringLiteral("not bool".to_string()),
        span,
    ));

    assert!(check_dimensions_tir(&mut tir, &src).is_err());
}

#[test]
fn check_dimensionless_const() {
    let types = check("const node g0: Dimensionless = 9.80665;").unwrap();
    assert_eq!(
        types[&ScopedName::parse("g0").unwrap()],
        DeclaredType::Quantity(Dimension::dimensionless())
    );
}

#[test]
fn check_dimensionless_arithmetic() {
    let types = check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
    assert_eq!(
        types[&ScopedName::parse("y").unwrap()],
        DeclaredType::Quantity(Dimension::dimensionless())
    );
}

#[test]
fn check_length_quantity_literal() {
    let types = check("param alt: Length = 400.0 km;").unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::parse("alt").unwrap()],
        DeclaredType::Quantity(length)
    );
}

#[test]
fn check_velocity_from_division() {
    let source = "param dist: Length = 100.0 km;\nparam time: Time = 2.0 h;\nnode speed: Velocity = @dist / @time;";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    )))
    .unwrap();
    assert_eq!(
        types[&ScopedName::parse("speed").unwrap()],
        DeclaredType::Quantity(velocity)
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
fn check_expected_fail_rejects_variant_key_on_unindexed_assertion() {
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
        "param speed: Velocity = 100.0 m / s;\nnode speed_kmh: Velocity = @speed -> km / h;";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    )))
    .unwrap();
    assert_eq!(
        types[&ScopedName::parse("speed_kmh").unwrap()],
        DeclaredType::Quantity(velocity)
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
    let source = "param r: Length = 5.0 m;\nnode area: Area = @r ^ 2;";
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
    let velocity = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    )))
    .unwrap();
    assert_eq!(
        types[&ScopedName::parse("dv").unwrap()],
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Quantity(velocity)),
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
fn incomplete_large_axis_map_reports_one_bounded_missing_witness() {
    use std::fmt::Write as _;

    const AXIS_COUNT: usize = 19;
    let mut source = String::new();
    for axis in 0..AXIS_COUNT {
        writeln!(source, "pub index A{axis} = {{ X, Y }};").unwrap();
    }
    let axes = (0..AXIS_COUNT)
        .map(|axis| format!("A{axis}"))
        .collect::<Vec<_>>()
        .join(", ");
    let tuple = (0..AXIS_COUNT)
        .map(|axis| format!("A{axis}.X"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "param values: Dimensionless[{axes}] = {{ ({tuple}): 1.0 }};"
    )
    .unwrap();

    let error = check(&source).unwrap_err();
    assert!(
        matches!(&error, GraphcalError::EvalError { message, .. }
            if message.contains("missing 524287 entries")
                && message.contains("first missing entry")
                && message.contains("A18.Y")),
        "got: {error:?}"
    );
}

#[test]
fn map_key_space_overflow_is_preempted_by_the_eager_shape_policy() {
    use std::fmt::Write as _;

    let axis_count = usize::BITS as usize;
    let mut source = String::new();
    for axis in 0..axis_count {
        writeln!(source, "pub index A{axis} = {{ X, Y }};").unwrap();
    }
    let axes = (0..axis_count)
        .map(|axis| format!("A{axis}"))
        .collect::<Vec<_>>()
        .join(", ");
    let tuple = (0..axis_count)
        .map(|axis| format!("A{axis}.X"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "param values: Dimensionless[{axes}] = {{ ({tuple}): 1.0 }};"
    )
    .unwrap();

    let error = check(&source).unwrap_err();
    assert!(
        matches!(
            &error,
            GraphcalError::MaterializedShapeTooLarge {
                maximum: 1_000_000,
                ..
            }
        ),
        "got: {error:?}"
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
fn check_product_and_rss_aggregation_dimensions() {
    let source = "\
pub index Factor = { A, B, C };
param lengths: Length[Factor] = {
    Factor.A: 2.0 m,
    Factor.B: 3.0 m,
    Factor.C: 4.0 m,
};
node volume: Volume = product(@lengths);
node root_sum_square: Length = rss(@lengths);";
    check(source).unwrap();
}

#[test]
fn check_count_aggregation_returns_int_for_non_quantity_elements() {
    let source = "\
pub index Case = { First, Second, Third };
node flags: Bool[Case] = for case: Case { true };
node n: Int = count(@flags);";
    check(source).unwrap();
}

#[test]
fn check_count_no_longer_returns_dimensionless_quantity() {
    let source = "\
pub index Case = { First, Second };
node values: Bool[Case] = for case: Case { true };
node n: Dimensionless = count(@values);";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::DimensionMismatchInAnnotation { .. }
    ));
}

#[test]
fn check_count_rejects_multi_axis_input() {
    let source = "\
pub index Row = { A, B };
pub index Column = { X, Y, Z };
node matrix: Bool[Row, Column] = for row: Row, column: Column { true };
node n: Int = count(@matrix);";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::MultiAxisAggregation { rank: 2, .. }
    ));
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
fn check_minimum_maximum_aggregations() {
    let source = "\
pub index Case = { Low, High };
param values: Length[Case] = {
Case.Low: 1.0 m,
Case.High: 2.0 m,
};
node low: Length = minimum(@values);
node high: Length = maximum(@values);";
    check(source).unwrap();
}

#[test]
fn reduction_functions_have_fixed_arity() {
    let err = check("node x: Dimensionless = minimum(1.0, 2.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn zero_argument_arity_error_points_at_the_call() {
    let source = "node x: Dimensionless = sin();";
    let error = check(source).unwrap_err();
    let GraphcalError::WrongArity { span, .. } = error else {
        panic!("expected wrong-arity diagnostic");
    };
    assert_eq!(span.offset(), source.find("sin").unwrap());
    assert!(!span.is_empty());
}

#[test]
fn check_linear_algebra_preserves_axes_and_dimensions() {
    let source = "\
param a: Length[Fin(2), Fin(3)] = for i: Fin(2), j: Fin(3) { 1.0 m };
param b: Time[Fin(3), Fin(4)] = for i: Fin(3), j: Fin(4) { 1.0 s };
node product: Length * Time[Fin(2), Fin(4)] = matmul(@a, @b);
node transposed: Length[Fin(3), Fin(2)] = transpose(@a);";
    check(source).unwrap();
}

#[test]
fn check_algorithmic_linear_algebra_dimensions() {
    let source = "\
param a: Length[Fin(2), Fin(2)] = for i: Fin(2), j: Fin(2) { 1.0 m };
param b: Area[Fin(2)] = for i: Fin(2) { 1.0 m^2 };
node solution: Length[Fin(2)] = solve(@a, @b);
node inverse_a: Length^-1[Fin(2), Fin(2)] = inverse(@a);
node determinant_a: Area = det(@a);";
    check(source).unwrap();
}

#[test]
fn check_linear_algebra_rejects_distinct_contraction_axes() {
    let source = "\
pub index Left = { L1, L2 };
pub index Right = { R1, R2 };
param a: Dimensionless[Left] = for i: Left { 1.0 };
param b: Dimensionless[Right] = for i: Right { 1.0 };
node result: Dimensionless = dot(@a, @b);";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::LinearAlgebraShapeMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn check_cross_requires_three_component_axis() {
    let source = "\
param a: Dimensionless[Fin(2)] = for i: Fin(2) { 1.0 };
param b: Dimensionless[Fin(2)] = for i: Fin(2) { 2.0 };
node result: Dimensionless[Fin(2)] = cross(@a, @b);";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::LinearAlgebraShapeMismatch { .. }
    ));
}

#[test]
fn check_linear_algebra_rejects_non_quantity_elements() {
    let source = "\
param flags: Bool[Fin(2)] = for i: Fin(2) { true };
node result: Dimensionless = norm(@flags);";
    let error = check(source).unwrap_err();
    assert!(matches!(error, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn linear_algebra_functions_have_fixed_arity() {
    let error = check("node x: Dimensionless = dot(1.0);").unwrap_err();
    assert!(matches!(error, GraphcalError::WrongArity { .. }));
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
fn check_scan_supports_heterogeneous_accumulator() {
    let source = "\
pub index Flag = { A, B };
param flags: Bool[Flag] = {
Flag.A: true,
Flag.B: false,
};
node count_true: Int[Flag] = scan(
    @flags,
    0,
    |count, flag| if flag { count + 1 } else { count }
);";
    check(source).unwrap();
}

#[test]
fn scan_accepts_indexed_accumulator_and_prepends_source_axis() {
    let source = "\
index Element = { A, B };
index Step = { First, Second };
node input: Dimensionless[Step] = for step: Step { 1.0 };
node initial: Dimensionless[Element] = for element: Element { 0.0 };
node state: Dimensionless[Step, Element] = scan(
    @input,
    @initial,
    |previous, item| for element: Element { previous[element] + item }
);";
    check(source).unwrap();
}

#[test]
fn scan_rejects_multi_axis_source_instead_of_choosing_an_axis_implicitly() {
    let source = include_str!("../../../../../tests/fixtures/invalid/scan_multi_axis_source.gcl");
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::MultiAxisScanSource { rank: 2, .. }),
        "got: {err:?}"
    );
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

// --- Comparison rules ---

#[test]
fn check_comparison_rejects_indexed_operands_for_every_operator() {
    let prefix = "\
index Case = { A, B };
node values: Length[Case] = {
Case.A: 1.0 m,
Case.B: 2.0 m,
};";

    for op in ["==", "!=", "<", "<=", ">", ">="] {
        for expr in [
            format!("@values {op} 1.0 m"),
            format!("1.0 m {op} @values"),
            format!("@values {op} @values"),
        ] {
            let source = format!("{prefix}\nnode bad: Bool = {expr};");
            let err = check(&source).unwrap_err();
            assert!(
                matches!(
                    &err,
                    GraphcalError::IndexedComparisonOperand { found, .. }
                        if found == "Length[Case]"
                ),
                "operator `{op}` in `{expr}` produced: {err:?}"
            );
        }
    }
}

#[test]
fn check_explicit_for_comparison_of_indexed_values() {
    let source = "\
index Case = { A, B };
node lhs: Length[Case] = { Case.A: 1.0 m, Case.B: 2.0 m };
node rhs: Length[Case] = { Case.A: 1.0 m, Case.B: 2.5 m };
node same: Bool[Case] = for case: Case { @lhs[case] == @rhs[case] };
node below: Bool[Case] = for case: Case { @lhs[case] < 3.0 m };";
    check(source).unwrap();
}

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
fn check_power_runtime_exponent_dimensioned_base() {
    let source = "\
param x: Length = 1.0 m;
param n: Dimensionless = 2.0;
node bad: Area = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::RuntimeExponentForDimensionedBase { .. }),
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
fn check_power_float_syntax_on_dimensioned_base_has_exact_replacement() {
    let source = "param x: Length = 1.0 m;\nnode bad: Length^(1/4) = @x ^ 0.25;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::FloatPowerExponent {
                replacement: Some(ref replacement),
                ..
            } if replacement == "(1/4)"
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_power_integral_float_syntax_suggests_integer() {
    let source = "param x: Length = 1.0 m;\nnode bad: Area = @x ^ 2.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::FloatPowerExponent {
                replacement: Some(ref replacement),
                ..
            } if replacement == "2"
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_power_dimensioned_exponent_uses_d001() {
    let source = "\
param x: Dimensionless = 2.0;
param n: Length = 1.0 m;
node bad: Dimensionless = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn hir_normalizes_omitted_dimension_and_unit_powers() {
    let (tir, _) = module_aware_tir("param distance: Length = 1.0 m;");
    let param = tir.root().params().first().unwrap();
    let crate::hir::TypeExprKind::DimExpr(dimension) = &param.type_ann.type_expr.kind else {
        panic!("expected dimension expression");
    };
    assert_eq!(
        dimension.terms[0].term.power,
        crate::dimension::Rational::ONE
    );

    let expression = param.default_expr.as_ref().unwrap();
    let crate::hir::ExprKind::QuantityLiteral { unit, .. } = &expression.kind else {
        panic!("expected quantity literal");
    };
    assert_eq!(unit.terms[0].power, crate::dimension::Rational::ONE);
}

#[test]
fn hir_preserves_exact_power_metadata() {
    let (tir, _) = module_aware_tir("param x: Length = 4.0 m;\nnode y: Length^(3/2) = @x ^ (3/2);");
    let expression = &tir.root().nodes().first().unwrap().expr;
    assert!(matches!(
        expression.kind,
        crate::hir::ExprKind::BinOp {
            op: crate::syntax::ast::BinOp::Pow(
                crate::syntax::ast::PowerExponent::Exact(exponent)
            ),
            ..
        } if exponent == crate::exact_rational::ExactRational::try_new(3, 2).unwrap()
    ));
}

#[test]
fn check_power_arbitrary_exact_rational_exponent() {
    let source = "\
pub dim LengthThreeHalves = Length ^ (3/2);
param x: Length = 4.0 m;
node y: LengthThreeHalves = @x ^ (3/2);";
    check(source).unwrap();
}

#[test]
fn check_power_exact_zero_exponent_is_valid() {
    let source = "param x: Length = 4.0 m;\nnode y: Dimensionless = @x ^ 0;";
    check(source).unwrap();
}

#[test]
fn check_power_dimension_rational_overflow_uses_d010() {
    let source = "param x: Length = 4.0 m;\nnode bad: Dimensionless = @x ^ (1/2147483648);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionOverflow { .. }),
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
node y: InvLengthSquared = @x ^ -2;";
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
    // `2 ^ 3 ^ 2` parses right-assoc as `2 ^ (3 ^ 2)`. Quantity chains were
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

// --- expect_quantity error: struct used where quantity expected ---

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
fn check_field_access_on_quantity() {
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
fn check_types_match_struct_vs_quantity() {
    // Declared as a struct type but expression evaluates to quantity → mismatch
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
fn check_scan_on_unindexed() {
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
fn check_index_access_on_quantity() {
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

#[test]
fn check_finite_index_constant_index_out_of_bounds() {
    let source = "\
param v: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0; 2.0; 3.0; };
node bad: Dimensionless = @v[5];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("index 5 out of bounds for Fin(3)")),
        "got: {err:?}"
    );
}

#[test]
fn check_finite_index_constant_index_negative() {
    let source = "\
param v: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0; 2.0; 3.0; };
node bad: Dimensionless = @v[0 - 1];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("index expression evaluated to negative value: -1")),
        "got: {err:?}"
    );
}

#[test]
fn check_ambiguous_bare_index_label_surfaces_resolver_error() {
    let source = "\
pub index M = { A };
pub index P = { A };
node x: Dimensionless = A;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("ambiguous index label `A`")),
        "got: {err:?}"
    );
}

#[test]
fn check_prelude_dimension_in_value_position_is_not_unknown_local() {
    let source = "node x: Dimensionless = Length;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message == "dimension `Length` cannot be used as a value"),
        "got: {err:?}"
    );
}

#[test]
fn check_value_name_in_type_position_reports_wrong_universe() {
    let source = "\
node a: Dimensionless = 1.0;
node b: a = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("`a` is a value, not a type")),
        "got: {err:?}"
    );
}

#[test]
fn check_index_label_in_type_position_reports_wrong_universe() {
    let source = "\
pub index M = { A };
param x: M.A = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("`M.A` is an index label, not a type")),
        "got: {err:?}"
    );
}

#[test]
fn check_constructor_name_in_type_position_reports_wrong_universe() {
    let source = "\
type Pos { MkPos(v: Dimensionless) }
param p: MkPos = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("`MkPos` is a constructor, not a type")),
        "got: {err:?}"
    );
}

#[test]
fn check_index_as_type_application_head_reports_wrong_universe() {
    let source = "\
pub index M = { A };
param x: M<Length> = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("`M` is an index, not a type")),
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

// --- Unit constness policies ---

#[test]
fn const_unit_rejects_graph_ref_scale() {
    let source = "\
base dim Money;
base unit USD: Money;
param rate: Dimensionless = 1.08;
const unit EUR: Money = (@rate) USD;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::GraphRefInConstUnit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn dynamic_unit_scale_requires_scalar_dimensionless_quantity() {
    for factor in [
        "param factor: Length = 2.0 m;",
        "param factor: Bool = true;",
        "param factor: Int = 2;",
        "pub index Case = { A, B };\nparam factor: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.0 };",
    ] {
        let source = format!(
            "base dim Money;\nbase unit USD: Money;\n{factor}\nunit EUR: Money = (@factor) USD;"
        );
        let err = check(&source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DynamicUnitScaleTypeMismatch { .. }),
            "got: {err:?}"
        );
    }
}

#[test]
fn dynamic_unit_scale_accepts_scalar_dimensionless_quantity() {
    check(
        "base dim Money;\nbase unit USD: Money;\nparam factor: Dimensionless = 1.08;\nunit EUR: Money = (@factor) USD;",
    )
    .unwrap();
}

#[test]
fn const_unit_rejects_runtime_unit_reference() {
    let source = "\
unit mile: Length = 1609.344 m;
const unit double_mile: Length = 2.0 mile;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonConstUnitInConst { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_node_rejects_runtime_quantity_literal() {
    let source = "\
unit mile: Length = 1609.344 m;
const node distance: Length = 1.0 mile;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonConstUnitInConst { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_node_rejects_runtime_unit_in_domain_bound() {
    let source = "\
unit mile: Length = 1609.344 m;
const node distance: Length(min: 1.0 mile) = 1609.344 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonConstUnitInConst { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_node_accepts_const_quantity_literal() {
    let source = "\
const unit mile: Length = 1609.344 m;
const node distance: Length = 1.0 mile;";
    check(source).unwrap();
}

#[test]
fn const_node_rejects_runtime_conversion_target() {
    let source = "\
unit mile: Length = 1609.344 m;
const node distance: Length = 1609.344 m -> mile;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonConstUnitInConst { .. }),
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
    // i : Fin(3) indexing into D[Fin(3)] — 3 <= 3 — safe
    let source = "\
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
node w: Dimensionless[Fin(3)] = for i: Fin(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_smaller_bound_indexing() {
    // i : Fin(3) indexing into D[Fin(5)] — 3 <= 5 — safe
    let source = "\
param v: Dimensionless[Fin(5)] = for i: Fin(5) { 1.0 };
node w: Dimensionless[Fin(3)] = for i: Fin(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_out_of_bounds() {
    // i : Fin(5) indexing into D[Fin(3)] — 5 > 3 — compile error
    let source = "\
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
node w: Dimensionless[Fin(5)] = for i: Fin(5) { @v[i] };";
    let err = check(source).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("IndexMismatch"),
        "expected an index-identity error, got: {msg}"
    );
}

#[test]
fn quantity_local_cannot_index_named_indexed_value() {
    let source = "\
pub index Phase = { A };
pub index TimeStep = range(0.0 s, 1.0 s, step: 1.0 s);
param v: Dimensionless[Phase] = { Phase.A: 1.0 };
node w: Dimensionless[TimeStep] = for t: TimeStep { @v[t] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::IndexMismatch { expected, found, .. } if expected.as_str() == "Phase" && found.as_str() == "TimeStep"),
        "got: {err:?}"
    );
}

#[test]
fn range_loop_var_cannot_index_different_range_indexed_value() {
    let source = "\
pub index TimeGrid = range(0.0 s, 2.0 s, step: 1.0 s);
pub index LenGrid = range(0.0 m, 2.0 m, step: 1.0 m);
param v: Dimensionless[LenGrid] = for x: LenGrid { 1.0 };
node w: Dimensionless[TimeGrid] = for t: TimeGrid { @v[t] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::IndexMismatch { expected, found, .. } if expected.as_str() == "LenGrid" && found.as_str() == "TimeGrid"),
        "got: {err:?}"
    );
}

#[test]
fn unfold_uses_explicit_coordinate_axis_and_previous_state() {
    let source = "\
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node distance: Length[Step] = unfold(
    Step,
    1.0 m,
    |prev_distance, prev_t, t| prev_distance + (2.0 m/s) * (coord(t) - coord(prev_t))
);";
    check(source).unwrap();
}

#[test]
fn unfold_accepts_indexed_state_and_prepends_coordinate_axis() {
    let source = include_str!("../../../../../tests/fixtures/valid/indexed_state_recurrence.gcl");
    check(source).unwrap();
}

#[test]
fn unfold_accepts_matrix_state() {
    let source = "\
index Row = { R1, R2 };
index Column = { C1, C2 };
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node initial: Dimensionless[Row, Column] =
    for row: Row, column: Column { 1.0 };
node state: Dimensionless[Step, Row, Column] = unfold(
    Step,
    @initial,
    |previous, previous_t, t| for row: Row, column: Column {
        previous[row, column] + (coord(t) - coord(previous_t)) / 1.0 s
    }
);";
    check(source).unwrap();
}

#[test]
fn unfold_indexed_state_annotation_mismatch_uses_source_axis_order() {
    let source = "\
index Element = { A, B };
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node initial: Dimensionless[Element] = for element: Element { 1.0 };
node state: Dimensionless[Element, Step] = unfold(
    Step,
    @initial,
    |previous, previous_t, t| for element: Element { previous[element] }
);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            &err,
            GraphcalError::DimensionMismatchInAnnotation {
                declared,
                inferred,
                ..
            } if declared == "Dimensionless[Element, Step]"
                && inferred == "Dimensionless[Step, Element]"
        ),
        "got: {err:?}"
    );
}

#[test]
fn unfold_rejects_indexed_body_with_different_state_axis() {
    let source = "\
index Element = { A, B };
index Other = { A, B };
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node initial: Dimensionless[Element] = for element: Element { 1.0 };
node state: Dimensionless[Step, Element] = unfold(
    Step,
    @initial,
    |previous, previous_t, t| for other: Other { 1.0 }
);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            &err,
            GraphcalError::DimensionMismatch {
                expected,
                found,
                ..
            } if expected == "Dimensionless[Element]"
                && found == "Dimensionless[Other]"
        ),
        "got: {err:?}"
    );
}

#[test]
fn unfold_rejects_non_coordinate_axis() {
    let source = "\
index Phase = { Start, End };
node values: Dimensionless[Phase] = unfold(
    Phase,
    1.0,
    |prev_value, prev_phase, phase| prev_value + 1.0
);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::EvalError { message, .. } if message.contains("unfold requires a coordinate index")),
        "got: {err:?}"
    );
}

#[test]
fn unfold_init_self_reference_is_cycle() {
    let source = "\
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node y: Dimensionless[Step] = unfold(Step, sum(@y), |prev_y, p, t| prev_y + 1.0);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::CyclicDependency { .. }),
        "expected CyclicDependency, got: {err:?}"
    );
}

#[test]
fn unfold_body_self_references_are_cycles_for_every_coordinate() {
    for self_read in ["@y[prev_t]", "@y[t]", "@y[t + (1.0 s)]"] {
        let source = format!(
            "index Step = range(0.0 s, 2.0 s, step: 1.0 s);\n\
             node y: Dimensionless[Step] = unfold(\n\
                 Step,\n\
                 1.0,\n\
                 |prev_y, prev_t, t| {self_read} + 1.0\n\
             );"
        );
        let err = check(&source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::CyclicDependency { .. }),
            "expected CyclicDependency for `{self_read}`, got: {err:?}"
        );
    }
}

#[test]
fn negation_rejects_fin_index_variable() {
    let source = "\
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
node w: Dimensionless[Fin(3)] = for i: Fin(3) { @v[-i] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::DimensionMismatch { found, .. } if found.contains("Fin")),
        "got: {err:?}"
    );
}

#[test]
fn negation_rejects_datetime() {
    let source = "node t: Datetime<UTC> = -datetime(\"2026-01-01T00:00:00Z\");";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::DimensionMismatch { found, .. } if found.contains("Datetime")),
        "got: {err:?}"
    );
}

#[test]
fn aggregation_rejects_non_quantity_elements() {
    let source = "\
pub index Phase = { A, B };
param flags: Bool[Phase] = for p: Phase { true };
node total: Dimensionless = sum(@flags);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::DimensionMismatch { expected, .. } if expected == "indexed quantity collection"),
        "got: {err:?}"
    );
}

#[test]
fn aggregation_rejects_int_elements() {
    let source = "\
param counts: Int[Fin(3)] = for i: Fin(3) { i };
node total: Int = sum(@counts);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, GraphcalError::DimensionMismatch { expected, .. } if expected == "indexed quantity collection"),
        "got: {err:?}"
    );
}

#[test]
fn fin_comparison_same_range() {
    // i : Fin(3), j : Fin(3) — i == j is valid
    let source = "\
node m: Dimensionless[Fin(3), Fin(3)] = for i: Fin(3), j: Fin(3) {
    if i == j { 1.0 } else { 0.0 }
};";
    check(source).unwrap();
}

#[test]
fn fin_arithmetic_with_int() {
    // A Fin loop variable is a key: integer use goes through to_int().
    let source = "\
node v: Dimensionless[Fin(3)] = for i: Fin(3) { to_float(to_int(i)) };";
    check(source).unwrap();
}

// -----------------------------------------------------------------------
// Domain constraint bound dimensions (#438)
// -----------------------------------------------------------------------

#[test]
fn domain_bound_quantity_literal_matches() {
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
    // Integer literal on a dimensioned quantity should also be rejected.
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
// Int domain bounds must remain exact Int values (#439, #958)
// -----------------------------------------------------------------------

#[test]
fn int_domain_bound_int_literal_accepted() {
    let source = "param n: Int(min: 1, max: 100) = 5;";
    check(source).unwrap();
}

#[test]
fn int_domain_bound_dimensionless_quantity_rejected() {
    let source = "param n: Int(min: 0.0, max: 100.0) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundTypeMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_with_unit_rejected() {
    let source = "param n: Int(min: 1.0 kg, max: 10.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundTypeMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_arithmetic_with_unit_rejected() {
    // Arithmetic that produces a dimensioned result is also rejected.
    let source = "param n: Int(min: 1.0 m / 1.0 s) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundTypeMismatch { .. }),
        "got: {err:?}"
    );
}

// -----------------------------------------------------------------------
// Datetime domain bound types (#958)
// -----------------------------------------------------------------------

#[test]
fn datetime_domain_bounds_accept_the_exact_target_scale() {
    let source = r#"
param utc: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
) = datetime("2024-06-01T00:00:00Z");
param tt: Datetime<TT>(
    min: epoch<TT>("2024-01-01T00:00:00"),
    max: epoch<TT>("2024-12-31T23:59:59"),
) = epoch<TT>("2024-06-01T00:00:00");
"#;
    check(source).unwrap();
}

#[test]
fn datetime_domain_bound_rejects_a_different_scale() {
    let source = r#"
param event: Datetime<TT>(min: datetime("2024-01-01T00:00:00Z")) =
    epoch<TT>("2024-06-01T00:00:00");
"#;
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::DatetimeDomainBoundTypeMismatch { .. }
    ));
}

#[test]
fn datetime_domain_bound_accepts_an_explicit_scale_conversion() {
    let source = r#"
param event: Datetime<TT>(min: to_tt(datetime("2024-01-01T00:00:00Z"))) =
    epoch<TT>("2024-06-01T00:00:00");
"#;
    check(source).unwrap();
}

#[test]
fn datetime_domain_bound_rejects_a_non_datetime_value() {
    let source = r#"param event: Datetime(min: 0) = datetime("2024-01-01T00:00:00Z");"#;
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::DatetimeDomainBoundTypeMismatch { .. }
    ));
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
        matches!(err, GraphcalError::IntDomainBoundTypeMismatch { .. }),
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

// -----------------------------------------------------------------------
// Concrete obligations from generic field constraints
// -----------------------------------------------------------------------

#[test]
fn generic_dimension_field_bound_is_checked_after_substitution() {
    let source = r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Box<Time> = Box<Time>(x: 1.0 s);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn temporary_generic_constructor_obligation_is_checked() {
    let source = r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Time = Box<Time>(x: 1.0 s).x;
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn matching_generic_dimension_field_bound_passes() {
    let source = r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node good: Box<Length> = Box<Length>(x: 1.0 m);
";
    check(source).unwrap();
}

#[test]
fn generic_dimension_expression_field_bound_is_checked_after_substitution() {
    let source = r"
type Squared<D: Dim> { Squared(x: D^2(min: 0.5 m^2)) }
node bad: Squared<Time> = Squared<Time>(x: 1.0 s^2);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn nested_generic_field_obligation_is_checked() {
    let source = r"
type Inner<D: Dim> { Inner(x: D(min: 0.5 m)) }
type Outer<D: Dim> { Outer(inner: Inner<D>) }
node bad: Outer<Time> = Outer<Time>(inner: Inner<Time>(x: 1.0 s));
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn defaulted_generic_field_obligation_is_checked() {
    let source = r"
type Box<D: Dim = Time> { Box(x: D(min: 0.5 m)) }
node bad: Box = Box(x: 1.0 s);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {error:?}"
    );
}

#[test]
fn model_schema_rejects_undischarged_generic_field_obligation() {
    let source = r"
pub type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
param port: Box<Time>;
";
    let (tir, src, identity, generic_args) = model_port_application(source);

    let error = ConcreteModelType::try_new(&tir, &identity, &generic_args, &src).unwrap_err();
    assert!(
        matches!(
            error,
            ConcreteModelTypeError::Compiler(GraphcalError::DomainDimensionMismatch { .. })
        ),
        "got: {error:?}"
    );
}

#[test]
fn model_schema_type_rejects_too_few_and_too_many_args_for_phantom_generic() {
    let source = r"
pub type Phantom<N: Nat> { Phantom }
param port: Phantom<1>;
";
    let (tir, src, identity, generic_args) = model_port_application(source);
    assert_eq!(generic_args.len(), 1);

    let too_few = ConcreteModelType::try_new(&tir, &identity, &[], &src).unwrap_err();
    assert!(matches!(
        too_few,
        ConcreteModelTypeError::GenericArityMismatch {
            expected: 1,
            actual: 0,
            ..
        }
    ));

    let too_many_args = vec![generic_args[0].clone(), generic_args[0].clone()];
    let too_many = ConcreteModelType::try_new(&tir, &identity, &too_many_args, &src).unwrap_err();
    assert!(matches!(
        too_many,
        ConcreteModelTypeError::GenericArityMismatch {
            expected: 1,
            actual: 2,
            ..
        }
    ));
}

#[test]
fn model_schema_type_rejects_wrong_generic_sort_before_expansion() {
    let source = r"
pub type Phantom<N: Nat> { Phantom }
param port: Phantom<1>;
";
    let (tir, src, identity, _) = model_port_application(source);
    let wrong_sort = [DeclaredGenericArg::Type(DeclaredType::Int)];

    let error = ConcreteModelType::try_new(&tir, &identity, &wrong_sort, &src).unwrap_err();
    assert!(matches!(
        error,
        ConcreteModelTypeError::GenericSortMismatch {
            expected: crate::registry::type_def::TypeGenericConstraint::Nat,
            actual: crate::registry::type_def::TypeGenericConstraint::Type,
            ..
        }
    ));
}

#[test]
fn model_schema_type_accepts_complete_defaulted_args_from_compiler() {
    let source = r"
pub type Defaults<N: Nat = 2, T: Type = Int> { Defaults(value: T) }
param port: Defaults;
";
    let (tir, src, identity, generic_args) = model_port_application(source);
    assert_eq!(generic_args.len(), 2);

    let omitted_defaults = ConcreteModelType::try_new(&tir, &identity, &[], &src).unwrap_err();
    assert!(matches!(
        omitted_defaults,
        ConcreteModelTypeError::GenericArityMismatch {
            expected: 2,
            actual: 0,
            ..
        }
    ));

    let model_type = ConcreteModelType::try_new(&tir, &identity, &generic_args, &src).unwrap();
    let constructors = model_type.constructors(&src).unwrap();
    assert_eq!(constructors.len(), 1);
    assert_eq!(constructors[0].fields().len(), 1);
    assert_eq!(
        constructors[0].fields()[0].declared_type(),
        &DeclaredType::Int
    );
}

#[test]
fn model_schema_type_expands_nested_concrete_type_args() {
    let source = r"
pub type Inner<T: Type> { Inner(value: T) }
pub type Outer<T: Type> { Outer(value: Inner<T>) }
param port: Outer<Int>;
";
    let (tir, src, identity, generic_args) = model_port_application(source);
    let model_type = ConcreteModelType::try_new(&tir, &identity, &generic_args, &src).unwrap();
    let constructors = model_type.constructors(&src).unwrap();

    assert!(matches!(
        constructors[0].fields()[0].declared_type(),
        DeclaredType::Struct(_, nested_args)
            if matches!(nested_args.as_slice(), [DeclaredGenericArg::Type(DeclaredType::Int)])
    ));
}

#[test]
fn model_schema_required_indexes_have_distinct_validated_and_concrete_states() {
    let source = r"
pub(bind) index Axis;
pub type Vector<I: Index> { Vector(values: Dimensionless[I]) }
param port: Vector<Axis>;
";
    let (tir, src, identity, generic_args) = model_port_application(source);

    let validated = ValidatedModelType::try_new(&tir, &identity, &generic_args, &src).unwrap();
    assert_eq!(validated.constructors(&src).unwrap().len(), 1);

    let error = ConcreteModelType::try_new(&tir, &identity, &generic_args, &src).unwrap_err();
    assert!(matches!(
        error,
        ConcreteModelTypeError::RequiredIndex { .. }
    ));
}

#[test]
fn symbolic_static_fin_key_obligation_passes_below_cardinality() {
    let source = r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 1)))) }
node good: T<2> = T<2>(x: 1);
";
    check(source).unwrap();
}

#[test]
fn symbolic_static_fin_key_obligation_rejects_equal_cardinality() {
    let source = r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 1)))) }
node bad: T<1> = T<1>(x: 1);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(&error, GraphcalError::EvalError { message, .. }
            if message.contains("out of bounds for Fin(1)")),
        "got: {error:?}"
    );
}

#[test]
fn symbolic_static_fin_key_obligation_rejects_fin_zero() {
    let source = r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 0)))) }
node bad: T<0> = T<0>(x: 0);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(&error, GraphcalError::EvalError { message, .. }
            if message.contains("finite index size must be greater than zero")
                || message.contains("Fin(0)")),
        "got: {error:?}"
    );
}

#[test]
fn symbolic_fin_constant_index_is_checked_after_substitution() {
    let source = r"
type T<N: Nat> {
    T(x: Int(min: to_int((for i: Fin(N) { i })[1])))
}
node bad: T<1> = T<1>(x: 1);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(&error, GraphcalError::EvalError { message, .. }
            if message.contains("index 1 out of bounds for Fin(1)")),
        "got: {error:?}"
    );
}

#[test]
fn negative_constant_index_is_rejected_before_symbolic_fin_deferral() {
    let source = r"
type T<N: Nat> {
    T(x: Int(min: to_int((for i: Fin(N) { i })[-1])))
}
node value: T<2> = T<2>(x: 1);
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(&error, GraphcalError::EvalError { message, .. }
            if message.contains("negative value: -1")),
        "got: {error:?}"
    );
}

// -----------------------------------------------------------------------
// Generic-argument domain constraints are rejected in every type definition
// -----------------------------------------------------------------------

#[test]
fn generic_argument_constraint_in_type_default_is_rejected() {
    let source = r"
type Wrapper<T: Type> { Wrapper(value: T) }
type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::GenericTypeArgDomainConstraint { .. }
    ));
}

#[test]
fn generic_argument_constraint_in_inline_dag_type_is_rejected() {
    let source = r"
dag nested {
    type Wrapper<T: Type> { Wrapper(value: T) }
    type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
    node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
}
";
    let error = check(source).unwrap_err();
    assert!(
        matches!(error, GraphcalError::GenericTypeArgDomainConstraint { .. }),
        "got: {error:?}"
    );
}

#[test]
fn ordinary_field_constraint_remains_legal() {
    let source = r"
type Wrapper<T: Type> { Wrapper(value: T) }
type Good { Good(value: Length(min: 0.0 m)) }
node good: Good = Good(value: 1.0 m);
";
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
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::parse("doubled").unwrap()],
        DeclaredType::Quantity(length)
    );
}

#[test]
fn inline_dag_call_can_project_defaulted_or_bound_param() {
    let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}

node default_factor: Dimensionless = @config().factor;
node bound_factor: Dimensionless = @config(factor: 3.0).factor;
";
    let types = check(source).unwrap();
    assert_eq!(
        types[&ScopedName::parse("default_factor").unwrap()],
        DeclaredType::Quantity(Dimension::dimensionless())
    );
    assert_eq!(
        types[&ScopedName::parse("bound_factor").unwrap()],
        DeclaredType::Quantity(Dimension::dimensionless())
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
        matches!(&err, GraphcalError::MissingDagBindings { missing, .. } if missing == &vec!["factor".to_string()]),
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
        matches!(err, GraphcalError::DagArgTypeMismatch { .. }),
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
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::parse("distances").unwrap()],
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Quantity(length)),
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
    import test.{ index Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region.A: 1.0 m, Region.B: 3.0 m };
node out: Length = @doubler(v: @dist).result[Region.A];
";
    let types = check(source).unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::parse("out").unwrap()],
        DeclaredType::Quantity(length)
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
fn exact_exponent_beyond_dimension_model_uses_d010() {
    for source in [
        "node x: Dimensionless = (2.0 m) ^ 4294967296;",
        "node x: Dimensionless = (2.0 m) ^ -4294967296;",
    ] {
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionOverflow { .. }),
            "expected DimensionOverflow, got: {err:?}"
        );
    }
}

#[test]
fn out_of_range_float_exponent_still_uses_float_syntax_diagnostic() {
    let source = "node x: Dimensionless = (2.0 m) ^ 4294967296.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::FloatPowerExponent {
                replacement: None,
                ..
            }
        ),
        "expected FloatPowerExponent without an unusable fix, got: {err:?}"
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

// --- Plot encoding and plot-family property validation ---

#[test]
fn check_infers_every_plot_encoding_channel() {
    for channel in [
        "x", "y", "color", "size", "shape", "opacity", "detail", "text", "tooltip",
    ] {
        let source = format!("plot p = {{ mark: line, encode: {{ {channel}: true + 1.0 }} }};");
        assert!(
            matches!(check(&source), Err(GraphcalError::DimensionMismatch { .. })),
            "encoding channel `{channel}` escaped inference"
        );
    }
}

#[test]
fn check_rejects_non_plottable_encoding_leaves() {
    for (source, expected) in [
        (
            "type Pair { Pair(x: Dimensionless) }\n\
             node pair: Pair = Pair(x: 1.0);\n\
             plot p = { mark: point, encode: { x: @pair } };",
            "Pair",
        ),
        (
            "node value: Complex<Dimensionless> = complex(1.0, 2.0);\n\
             plot p = { mark: point, encode: { x: @value } };",
            "Complex<Dimensionless>",
        ),
    ] {
        assert!(
            matches!(
                check(source),
                Err(GraphcalError::PlotEncodingTypeMismatch {
                    channel: crate::syntax::ast::EncodingChannel::X,
                    ref found,
                    ..
                }) if found == expected
            ),
            "non-plottable leaf `{expected}` was accepted"
        );
    }
}

#[test]
fn check_rejects_incompatible_plot_channel_axes() {
    let source = r"
index Step = { A, B };
index Pair = { Left, Right };
plot p = {
    mark: point,
    encode: {
        x: for step: Step { step },
        y: for pair: Pair { pair },
    },
};
";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        GraphcalError::PlotEncodingAxisMismatch { ref channels, .. }
            if channels.contains("test.Step") && channels.contains("test.Pair")
    ));
}

#[test]
fn check_accepts_plot_axis_subsets_and_multiaxis_broadcasting() {
    let source = r"
index Phase = { A, B };
index Step = { Start, End };
plot p = {
    mark: rect,
    encode: {
        x: for phase: Phase { phase },
        y: 1.0,
        color: for phase: Phase, step: Step { 1 },
    },
};
";
    check(source).unwrap();
}

#[test]
fn check_accepts_every_plottable_leaf_kind() {
    let source = r#"
index Phase = { A, B };
node instant: Datetime = datetime("2026-01-01T00:00:00Z");
plot p = {
    mark: point,
    encode: {
        x: "label",
        y: 1.0 m,
        color: true,
        size: 1,
        detail: @instant,
        text: Phase.A,
    },
};
"#;
    check(source).unwrap();
}

#[test]
fn check_rejects_ineffective_conversion_inside_plot_encoding() {
    let source = "plot p = { mark: point, encode: { x: (1.0 m -> cm) + 1.0 m } };";
    assert!(matches!(
        check(source),
        Err(GraphcalError::IneffectiveConversion { .. })
    ));
}

#[test]
fn check_unknown_plot_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, caption: \"typo\" };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InvalidPlotProperty { ref property, .. } if property == "caption"),
        "got: {err:?}"
    );
}

#[test]
fn check_unknown_mark_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line { strokewidth: 3.0 }, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InvalidPlotProperty { ref property, .. } if property == "strokewidth"),
        "got: {err:?}"
    );
}

#[test]
fn check_string_property_with_number_value_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, title: 42.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::PlotPropertyTypeMismatch {
                property: "title",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_numeric_property_with_string_value_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, width: \"wide\" };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::PlotPropertyTypeMismatch {
                property: "width",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_dimensioned_mark_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line { stroke_width: 2.0 m }, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::PlotPropertyDimensioned {
                property: "stroke_width",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_figure_width_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [p], width: 300.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InvalidPlotProperty { ref property, context: "a figure declaration", .. } if property == "width"),
        "got: {err:?}"
    );
}

#[test]
fn check_layer_width_is_accepted() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
layer l = { plots: [p], width: 300.0, title: \"ok\" };";
    check(source).unwrap();
}

#[test]
fn check_valid_plot_properties_pass() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = {
    mark: line { stroke_width: 2.0, opacity: 0.5, color: \"steelblue\", filled: true },
    encode: { x: for s: Step { @vals[s] }, y: for s: Step { @vals[s] } },
    title: \"ok\",
    width: 400.0,
    x_label: \"X\",
};";
    check(source).unwrap();
}

// --- Figure/layer plot references are validated at resolution time (#843) ---

#[test]
fn check_unknown_plot_reference_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot real_plot = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [real_plot, my_polt] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownPlotReference { owner_kind: "figure", ref name, .. } if name.to_string() == "my_polt"),
        "got: {err:?}"
    );
}

#[test]
fn check_figure_referencing_figure_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure inner = { plots: [p] };
figure outer = { plots: [inner] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            GraphcalError::CompositionReferencesNonPlot {
                actual_kind: "figure",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_duplicate_plot_reference_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [p, p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DuplicatePlotReference { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_valid_plot_references_pass() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
plot q = { mark: point, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [p, q] };
layer l = { plots: [p, q] };";
    check(source).unwrap();
}

// --- #[hidden] attribute (#847) ---

#[test]
fn check_hidden_on_plot_is_accepted() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
#[hidden]
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };";
    check(source).unwrap();
}

#[test]
fn check_hidden_on_node_is_rejected() {
    let source = "\
#[hidden]
node x: Dimensionless = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InvalidHiddenTarget { ref kind, .. }
        if kind == &crate::ir::resolve::AttributeTarget::declaration(
            crate::ir::resolve::DeclarationKind::Node,
        )),
        "got: {err:?}"
    );
}

#[test]
fn check_hidden_on_figure_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
#[hidden]
figure f = { plots: [p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::InvalidHiddenTarget { ref kind, .. }
        if kind == &crate::ir::resolve::AttributeTarget::declaration(
            crate::ir::resolve::DeclarationKind::Figure,
        )),
        "got: {err:?}"
    );
}

#[test]
fn check_hidden_with_args_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };
#[hidden(now)]
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { ref message, .. } if message.contains("no arguments")),
        "got: {err:?}"
    );
}
