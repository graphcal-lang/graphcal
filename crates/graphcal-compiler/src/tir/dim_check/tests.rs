use super::*;
use crate::dimension::{BaseDimId, Dimension};
use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedStructTypeName};
use crate::semantic::checked_type::{CheckedGenericArg, IndexTypeRef, StructTypeRef};
use crate::semantic_error::SemanticErrorKind;
use crate::semantic_error::attribute::AttributeError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::domain::DomainError;
use crate::semantic_error::graph::GraphError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::module::ModuleError;
use crate::semantic_error::name::NameError;
use crate::semantic_error::structure::StructError;
use crate::semantic_error::visibility::VisibilityError;
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::parser::Parser;
use crate::syntax::span::Span;

fn make_src(source: &str) -> crate::source_id::SourceId {
    crate::source_registry::SourceRegistry::new()
        .register("test.gcl", std::sync::Arc::new(source.to_string()))
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

fn check(source: &str) -> Result<HashMap<ScopedName, CheckedType>, SemanticError> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let src = make_src(source);
    let lowered = crate::ir::lower::lower_file_with_inline_dags_for_test(&file, "test.gcl", src)?;
    let resolver = lowered.resolver;
    let mut project_types = crate::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|err| {
        SemanticError::internal_error(
            format!("test module type prelude failed: {err}"),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::Source(Span::new(0, 0)),
        )
    })?;
    for dag in std::iter::once(&lowered.root).chain(&lowered.inline_dags) {
        project_types
            .insert_module(dag.definitions())
            .map_err(|error| {
                SemanticError::internal_error(
                    format!("test HIR type store failed: {error}"),
                    src,
                    crate::diagnostic_anchor::DiagnosticAnchor::Source(Span::new(0, 0)),
                )
            })?;
    }
    let mut builder = crate::tir::typed::type_resolve_draft(
        lowered.root,
        src,
        &resolver,
        std::sync::Arc::new(project_types.clone()),
    )?;
    for dag_body_ir in lowered.inline_dags {
        let compiled_dag = crate::tir::typed::type_resolve_single_with_modules(
            dag_body_ir,
            src,
            &resolver,
            &project_types,
        )?;
        builder.insert_dag(compiled_dag).map_err(|error| {
            SemanticError::internal_error(
                error.to_string(),
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(Span::new(0, 0)),
            )
        })?;
    }
    let tir = check_draft(builder, src)?;
    Ok(root_declared_types(tir.root().body()))
}

/// Instantiate and check a draft without external override summaries.
fn check_draft(
    draft: crate::tir::typed::TirDraft,
    src: crate::source_id::SourceId,
) -> Result<crate::tir::typed::CheckedTir, SemanticError> {
    draft
        .instantiate(
            &crate::tir::typed::CheckedOverrideDependencies::default(),
            src,
        )
        .and_then(|instantiated| {
            crate::outcome::without_cancellation(|cancellation| {
                instantiated.check(src, cancellation)
            })
        })
}

/// The checked declared type of every root value declaration and imported
/// value, keyed by its written name.
fn root_declared_types(root: &crate::tir::typed::DagTIR) -> HashMap<ScopedName, CheckedType> {
    root.consts()
        .map(|entry| (entry.name(), &entry.type_ann))
        .chain(root.params().map(|entry| (entry.name(), &entry.type_ann)))
        .chain(root.nodes().map(|entry| (entry.name(), &entry.type_ann)))
        .map(|(name, annotation)| {
            (
                ScopedName::local(name.clone()),
                annotation.checked().declared().clone(),
            )
        })
        .chain(
            root.imported_bindings()
                .iter()
                .map(|(name, binding)| (name.clone(), binding.declared_type().clone())),
        )
        .collect()
}

/// Edit the root DAG's declaration records in place.
fn edit_root_decls(
    tir: &mut crate::tir::typed::TirDraft,
    edit: impl FnMut(&mut crate::tir::typed::TypedDecl),
) {
    tir.root_mut().decls.update(edit);
}

fn module_aware_tir(source: &str) -> (crate::tir::typed::TirDraft, crate::source_id::SourceId) {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let src = make_src(source);
    let ir = crate::ir::lower::lower(&file, "test.gcl", src).unwrap();
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(ir.dag_id().clone(), &file.declarations);
    let resolver = modules.build().unwrap();
    let mut project_types = crate::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_module(ir.definitions()).unwrap();
    let tir = crate::tir::typed::type_resolve_draft(
        ir,
        src,
        &resolver,
        std::sync::Arc::new(project_types),
    )
    .unwrap();
    (tir, src)
}

/// Count the nodes of every checked tree of `bodies` that `select` accepts.
fn count_nodes(bodies: &crate::tir::texpr::CheckedBodies, select: fn(bool) -> bool) -> usize {
    use crate::tir::texpr::{CheckedBody, TNodeRef, visit_tnodes};
    let mut count = 0_usize;
    for (_, body) in bodies.roots() {
        let mut visit = |contextual: bool| {
            if select(contextual) {
                count = count.saturating_add(1);
            }
        };
        match body {
            CheckedBody::Executable(body) => visit_tnodes(body.as_node(), &mut |node| {
                visit(matches!(node, TNodeRef::Contextual(_)));
            }),
            CheckedBody::Deferred(body) => visit_tnodes(body.as_node(), &mut |node| {
                visit(matches!(node, TNodeRef::Contextual(_)));
            }),
        }
    }
    count
}

fn count_contextual(bodies: &crate::tir::texpr::CheckedBodies) -> usize {
    count_nodes(bodies, |contextual| contextual)
}

fn count_contextual_nodes(tree: &crate::tir::texpr::TExpr) -> usize {
    let mut count = 0_usize;
    crate::tir::texpr::visit_tnodes(crate::tir::texpr::TNodeRef::Value(tree), &mut |node| {
        if matches!(node, crate::tir::texpr::TNodeRef::Contextual(_)) {
            count = count.saturating_add(1);
        }
    });
    count
}

#[test]
fn one_checking_pass_records_every_expression_once() {
    for depth in [0, 8, 16, 32] {
        let source = format!(
            "node value: Dimensionless = {}1.0{};",
            "-(".repeat(depth),
            ")".repeat(depth)
        );
        let (tir, src) = module_aware_tir(&source);
        let tir = check_draft(tir, src).unwrap();
        assert_eq!(count_nodes(tir.root().bodies(), |_| true), depth + 1);
    }
}

#[test]
fn consuming_rules_record_contextual_literals() {
    let (tir, src) =
        module_aware_tir("node value: Datetime<UTC> = datetime(\"2026-01-01T00:00:00Z\");");
    let tir = check_draft(tir, src).unwrap();
    // The literal argument is parsed into the typed datetime it builds.
    assert_eq!(count_contextual(tir.root().bodies()), 0);
    let node = tir.root().body().nodes().next().unwrap();
    let independent = check_callless_value_expr_type(
        &tir,
        node.definition.formula().unwrap(),
        node.type_ann.checked().declared(),
        src,
    )
    .unwrap();
    assert!(matches!(
        independent.tree().kind(),
        crate::tir::texpr::TExprKind::DatetimeLiteral(crate::tir::texpr::DatetimeLiteral::Offset(
            _
        ))
    ));
    assert_eq!(count_contextual_nodes(independent.tree()), 0);

    let (tir, src) = module_aware_tir(
        "node zoned: Datetime<UTC> = datetime(\"2026-01-01T09:00:00\", \"Asia/Tokyo\");\n\
         node civil: Datetime<TAI> = epoch<TAI>(\"2026-01-01T00:00:00\");\n\
         index S = { A, B };\n\
         node xs: Dimensionless[S] = { S#A: 1.0, S#B: 2.0 };\n\
         plot p = {\n\
             mark: point,\n\
             encode: { x: @xs, y: @xs, color: \"red\" },\n\
             title: \"Values\",\n\
         };",
    );
    let tir = check_draft(tir, src).unwrap();
    // The string encoding and the string property; datetime constructor
    // arguments are parsed into the typed datetimes they build.
    assert_eq!(count_contextual(tir.root().bodies()), 2);
}

#[test]
fn body_observations_reject_a_second_record_of_one_expression() {
    let source = "node value: Dimensionless = 1.0;";
    let (tir, src) = module_aware_tir(source);
    let observations = infer::hir::BodyObservations::default();
    let expr = tir
        .root()
        .nodes()
        .next()
        .unwrap()
        .definition
        .formula()
        .unwrap();
    let ty = CheckedType::Quantity(Dimension::dimensionless());
    let unchecked = tir.clone().finish();
    observations
        .record(expr, &ty, tir.root(), &unchecked, src)
        .unwrap();
    assert!(matches!(
        observations.record(expr, &ty, tir.root(), &unchecked, src),
        Err(SemanticError::Internal(_))
    ));
}

#[test]
fn string_plot_channel_does_not_depend_on_a_rigid_dimension_port() {
    let source = r#"
dag lib {
    pub(bind) dim Q = Length;
    param q: Q = 1.0 m;
    index S = { A, B };
    node xs: Q[S] = { S#A: @q, S#B: @q };
    plot p = {
        mark: point,
        encode: { x: @xs, y: @xs, color: "red" },
    };
}
"#;
    check(source).unwrap();
}

#[test]
fn template_closure_reports_the_first_observed_type_definition_use() {
    let source = "pub(bind) type Record { Record(x: Dimensionless) }\n\
                  param r: Record;\n\
                  pub node plain: Dimensionless = 1.0;\n\
                  pub node read: Dimensionless = @r.x + @r.x;";
    let (tir, src) = module_aware_tir(source);
    let error = check_draft(tir, src).unwrap_err();
    let SemanticError::Located(crate::diagnostic::Diagnostic {
        kind:
            SemanticErrorKind::Visibility(VisibilityError::TemplateBodyDependsOnStaticDefault {
                body_name,
                ..
            }),
        primary: span,
        ..
    }) = error
    else {
        panic!("expected V007, got {error:?}");
    };
    assert_eq!(body_name.as_str(), "read");
    assert_eq!(span.offset(), source.find("@r.x").unwrap() + "@r.".len());
}

fn model_port_application(
    source: &str,
) -> (
    crate::tir::typed::TirDraft,
    crate::source_id::SourceId,
    StructTypeRef,
    Vec<CheckedGenericArg>,
) {
    let (tir, src) = module_aware_tir(source);
    let declared_types = root_declared_types(tir.root());
    let CheckedType::Struct(identity, generic_args) = &declared_types
        [&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("port"))]
    else {
        panic!("expected `port` to be a concrete model struct");
    };
    (tir, src, identity.clone(), generic_args.clone())
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
param values: Dimensionless[Axis] = { Axis#A: 3.0, Axis#B: 4.0 };
param fixed_values: Dimensionless[FixedAxis] = {
    FixedAxis#A: 5.0,
    FixedAxis#B: 6.0,
};
param dependent: Dimensionless = @record.x + @fixed.x + match Axis#A {
    Axis#A => 1.0,
    Axis#B => 2.0,
};
param wrapped: Wrapper<Record, Axis> =
    Wrapper<Record, Axis>(value: @record, values: @values);
param fixed_wrapped: Wrapper<Fixed, FixedAxis> =
    Wrapper<Fixed, FixedAxis>(value: @fixed, values: @fixed_values);
";
    let (tir, src) = module_aware_tir(source);
    let tir = check_draft(tir, src).unwrap();

    let summary = collect_override_dependency_summary(
        &tir,
        &crate::cancellation::CancellationToken::unbounded(),
    )
    .unwrap();
    let owner = test_dag_id();
    let record = NominalOverrideIdentity::Type(ResolvedStructTypeName::for_test(
        owner.clone(),
        crate::syntax::type_name::StructTypeName::expect_valid("Record"),
    ));
    let axis = NominalOverrideIdentity::Index(ResolvedIndexName::for_test(
        owner.clone(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    ));
    let dependencies = |param: &str| {
        summary.get(&ResolvedDeclName::for_test(
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
    let tir = check_draft(tir, src).unwrap();
    let cancellation = crate::cancellation::CancellationSource::new();
    cancellation.cancel();

    assert!(matches!(
        collect_override_dependency_summary(&tir, &cancellation.token()),
        Err(crate::cancellation::Cancelled)
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

    let a = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("a"));
    let b = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("b"));
    let x = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("x"));
    let y = ResolvedDeclName::for_test(dag_id, DeclName::expect_valid("y"));

    let mut resolved = crate::tir::typed::ResolvedDagDependencies::default();
    resolved.const_deps.insert(a.clone(), BTreeSet::new());
    resolved.const_deps.insert(b, BTreeSet::from([a]));
    resolved.runtime_deps.insert(x.clone(), BTreeSet::new());
    resolved.runtime_deps.insert(y, BTreeSet::from([x]));

    tir.root_mut().semantic.dependencies = resolved;

    check_draft(tir, src).unwrap();
}

#[test]
fn materialized_shape_identity_survives_equal_and_shifted_source_coordinates() {
    let source =
        "node result: Dimensionless = sum(for p: Fin(2) { 1.0 }) + sum(for q: Fin(3) { 1.0 });";
    let (mut draft, src) = module_aware_tir(source);
    let mut ids = Vec::new();
    crate::hir::expr::visit_expr(
        draft
            .root()
            .nodes()
            .next()
            .unwrap()
            .definition
            .formula()
            .unwrap(),
        &mut |expr| {
            if matches!(expr.kind(), crate::hir::expr::ExprKind::ForComp { .. }) {
                ids.push(expr.id().clone());
            }
        },
    );
    assert_eq!(ids.len(), 2);
    edit_root_decls(&mut draft, |decl| {
        if let crate::ir::entry::Decl::Node(entry) = decl {
            entry
                .definition
                .formula_mut()
                .unwrap()
                .map_spans_for_test(|_| Span::new(0, 1));
        }
    });
    let tir = check_draft(draft.clone(), src).unwrap();
    // The executable tree nodes checked for `ids`, in pre-order.
    let nodes = |tir: &crate::tir::typed::CheckedTir| {
        let formula = tir
            .root()
            .body()
            .nodes()
            .next()
            .unwrap()
            .definition
            .formula()
            .unwrap();
        let tree = tir.root().bodies().executable_value(formula.id()).unwrap();
        let mut types = Vec::new();
        crate::tir::texpr::visit_tnodes(crate::tir::texpr::TNodeRef::Value(tree), &mut |node| {
            if let crate::tir::texpr::TNodeRef::Value(expr) = node
                && ids.contains(expr.id())
            {
                types.push(expr.ty().clone());
            }
        });
        types
    };
    let totals = |tir: &crate::tir::typed::CheckedTir| {
        nodes(tir)
            .iter()
            .map(|ty| {
                ty.materialized_shape(|axis| {
                    Ok::<_, crate::tir::materialized_shape::MaterializedShapeError>(
                        tir.index_def(axis)
                            .and_then(|index| index.concrete_cardinality()),
                    )
                })
                .unwrap()
                .expect("an indexed value has a materialized shape")
                .total()
                .get()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(totals(&tir), vec![2, 3]);
    edit_root_decls(&mut draft, |decl| {
        if let crate::ir::entry::Decl::Node(entry) = decl {
            entry
                .definition
                .formula_mut()
                .unwrap()
                .map_spans_for_test(|_| Span::new(2, 3));
        }
    });
    let tir = check_draft(draft, src).unwrap();
    assert_eq!(totals(&tir), vec![2, 3]);
    let (rebuilt, rebuilt_src) = module_aware_tir(source);
    let rebuilt = check_draft(rebuilt, rebuilt_src).unwrap();
    assert!(nodes(&rebuilt).is_empty());
}

#[test]
fn node_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir("node y: Dimensionless = sqrt(4.0);");
    edit_root_decls(&mut tir, |decl| {
        if let crate::ir::entry::Decl::Node(entry) = decl {
            entry
                .definition
                .formula_mut()
                .unwrap()
                .replace_kind_for_test(crate::hir::expr::ExprKind::StringLiteral(
                    "not dimensionless".to_string(),
                ));
        }
    });

    assert!(check_draft(tir, src).is_err());
}

#[test]
fn indexed_node_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir(
        "index Phase = { Burn };\n\
         node y: Dimensionless[Phase] = for p: Phase { match p { Phase#Burn => 1.0 } };",
    );
    edit_root_decls(&mut tir, |decl| {
        if let crate::ir::entry::Decl::Node(entry) = decl {
            entry
                .definition
                .formula_mut()
                .unwrap()
                .replace_kind_for_test(crate::hir::expr::ExprKind::StringLiteral(
                    "not indexed".to_string(),
                ));
        }
    });

    assert!(check_draft(tir, src).is_err());
}

#[test]
fn assert_entry_body_is_authoritative_for_hir_dimension_check() {
    let (mut tir, src) = module_aware_tir("assert ok = sqrt(4.0) == 2.0;");
    edit_root_decls(&mut tir, |decl| {
        if let crate::ir::entry::Decl::Assert(entry) = decl {
            entry.body = crate::hir::expr::CheckedAssertBody::from_assert_body_for_test(
                crate::hir::expr::AssertBody::Expr(Box::new(crate::hir::expr::Expr::new(
                    crate::hir::expr::ExprKind::StringLiteral("not bool".to_string()),
                    entry.span,
                ))),
            );
        }
    });

    assert!(check_draft(tir, src).is_err());
}

#[test]
fn check_dimensionless_const() {
    let types = check("const node g0: Dimensionless = 9.80665;").unwrap();
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("g0"))],
        CheckedType::Quantity(Dimension::dimensionless())
    );
}

#[test]
fn check_dimensionless_arithmetic() {
    let types = check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("y"))],
        CheckedType::Quantity(Dimension::dimensionless())
    );
}

#[test]
fn check_length_quantity_literal() {
    let types = check("param alt: Length = 400.0 km;").unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("alt"))],
        CheckedType::Quantity(length)
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
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("speed"))],
        CheckedType::Quantity(velocity)
    );
}

#[test]
fn check_add_dimension_mismatch() {
    let source = "param x: Length = 1.0 m;\nparam y: Time = 1.0 s;\nnode z: Length = @x + @y;";
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
            ..
        })
    ));
}

#[test]
fn check_annotation_mismatch() {
    let source = "param x: Length = 1.0 m;\nnode y: Time = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::DimensionMismatchInAnnotation { .. }
                ),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_duplicate_key() {
    let source = "\
pub index Mode = { A, B };
param lhs: Dimensionless[Mode] = { Mode#A: 1.0, Mode#B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode#A: 2.0, Mode#B: 0.0 };
#[expected_fail(Mode#A, Mode#A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::ExpectedFailDuplicateKey),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_foreign_index_key() {
    let source = "\
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode#A: 1.0, Mode#B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode#A: 2.0, Mode#B: 0.0 };
#[expected_fail(Other#A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(
                    AttributeError::ExpectedFailKeyIndexMismatch { .. }
                ),
                ..
            })
        ),
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
#[expected_fail(Mode#A)]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(
                    AttributeError::ExpectedFailKeyShapeMismatch { .. }
                ),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_expected_fail_rejects_variant_key_on_unindexed_assertion() {
    let source = "\
pub index Mode = { A, B };
param lhs: Dimensionless = 1.0;
param rhs: Dimensionless = 2.0;
#[expected_fail(Mode#A)]
assert order = @lhs > @rhs;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::ExpectedFailNotIndexed),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::ExpectedFailAllOnIndexed),
                ..
            })
        ),
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
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(
            "speed_kmh"
        ))],
        CheckedType::Quantity(velocity)
    );
}

#[test]
fn check_conversion_wrong_dimension() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = @x -> s;";
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::ConversionDimensionMismatch { .. }),
            ..
        })
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(
                DimensionError::DimensionMismatchInAnnotation { .. }
            ),
            ..
        })
    ));
}

#[test]
fn check_builtin_sin_requires_angle() {
    let source = "param x: Length = 1.0 m;\nnode y: Dimensionless = sin(@x);";
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
            ..
        })
    ));
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
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
            ..
        })
    ));
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
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Name(NameError::UnknownFunction { .. }),
            ..
        })
    ));
}

// --- Indexed type tests ---

#[test]
fn check_indexed_param_map_literal() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};";
    let types = check(source).unwrap();
    let velocity = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    )))
    .unwrap();
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("dv"))],
        CheckedType::Indexed {
            element: Box::new(CheckedType::Quantity(velocity)),
            index: test_index_ref("Maneuver"),
        }
    );
}

#[test]
fn check_for_comprehension() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node doubled: Velocity[Maneuver] = for m: Maneuver { @dv[m] + @dv[m] };";
    check(source).unwrap();
}

#[test]
fn check_for_comprehension_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node bad: Length[Maneuver] = for m: Maneuver { @dv[m] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::DimensionMismatchInAnnotation { .. }
                ),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_index_access_with_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
param first: Velocity = @dv[Maneuver#Departure];";
    check(source).unwrap();
}

#[test]
fn check_map_literal_missing_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::MissingVariants { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_map_literal_extra_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::ExtraVariants { .. }),
                ..
            })
        ),
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
        .map(|axis| format!("A{axis}#X"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "param values: Dimensionless[{axes}] = {{ ({tuple}): 1.0 }};"
    )
    .unwrap();

    let error = check(&source).unwrap_err();
    assert!(
        matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::NonExhaustiveMapLiteral { .. }), .. })
            if kind.to_string().contains("missing 524287 entries")
                && kind.to_string().contains("first missing entry")
                && kind.to_string().contains("A18#Y")),
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
        .map(|axis| format!("A{axis}#X"))
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
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::MaterializedShapeTooLarge {
                    maximum: 1_000_000,
                    ..
                }),
                ..
            })
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
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node bad: Velocity[Phase] = for p: Phase { @dv[p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_sum_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node total_dv: Velocity = sum(@dv);";
    check(source).unwrap();
}

#[test]
fn check_product_and_rss_aggregation_dimensions() {
    let source = "\
pub index Factor = { A, B, C };
param lengths: Length[Factor] = {
    Factor#A: 2.0 m,
    Factor#B: 3.0 m,
    Factor#C: 4.0 m,
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(
                DimensionError::DimensionMismatchInAnnotation { .. }
            ),
            ..
        })
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::MultiAxisAggregation {
                rank: 2,
                ..
            }),
            ..
        })
    ));
}

#[test]
fn check_mean_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node avg_dv: Velocity = mean(@dv);";
    check(source).unwrap();
}

#[test]
fn check_minimum_maximum_aggregations() {
    let source = "\
pub index Case = { Low, High };
param values: Length[Case] = {
Case#Low: 1.0 m,
Case#High: 2.0 m,
};
node low: Length = minimum(@values);
node high: Length = maximum(@values);";
    check(source).unwrap();
}

#[test]
fn reduction_functions_have_fixed_arity() {
    let err = check("node x: Dimensionless = minimum(1.0, 2.0);").unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Name(NameError::WrongArity { .. }),
            ..
        })
    ));
}

#[test]
fn zero_argument_arity_error_points_at_the_call() {
    let source = "node x: Dimensionless = sin();";
    let error = check(source).unwrap_err();
    let SemanticError::Located(crate::diagnostic::Diagnostic {
        kind: SemanticErrorKind::Name(NameError::WrongArity { .. }),
        primary: span,
        ..
    }) = error
    else {
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
node matrix_product: Length * Time[Fin(2), Fin(4)] = matmul(@a, @b);
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::LinearAlgebraShapeMismatch { .. }
                ),
                ..
            })
        ),
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::LinearAlgebraShapeMismatch { .. }),
            ..
        })
    ));
}

#[test]
fn check_linear_algebra_rejects_non_quantity_elements() {
    let source = "\
param flags: Bool[Fin(2)] = for i: Fin(2) { true };
node result: Dimensionless = norm(@flags);";
    let error = check(source).unwrap_err();
    assert!(matches!(
        error,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
            ..
        })
    ));
}

#[test]
fn linear_algebra_functions_have_fixed_arity() {
    let error = check("node x: Dimensionless = dot(1.0);").unwrap_err();
    assert!(matches!(
        error,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Name(NameError::WrongArity { .. }),
            ..
        })
    ));
}

/// An argument that fails inference on its own. A wrong-arity call that
/// contains it must still report the arity, proving the entry-driven arity
/// check runs before any argument is inferred.
const ILL_TYPED_ARG: &str = "(1.0 m + 1.0 s)";

#[test]
fn builtin_arity_is_checked_before_arguments_are_inferred() {
    let alone = check(&format!("node x: Dimensionless = {ILL_TYPED_ARG};")).unwrap_err();
    assert!(
        matches!(
            alone,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
        "got: {alone:?}"
    );
    // (callee, arguments with `{bad}` placeholders, expected arity), one row
    // per family and per arity within a family.
    let cases = [
        ("sin", "{bad}, 1.0", 1),
        ("re", "{bad}, 1.0", 1),
        ("complex", "{bad}", 2),
        ("sum", "{bad}, 1.0", 1),
        ("norm", "{bad}, 1.0", 1),
        ("dot", "{bad}", 2),
        ("year", "{bad}, 1.0", 1),
        ("from_jd", "{bad}, 1.0", 1),
        ("to_jd", "{bad}, 1.0", 1),
        ("to_tai", "{bad}, 1.0", 1),
        ("epoch<TT>", "{bad}, 1.0", 1),
        ("to_float", "{bad}, 1.0", 1),
    ];
    for (function, arguments, expected) in cases {
        let arguments = arguments.replace("{bad}", ILL_TYPED_ARG);
        let got = arguments.split(", ").count();
        let source = format!("node x: Dimensionless = {function}({arguments});");
        let error = check(&source).unwrap_err();
        let SemanticError::Located(crate::diagnostic::Diagnostic {
            kind:
                SemanticErrorKind::Name(NameError::WrongArity {
                    name,
                    expected: found_expected,
                    got: found_got,
                    ..
                }),
            primary: span,
            ..
        }) = error
        else {
            panic!("expected wrong-arity diagnostic for `{source}`, got: {error:?}");
        };
        let spelling = function.split('<').next().unwrap();
        assert_eq!(name.to_string(), spelling, "`{source}`");
        assert_eq!((found_expected, found_got), (expected, got), "`{source}`");
        assert_eq!(span.offset(), source.find(function).unwrap(), "`{source}`");
    }
}

#[test]
fn optional_trailing_arity_is_checked_before_arguments_are_inferred() {
    let source = format!("node x: Datetime<UTC> = datetime({ILL_TYPED_ARG}, 1.0, 2.0);");
    let error = check(&source).unwrap_err();
    let SemanticError::Located(crate::diagnostic::Diagnostic {
        kind: SemanticErrorKind::Name(kind @ NameError::WrongOptionalArity { .. }),
        ..
    }) = &error
    else {
        panic!("expected optional-trailing arity diagnostic, got: {error:?}");
    };
    assert_eq!(
        kind.to_string(),
        "datetime() expects 1 or 2 arguments, got 3"
    );
}

#[test]
fn check_scan() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node cum_dv: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc + val);";
    check(source).unwrap();
}

#[test]
fn check_scan_supports_heterogeneous_accumulator() {
    let source = "\
pub index Flag = { A, B };
param flags: Bool[Flag] = {
Flag#A: true,
Flag#B: false,
};
node count_true: Int[Flag] = scan(
    @flags,
    0,
    |count_acc, flag| if flag { count_acc + 1 } else { count_acc }
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::MultiAxisScanSource {
                    rank: 2,
                    ..
                }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_scan_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 m, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_unknown_index_in_type_annotation() {
    let source = "param x: Velocity[NoSuchIndex] = 1.0 m / s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::UnknownIndex { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_for_with_sum() {
    // sum over a for comprehension
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
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
Case#A: 1.0 m,
Case#B: 2.0 m,
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
                    SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::IndexedComparisonOperand { found, .. }), .. })
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
node lhs: Length[Case] = { Case#A: 1.0 m, Case#B: 2.0 m };
node rhs: Length[Case] = { Case#A: 1.0 m, Case#B: 2.5 m };
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::RuntimeExponentForDimensionedBase
                ),
                ..
            })
        ),
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
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::FloatPowerExponent { replacement: Some(ref replacement), .. }), .. }) if replacement == "(1/4)"
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
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::FloatPowerExponent { replacement: Some(ref replacement), .. }), .. }) if replacement == "2"
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn hir_normalizes_omitted_dimension_and_unit_powers() {
    let (tir, _) = module_aware_tir("param distance: Length = 1.0 m;");
    let param = tir.root().params().next().unwrap();
    let crate::hir::types::DeclType::Value(crate::hir::types::ValueType {
        kind: crate::hir::types::ValueTypeKind::DimExpr(dimension),
        ..
    }) = &param.type_ann.decl_type
    else {
        panic!("expected dimension expression");
    };
    assert_eq!(
        dimension.terms[0].term.power,
        crate::dimension::Rational::ONE
    );

    let expression = param.default.as_ref().unwrap();
    let crate::hir::expr::ExprKind::QuantityLiteral { unit, .. } = expression.kind() else {
        panic!("expected quantity literal");
    };
    assert_eq!(unit.terms[0].power, crate::dimension::Rational::ONE);
}

#[test]
fn hir_preserves_exact_power_metadata() {
    let (tir, _) = module_aware_tir("param x: Length = 4.0 m;\nnode y: Length^(3/2) = @x ^ (3/2);");
    let expression = &tir
        .root()
        .nodes()
        .next()
        .unwrap()
        .definition
        .formula()
        .unwrap();
    assert!(matches!(
        expression.kind(),
        crate::hir::expr::ExprKind::BinOp {
            op: crate::syntax::ast::BinOp::Pow(
                crate::syntax::ast::PowerExponent::Exact(exponent)
            ),
            ..
        } if *exponent == crate::exact_rational::ExactRational::try_new(3, 2).unwrap()
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionOverflow),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Unknown dimension in type annotation ---

#[test]
fn check_unknown_dimension_in_type() {
    let source = "param x: NoSuchDimension = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::NotAStruct { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_record_field_access_requires_same_named_constructor() {
    let valid = "\
pub type Box { Box(value: Dimensionless) }
param box: Box = Box(value: 1.0);
node value: Dimensionless = @box.value;";
    check(valid).unwrap();

    let non_record = "\
pub type Box { Make(value: Dimensionless) }
param box: Box = Make(value: 1.0);
node value: Dimensionless = @box.value;";
    let err = check(non_record).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::NotAStruct { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::ExtraFields { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::DuplicateConstructionField { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::UnknownField { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::DuplicatePatternBinding { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Block let-binding type annotation mismatch ---

// --- type equality: mismatched kinds ---

#[test]
fn check_types_match_struct_vs_quantity() {
    // Declared as a struct type but expression evaluates to quantity → mismatch
    let source = "\
type Orbit { Orbit(altitude: Length, speed: Velocity) }
param x: Dimensionless = 1.0;
node o: Orbit = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::DimensionMismatchInAnnotation { .. }
                ),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::UnknownIndex { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Scan body type mismatch ---

#[test]
fn check_scan_body_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver#Departure: 2.46 km / s,
Maneuver#Correction: 0.5 km / s,
Maneuver#Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc * val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::ScanSourceNotIndexed),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Map literal dimension inconsistency ---

#[test]
fn check_map_literal_inconsistent_element_dims() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless[Phase] = {
Phase#Coast: 1.0,
Phase#Burn: 2.0 m,
};";
    let err = check(source).unwrap_err();
    // The map entries have different dimensions: first is Dimensionless, second is Length
    assert!(
        matches!(
            err,
            SemanticError::Located(
                crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(
                        DimensionError::DimensionMismatchInAnnotation { .. }
                    ),
                    ..
                } | crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                    ..
                }
            )
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
Phase#Coast: 1.0,
Phase#Burn: 2.0,
};
param bad: Dimensionless = @x[Phase#NoSuch];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::UnknownVariant { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Indexing a non-indexed value ---

#[test]
fn check_index_access_on_quantity() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless = 1.0;
param bad: Dimensionless = @x[Phase#Coast];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::IndexingNonIndexedValue),
                ..
            })
        ),
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
Phase#Coast: 1.0,
Phase#Burn: 2.0,
};
param bad: Dimensionless = @x[Stage#First];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                ..
            })
        ),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::IndexOutOfBounds { .. }), .. }) if kind.to_string().contains("index 5 out of bounds for Fin(3)")),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::NegativeIndex { .. }), .. }) if kind.to_string().contains("index expression evaluated to negative value: -1")),
        "got: {err:?}"
    );
}

#[test]
fn bare_term_does_not_probe_owner_scoped_index_labels() {
    let source = "\
pub index M = { A };
pub index P = { A };
node x: Dimensionless = A;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }) if kind.to_string().contains("unknown Term `A`")),
        "got: {err:?}"
    );
}

#[test]
fn value_position_does_not_probe_static_prelude_dimension() {
    let source = "node x: Dimensionless = Length;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }) if kind.to_string().contains("unknown Term `Length`")),
        "got: {err:?}"
    );
}

#[test]
fn type_position_does_not_probe_term_value() {
    let source = "\
node a: Dimensionless = 1.0;
node b: a = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name.as_bare() == Some(&crate::syntax::names::NameAtom::parse("a").unwrap())),
        "got: {err:?}"
    );
}

#[test]
fn index_label_syntax_is_rejected_as_a_type_without_fallback() {
    let source = "\
pub index M = { A };
param x: M#A = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(kind @ NameError::IndexLabelAsType { .. }), .. })
            if kind.to_string() == "index label `M#A` cannot be used as a type"),
        "got: {err:?}"
    );
}

#[test]
fn type_position_does_not_probe_term_constructor() {
    let source = "\
type Pos { MkPos(v: Dimensionless) }
param p: MkPos = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name.as_bare() == Some(&crate::syntax::names::NameAtom::parse("MkPos").unwrap())),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }) if kind.to_string().contains("`M` is an index, not a type")),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::GraphRefInConstUnit { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn dynamic_unit_scale_requires_scalar_dimensionless_quantity() {
    for factor in [
        "param factor: Length = 2.0 m;",
        "param factor: Bool = true;",
        "param factor: Int = 2;",
        "pub index Case = { A, B };\nparam factor: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.0 };",
    ] {
        let source = format!(
            "base dim Money;\nbase unit USD: Money;\n{factor}\nunit EUR: Money = (@factor) USD;"
        );
        let err = check(&source).unwrap_err();
        assert!(
            matches!(
                err,
                SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(
                        DimensionError::DynamicUnitScaleTypeMismatch { .. }
                    ),
                    ..
                })
            ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonConstUnitInConst { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonConstUnitInConst { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonConstUnitInConst { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonConstUnitInConst { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

// --- Error propagation through map literal entry ---

#[test]
fn check_map_literal_error_in_entry() {
    let source = "\
pub index Phase = { Coast, Burn };
param bad: Dimensionless[Phase] = {
Phase#Coast: 1.0 foobar,
Phase#Burn: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { .. }),
                ..
            })
        ),
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
Phase#Coast: 1.0,
Stage#Second: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                ..
            })
        ),
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
param v: Dimensionless[Phase] = { Phase#A: 1.0 };
node w: Dimensionless[TimeStep] = for t: TimeStep { @v[t] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::IndexMismatch { expected, found, .. }), .. }) if expected.to_string() == "Phase" && found.to_string() == "TimeStep"),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::IndexMismatch { expected, found, .. }), .. }) if expected.to_string() == "LenGrid" && found.to_string() == "TimeGrid"),
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
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation { declared, inferred, .. }), .. }) if declared == "Dimensionless[Element, Step]"
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
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { expected, found, .. }), .. }) if expected == "Dimensionless[Element]"
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
    |prev_value, prev_phase_key, phase_key| prev_value + 1.0
);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::UnfoldRequiresCoordinateIndex { .. }), .. }) if kind.to_string().contains("unfold requires a coordinate index")),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { .. }),
                ..
            })
        ),
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
            matches!(
                err,
                SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { .. }),
                    ..
                })
            ),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { found, .. }), .. }) if found.contains("Fin")),
        "got: {err:?}"
    );
}

#[test]
fn negation_rejects_datetime() {
    let source = "node t: Datetime<UTC> = -datetime(\"2026-01-01T00:00:00Z\");";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { found, .. }), .. }) if found.contains("Datetime")),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { expected, .. }), .. }) if expected == "indexed quantity collection"),
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { expected, .. }), .. }) if expected == "indexed quantity collection"),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_bare_int_on_dimensioned_rejected() {
    // Integer literal on a dimensioned quantity should also be rejected.
    let source = "param m: Mass(min: 1, max: 100.0 kg) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_creates_wrong_dimension() {
    // 1.0 m / 1.0 s is Velocity, but the constrained type is Length.
    let source = "param d: Length(min: 1.0 m / 1.0 s) = 5.0 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_inverse_dimension() {
    // 1.0 / 1.0 kg is 1/Mass, not Mass.
    let source = "param x: Mass(min: 1.0 / 1.0 kg) = 5.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_addition_unit_mismatch_in_bound() {
    // 5.0 m + 3.0 s is itself a dimension mismatch inside the bound expression.
    let source = "param t: Time(min: 5.0 m + 3.0 s) = 10.0 s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
Maneuver#Departure: 1.0 m / s,
Maneuver#Correction: 0.5 m / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::IntDomainBoundTypeMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_with_unit_rejected() {
    let source = "param n: Int(min: 1.0 kg, max: 10.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::IntDomainBoundTypeMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_arithmetic_with_unit_rejected() {
    // Arithmetic that produces a dimensioned result is also rejected.
    let source = "param n: Int(min: 1.0 m / 1.0 s) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::IntDomainBoundTypeMismatch { .. }),
                ..
            })
        ),
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Domain(DomainError::DatetimeDomainBoundTypeMismatch { .. }),
            ..
        })
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Domain(DomainError::DatetimeDomainBoundTypeMismatch { .. }),
            ..
        })
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn const_domain_bound_int_with_unit_rejected() {
    let source = "const node MAX_N: Int(min: 1.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::IntDomainBoundTypeMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            })
        ),
        "got: {error:?}"
    );
}

#[test]
fn model_schema_rejects_undischarged_generic_field_obligation() {
    let source = r"
pub type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
param port: Box<Time>;
";
    let (_, _, _, invalid_args) = model_port_application(source);
    let (tir, src, identity, valid_args) =
        model_port_application(&source.replace("Box<Time>", "Box<Length>"));
    let tir = check_draft(tir, src).unwrap();
    ConcreteModelType::try_new(&tir, &identity, &valid_args, src).unwrap();
    let error = ConcreteModelType::try_new(&tir, &identity, &invalid_args, src).unwrap_err();
    assert!(
        matches!(
            error,
            ConcreteModelTypeError::Compiler(SemanticError::Located(
                crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                    ..
                }
            ))
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
    let tir = check_draft(tir, src).unwrap();
    assert_eq!(generic_args.len(), 1);

    let too_few = ConcreteModelType::try_new(&tir, &identity, &[], src).unwrap_err();
    assert!(matches!(
        too_few,
        ConcreteModelTypeError::GenericArityMismatch {
            expected: 1,
            actual: 0,
            ..
        }
    ));

    let too_many_args = vec![generic_args[0].clone(), generic_args[0].clone()];
    let too_many = ConcreteModelType::try_new(&tir, &identity, &too_many_args, src).unwrap_err();
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
    let tir = check_draft(tir, src).unwrap();
    let wrong_sort = [CheckedGenericArg::Type(CheckedType::Int)];

    let error = ConcreteModelType::try_new(&tir, &identity, &wrong_sort, src).unwrap_err();
    assert!(matches!(
        error,
        ConcreteModelTypeError::GenericSortMismatch {
            expected: crate::syntax::ast::GenericConstraint::Nat,
            actual: crate::syntax::ast::GenericConstraint::Type,
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
    let tir = check_draft(tir, src).unwrap();
    assert_eq!(generic_args.len(), 2);

    let omitted_defaults = ConcreteModelType::try_new(&tir, &identity, &[], src).unwrap_err();
    assert!(matches!(
        omitted_defaults,
        ConcreteModelTypeError::GenericArityMismatch {
            expected: 2,
            actual: 0,
            ..
        }
    ));

    let model_type = ConcreteModelType::try_new(&tir, &identity, &generic_args, src).unwrap();
    let constructors = model_type.constructors(src).unwrap();
    assert_eq!(constructors.len(), 1);
    assert_eq!(constructors[0].fields().len(), 1);
    assert_eq!(
        constructors[0].fields()[0].declared_type(),
        &CheckedType::Int
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
    let tir = check_draft(tir, src).unwrap();
    let model_type = ConcreteModelType::try_new(&tir, &identity, &generic_args, src).unwrap();
    let constructors = model_type.constructors(src).unwrap();

    assert!(matches!(
        constructors[0].fields()[0].declared_type(),
        CheckedType::Struct(_, nested_args)
            if matches!(nested_args.as_slice(), [CheckedGenericArg::Type(CheckedType::Int)])
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
    let tir = check_draft(tir, src).unwrap();

    let validated = ValidatedModelType::try_new(&tir, &identity, &generic_args, src).unwrap();
    assert_eq!(validated.constructors(src).unwrap().len(), 1);

    let error = ConcreteModelType::try_new(&tir, &identity, &generic_args, src).unwrap_err();
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
        matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind), .. })
            if kind.to_string().contains("out of bounds for Fin(1)")),
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
        matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::InvalidFiniteIndexCardinality { .. }), .. })
            if kind.to_string().contains("finite index size must be greater than zero")
                || kind.to_string().contains("Fin(0)")),
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
        matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind), .. })
            if kind.to_string().contains("index 1 out of bounds for Fin(1)")),
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
        matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::NegativeIndex { .. }), .. })
            if kind.to_string().contains("negative value: -1")),
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
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
            ..
        })
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
        matches!(
            error,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                ..
            })
        ),
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
node doubled: Length = @scale(factor: 2.0, v: @src)::result;
";

#[test]
fn inline_dag_call_basic_returns_output_type() {
    let types = check(INLINE_DAG_CALL_SCALE).unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("doubled"))],
        CheckedType::Quantity(length)
    );
}

#[test]
fn inline_dag_call_can_project_defaulted_or_bound_param() {
    let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}

node default_factor: Dimensionless = @config()::factor;
node bound_factor: Dimensionless = @config(factor: 3.0)::factor;
";
    let types = check(source).unwrap();
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(
            "default_factor"
        ))],
        CheckedType::Quantity(Dimension::dimensionless())
    );
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(
            "bound_factor"
        ))],
        CheckedType::Quantity(Dimension::dimensionless())
    );
}

#[test]
fn inline_dag_call_unknown_dag() {
    let source = "\
param src: Length = 10.0 m;
node y: Length = @nope(v: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }) if kind.to_string().contains("unknown module")),
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
node y: Length = @id_len(bogus: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::UnknownLocalRef { .. }),
                ..
            })
        ),
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
node y: Length = @scale(v: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Graph(GraphError::MissingDagBindings { missing, .. }), .. }) if missing.iter().map(ToString::to_string).collect::<Vec<_>>() == ["factor"]),
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
node y: Length = @id_len(v: @src)::nope;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Struct(StructError::UnknownLocalRef { .. }),
                ..
            })
        ),
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
node y: Length = @id_len(v: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Graph(GraphError::DagArgTypeMismatch { .. }),
                ..
            })
        ),
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

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };
node distances: Length[Region] = for r: Region { @id_len(v: @dist[r])::result };
";
    let types = check(source).unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(
            "distances"
        ))],
        CheckedType::Indexed {
            element: Box::new(CheckedType::Quantity(length)),
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
node y: Length = @bogus(v: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
        "expected DimensionMismatch from inside dag body, got: {err:?}"
    );
}

#[test]
fn inline_dag_indexed_output_type_flows_through() {
    let source = "\
pub index Region = { A, B };

dag doubler {
    import test::{ index Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 3.0 m };
node out: Length = @doubler(v: @dist)::result[Region#A];
";
    let types = check(source).unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    assert_eq!(
        types[&ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("out"))],
        CheckedType::Quantity(length)
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
node y: Length = @private_result(v: @src)::hidden;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Visibility(VisibilityError::ImportPrivateItem { .. }),
                ..
            })
        ),
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
    pub node result: Length = @loop_self(v: @v)::result;
}

param src: Length = 1.0 m;
node y: Length = @loop_self(v: @src)::result;
";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { .. }),
                ..
            })
        ),
        "expected CyclicDependency, got: {err:?}"
    );
}

#[test]
fn inline_dag_mutual_recursion_cycle_detected() {
    let source = "\
dag a {
    param v: Length;
    pub node out: Length = @b(v: @v)::out;
}

dag b {
    param v: Length;
    pub node out: Length = @a(v: @v)::out;
}

param src: Length = 1.0 m;
node y: Length = @a(v: @src)::out;
";
    let err = check(source).unwrap_err();
    // The search visits `a` first and re-enters it from the call inside `b`.
    let SemanticError::Located(crate::diagnostic::Diagnostic {
        kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { name, .. }),
        primary: span,
        ..
    }) = &err
    else {
        panic!("expected CyclicDependency, got: {err:?}");
    };
    assert!(name.to_string().ends_with('a'), "{name}");
    let dag_b = source.find("dag b").unwrap();
    let param_src = source.find("param src").unwrap();
    assert!((dag_b..param_src).contains(&span.offset()), "{span:?}");
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
node y: Length = @forward(v: @src)::b;
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
            matches!(
                err,
                SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(DimensionError::DimensionOverflow),
                    ..
                })
            ),
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
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::FloatPowerExponent {
                    replacement: None,
                    ..
                }),
                ..
            })
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
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                ..
            })
        ),
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
            matches!(
                check(&source),
                Err(SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { .. }),
                    ..
                }))
            ),
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
                Err(SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::PlotEncodingTypeMismatch { channel: crate::syntax::ast::EncodingChannel::X, found, .. }), .. })) if found == expected
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
        SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::PlotEncodingAxisMismatch { channels, .. }), .. })
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
        x: for phase_key: Phase { phase_key },
        y: 1.0,
        color: for phase_key: Phase, step: Step { 1 },
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
        text: Phase#A,
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
        Err(SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::IneffectiveConversion),
            ..
        }))
    ));
}

#[test]
fn check_unknown_plot_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, caption: \"typo\" };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::InvalidPlotProperty { property, .. }), .. }) if property == "caption"),
        "got: {err:?}"
    );
}

#[test]
fn check_unknown_mark_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line { strokewidth: 3.0 }, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::InvalidPlotProperty { property, .. }), .. }) if property == "strokewidth"),
        "got: {err:?}"
    );
}

#[test]
fn check_string_property_with_number_value_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, title: 42.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::PlotPropertyTypeMismatch {
                    property: "title",
                    ..
                }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_numeric_property_with_string_value_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } }, width: \"wide\" };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::PlotPropertyTypeMismatch {
                    property: "width",
                    ..
                }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_dimensioned_mark_property_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line { stroke_width: 2.0 m }, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::PlotPropertyDimensioned {
                    property: "stroke_width",
                    ..
                }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_figure_width_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [p], width: 300.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::InvalidPlotProperty { property, context: "a figure declaration", .. }), .. }) if property == "width"),
        "got: {err:?}"
    );
}

#[test]
fn check_layer_width_is_accepted() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
layer l = { plots: [p], width: 300.0, title: \"ok\" };";
    check(source).unwrap();
}

#[test]
fn check_valid_plot_properties_pass() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
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
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot real_plot = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [real_plot, my_polt] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::UnknownPlotReference { owner_kind: "figure", name, .. }), .. }) if name.to_string() == "my_polt"),
        "got: {err:?}"
    );
}

#[test]
fn check_figure_referencing_figure_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure inner = { plots: [p] };
figure outer_figure = { plots: [inner] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::CompositionReferencesNonPlot {
                    actual_kind: "figure",
                    ..
                }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_duplicate_plot_reference_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
figure f = { plots: [p, p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::DuplicatePlotReference { .. }),
                ..
            })
        ),
        "got: {err:?}"
    );
}

#[test]
fn check_valid_plot_references_pass() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
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
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
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
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Attribute(AttributeError::InvalidHiddenTarget { kind, .. }), .. })
        if kind == &crate::declaration_kind::AttributeTarget::declaration(
            crate::declaration_kind::DeclarationKind::Node,
        )),
        "got: {err:?}"
    );
}

#[test]
fn check_hidden_on_figure_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };
#[hidden]
figure f = { plots: [p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Attribute(AttributeError::InvalidHiddenTarget { kind, .. }), .. })
        if kind == &crate::declaration_kind::AttributeTarget::declaration(
            crate::declaration_kind::DeclarationKind::Figure,
        )),
        "got: {err:?}"
    );
}

#[test]
fn check_hidden_with_args_is_rejected() {
    let source = "\
pub index Step = { A, B };
param vals: Dimensionless[Step] = { Step#A: 1.0, Step#B: 2.0 };
#[hidden(now)]
plot p = { mark: line, encode: { x: for s: Step { @vals[s] } } };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(&err, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Attribute(kind @ AttributeError::HiddenTakesNoArguments), .. }) if kind.to_string().contains("no arguments")),
        "got: {err:?}"
    );
}

#[test]
fn resolved_constructor_carries_owning_definition_and_field_constraints() {
    use crate::syntax::type_name::{ConstructorName, FieldName};
    let (tir, _src) = module_aware_tir(
        "type Maneuver {\n    Burn(dv: Dimensionless(min: 0.0), label: Dimensionless),\n    Coast,\n}\nnode m: Maneuver = Coast;",
    );
    let lookup = |name: &str| {
        tir.project_type_store()
            .lookup_constructor(&crate::resolved_name::ResolvedConstructorName::for_test(
                test_dag_id(),
                ConstructorName::expect_valid(name),
            ))
            .unwrap()
    };
    let burn = lookup("Burn");
    let coast = lookup("Coast");
    assert!(std::sync::Arc::ptr_eq(
        burn.definition(),
        coast.definition()
    ));
    assert_eq!(burn.owning_type(), burn.definition().identity());
    assert_eq!(burn.name(), ConstructorName::expect_valid("Burn"));
    assert_eq!(
        burn.constrained_fields().cloned().collect::<Vec<_>>(),
        vec![FieldName::expect_valid("dv")]
    );
    assert!(burn.constrains(&FieldName::expect_valid("dv")));
    assert!(!burn.constrains(&FieldName::expect_valid("label")));
    assert_eq!(coast.constrained_fields().count(), 0);
    assert_eq!(burn, &burn.clone());
    assert_ne!(burn, coast);
    let members = crate::hir::nominal::ResolvedConstructor::members_of(burn.definition())
        .map(|member| member.name())
        .collect::<Vec<_>>();
    assert_eq!(
        members,
        vec![
            ConstructorName::expect_valid("Burn"),
            ConstructorName::expect_valid("Coast")
        ]
    );
}

#[test]
fn resolved_nominal_aligns_field_semantics_with_its_definition() {
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};
    let (tir, _src) = module_aware_tir(
        "type Maneuver {\n    Burn(dv: Dimensionless(min: 0.0), label: Dimensionless),\n    Coast,\n}\ntype Point { Point(x: Dimensionless, y: Dimensionless(max: 1.0)) }\nnode m: Maneuver = Coast;\nnode p: Point = Point(x: 1.0, y: 0.5);",
    );
    let constructor = |name: &str| {
        tir.project_type_store()
            .lookup_constructor(&crate::resolved_name::ResolvedConstructorName::for_test(
                test_dag_id(),
                ConstructorName::expect_valid(name),
            ))
            .unwrap()
    };
    let type_name = |name: &str| {
        ResolvedStructTypeName::for_test(test_dag_id(), StructTypeName::expect_valid(name))
    };
    let defs = &tir.root().semantic.type_defs;
    assert!(defs.contains(&type_name("Maneuver")));
    assert!(!defs.contains(&type_name("Missing")));
    let maneuver = defs.nominal(&type_name("Maneuver")).unwrap();
    assert_eq!(maneuver.identity(), &type_name("Maneuver"));

    // Members and fields come in definition order, each with its semantics.
    let shape = maneuver
        .members()
        .map(|member| {
            (
                member.constructor().name().to_string(),
                member
                    .fields()
                    .map(|field| {
                        (
                            field.field().name().to_string(),
                            field.semantics().domain_bounds().is_some(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            (
                "Burn".to_string(),
                vec![("dv".to_string(), true), ("label".to_string(), false)]
            ),
            ("Coast".to_string(), vec![]),
        ]
    );

    // A constructor resolves to its own member, never to another type's.
    let coast = constructor("Coast");
    assert_eq!(coast.position(), 1);
    assert_eq!(
        maneuver.member_of(coast).unwrap().constructor().name(),
        ConstructorName::expect_valid("Coast")
    );
    assert!(maneuver.member_of(constructor("Point")).is_none());
    let burn = defs.member(constructor("Burn")).unwrap();
    assert_eq!(burn.nominal().identity(), &type_name("Maneuver"));
    assert!(burn.field(&FieldName::expect_valid("missing")).is_none());
    let dv = burn.field(&FieldName::expect_valid("dv")).unwrap();
    assert_eq!(dv.member().constructor().name(), burn.constructor().name());
    assert_eq!(dv.display_name(), "Maneuver.Burn.dv");

    // Only a one-constructor type named like its constructor is a record.
    assert!(maneuver.record_member().is_none());
    let point = defs.nominal(&type_name("Point")).unwrap();
    let record = point.record_member().unwrap();
    assert_eq!(
        record
            .field(&FieldName::expect_valid("y"))
            .unwrap()
            .display_name(),
        "Point.y"
    );

    let mut constrained = defs
        .constrained_fields()
        .map(|field| (field.field().display_name(), field.bounds().len()))
        .collect::<Vec<_>>();
    constrained.sort();
    assert_eq!(
        constrained,
        vec![
            ("Maneuver.Burn.dv".to_string(), 1),
            ("Point.y".to_string(), 1)
        ]
    );
}

#[test]
fn check_match_foreign_constructor_names_the_constructor_member() {
    use crate::semantic_error::structure::NominalMember;
    use crate::syntax::type_name::ConstructorName;
    let source = "\
pub type Maybe { Some(value: Length), None }
pub type Other { Elsewhere }
param x: Maybe = Some(value: 1.0 m);
node y: Length = match @x { Elsewhere => 1.0 m, Some(value: v) => v, None => 0.0 m };";
    let err = check(source).unwrap_err();
    let SemanticError::Located(crate::diagnostic::Diagnostic {
        kind: SemanticErrorKind::Struct(StructError::UnknownField { member, .. }),
        ..
    }) = &err
    else {
        panic!("got: {err:?}");
    };
    assert_eq!(
        member,
        &NominalMember::Constructor(ConstructorName::expect_valid("Elsewhere"))
    );
    assert_eq!(
        err.to_string(),
        "unknown field `Elsewhere` on struct `Maybe`"
    );
}

fn root_decl(name: &str) -> ResolvedDeclName {
    ResolvedDeclName::for_test(test_dag_id(), DeclName::expect_valid(name))
}

#[test]
fn checker_retains_dependency_then_source_ordered_schedules() {
    let source = "const node c: Dimensionless = @b + 1.0;\n\
                  const node a: Dimensionless = 1.0;\n\
                  const node b: Dimensionless = @a + 1.0;\n\
                  node n: Dimensionless = @m + @c;\n\
                  param p: Dimensionless = 1.0;\n\
                  node m: Dimensionless = @p + 1.0;\n\
                  node q: Dimensionless = 2.0;";
    let (draft, src) = module_aware_tir(source);
    let tir = check_draft(draft.clone(), src).unwrap();

    let constants = tir.const_schedule();
    assert_eq!(
        constants.dags(),
        [crate::tir::typed::dag_position::DagPosition::ROOT]
    );
    assert_eq!(
        constants.order().as_slice(),
        [root_decl("a"), root_decl("b"), root_decl("c")]
    );

    let runtime = tir.root().runtime_schedule();
    assert_eq!(runtime.execution_dags(), [test_dag_id()]);
    assert_eq!(
        runtime.order().as_slice(),
        [
            root_decl("p"),
            root_decl("q"),
            root_decl("m"),
            root_decl("n")
        ]
    );
    let steps = runtime
        .steps()
        .map(|(declaration, reads)| (declaration.clone(), reads.to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(
        steps,
        [
            (root_decl("p"), vec![]),
            (root_decl("q"), vec![]),
            (root_decl("m"), vec![root_decl("p")]),
            (root_decl("n"), vec![root_decl("c"), root_decl("m")]),
        ]
    );
    assert_eq!(
        runtime.dependencies_of(&root_decl("n")),
        Some(&[root_decl("c"), root_decl("m")][..])
    );
    assert_eq!(runtime.dependencies_of(&root_decl("c")), None);

    // A new checking revision rebuilds the same schedules.
    let retained = check_draft(draft, src).unwrap();
    assert_eq!(retained.root().runtime_schedule(), runtime);
    assert_eq!(retained.const_schedule(), constants);
}

#[test]
fn declaration_cycles_are_reported_deterministically_at_the_closing_declaration() {
    for (source, expected) in [
        (
            "const node a: Dimensionless = @c;\n\
             const node b: Dimensionless = @a;\n\
             const node c: Dimensionless = @b;",
            "b",
        ),
        (
            "node x: Dimensionless = @z;\n\
             node y: Dimensionless = @x;\n\
             node z: Dimensionless = @y;",
            "y",
        ),
        (
            "param seed: Dimensionless = 1.0;\n\
             node a: Dimensionless = @b + @seed;\n\
             node b: Dimensionless = @a;",
            "b",
        ),
    ] {
        for _ in 0..4 {
            let (tir, src) = module_aware_tir(source);
            let error = check_draft(tir, src).unwrap_err();
            assert!(
                matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { name, .. }), .. }) if name.to_string() == expected),
                "{source}: {error:?}"
            );
        }
    }
}

#[test]
fn call_arguments_prechecked_for_override_reconciliation_are_inferred_once() {
    let source = "node value: Dimensionless = sqrt(4.0) + sqrt(9.0);";
    let (mut tir, src) = module_aware_tir(source);
    let owner = root_decl("value");
    let reconciliation = crate::ir::override_reconciliation::OverrideReconciliation::new(
        root_decl("value"),
        &crate::ir::static_substitution::StaticSubstitution::default(),
        src,
        Span::new(0, 0),
    );
    tir.root_mut()
        .semantic
        .override_reconciliations
        .insert(owner, vec![reconciliation]);
    let tir = check_draft(tir, src).unwrap();
    assert_eq!(count_nodes(tir.root().bodies(), |_| true), 5);
}

#[test]
fn constructor_calls_place_written_fields_at_their_declared_fields() {
    use crate::semantic::struct_value::StructFieldsError;
    use crate::tir::texpr::{TConstruct, TExprKind};

    let source = "type Pair { Pair(left: Dimensionless, right: Dimensionless) }\n\
                  node p: Pair = Pair(right: 2.0, left: 1.0);";
    let (tir, src) = module_aware_tir(source);
    let tir = check_draft(tir, src).unwrap();
    let dag = tir.root();
    let formula = dag
        .body()
        .nodes()
        .find(|entry| entry.name().as_str() == "p")
        .and_then(|entry| entry.definition.formula())
        .unwrap();
    let tree = dag.bodies().executable_value(formula.id()).unwrap();
    let TExprKind::Construct(call) = tree.kind() else {
        panic!("expected a constructor call, got {tree:?}");
    };
    let names = |fields: &[crate::tir::texpr::TFieldInit]| {
        fields
            .iter()
            .map(|field| field.name.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    let written = call.fields().cloned().collect::<Vec<_>>();
    assert_eq!(names(&written), ["right", "left"]);

    // Fields are evaluated in written order and stored in declared order.
    let mut order = Vec::new();
    let value = call
        .apply(|init| {
            order.push(init.name.as_str().to_owned());
            Ok::<_, ()>(init.name.as_str().to_owned())
        })
        .unwrap();
    assert_eq!(order, ["right", "left"]);
    let fields = value
        .fields()
        .map(|(name, value)| (name.as_str().to_owned(), value.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        [
            ("left".to_owned(), "left".to_owned()),
            ("right".to_owned(), "right".to_owned())
        ]
    );
    // The first failing field, in written order, is the call's failure.
    assert_eq!(
        call.apply(|init| Err::<(), _>(init.name.as_str().to_owned()))
            .unwrap_err(),
        "right"
    );

    // A call admits exactly the application's declared fields.
    let application = || call.application().clone();
    let (right, left) = (written[0].clone(), written[1].clone());
    assert!(matches!(
        TConstruct::try_new(application(), vec![left.clone()]),
        Err(StructFieldsError::Missing { field, .. }) if field.as_str() == "right"
    ));
    let mut extra = left.clone();
    extra.name = crate::syntax::type_name::FieldName::expect_valid("extra");
    assert!(matches!(
        TConstruct::try_new(application(), vec![left.clone(), right.clone(), extra]),
        Err(StructFieldsError::Unexpected { field, .. }) if field.as_str() == "extra"
    ));
    assert!(matches!(
        TConstruct::try_new(application(), vec![left.clone(), left, right]),
        Err(StructFieldsError::Duplicate { field, .. }) if field.as_str() == "left"
    ));
}

#[test]
fn inference_emits_typed_trees_carrying_node_facts() {
    use crate::tir::texpr::{DatetimeLiteral, TConstRef, TExprKind, TIndexArg, TMatchArms};

    let source = "type Maneuver { Impulsive(delta_v: Dimensionless), Coast }\n\
                  node burn: Maneuver = Impulsive(delta_v: 2.0);\n\
                  node coast: Maneuver = Coast;\n\
                  node picked: Dimensionless = match @burn {\n\
                      Impulsive(delta_v: dv) => dv,\n\
                      Coast => 0.0,\n\
                  };\n\
                  node v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };\n\
                  node picked_entry: Dimensionless = @v[1];\n\
                  node when: Datetime<UTC> = datetime(\"2026-01-01T00:00:00Z\");\n\
                  plot p = { mark: point, encode: { x: @v, y: @v, color: \"red\" } };";
    let (tir, src) = module_aware_tir(source);
    let tir = check_draft(tir, src).unwrap();
    let dag = tir.root();
    let bodies = dag.bodies();
    let root = |name: &str| {
        let formula = dag
            .body()
            .nodes()
            .find(|entry| entry.name().as_str() == name)
            .and_then(|entry| entry.definition.formula())
            .unwrap();
        bodies
            .executable_value(formula.id())
            .unwrap_or_else(|error| panic!("`{name}` has no typed value root: {error}"))
    };

    let TExprKind::Construct(burn) = root("burn").kind() else {
        panic!("expected a constructor application");
    };
    assert_eq!(burn.application().constructor.name().as_str(), "Impulsive");
    assert_eq!(burn.fields().len(), 1);
    let coast = match root("coast").kind() {
        TExprKind::Const(crate::syntax::span::Spanned {
            value: TConstRef::Constructor(construct),
            ..
        })
        | TExprKind::Construct(construct) => construct.application(),
        other => panic!("expected a constructor application, got {other:?}"),
    };
    assert_eq!(coast.constructor.name().as_str(), "Coast");

    let TExprKind::Match {
        arms: TMatchArms::Constructors(arms),
        ..
    } = root("picked").kind()
    else {
        panic!("expected a constructor match");
    };
    let targets: Vec<_> = arms
        .iter()
        .map(|arm| arm.target.constructor.as_str().to_string())
        .collect();
    assert_eq!(targets, ["Impulsive", "Coast"]);

    let TExprKind::Index { args, .. } = root("picked_entry").kind() else {
        panic!("expected an index access");
    };
    assert!(matches!(
        args.first(),
        TIndexArg::Position { position, .. } if position.position == 1
    ));

    assert!(matches!(
        root("when").kind(),
        TExprKind::DatetimeLiteral(DatetimeLiteral::Offset(_))
    ));

    let plot = dag.body().plots().next().unwrap();
    let color = plot
        .body
        .encodings
        .iter()
        .map(|(_, expr)| expr)
        .find(|expr| matches!(expr.kind(), crate::hir::expr::ExprKind::StringLiteral(_)))
        .unwrap();
    assert!(bodies.contextual(color.id()).is_some());
}
