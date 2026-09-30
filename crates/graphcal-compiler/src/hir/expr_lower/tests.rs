use super::tolerant::Tolerant;
use crate::hir::expr::Draft;
use crate::resolved_name::ResolvedDeclName;
use std::collections::{BTreeSet, HashMap};

use crate::builtin::{BuiltinFn, ComplexFn};
use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolve::ModuleResolver;
use crate::resolve::category::DeclSymbolKind;
use crate::resolve::error::ModuleResolveError;
use crate::semantic::time_scale::TimeScale;
use crate::semantic::time_zone::TimeZoneRegistry;
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use super::call::lowering_arity;
use super::context::{BindingOverlay, ExprLoweringContext, FrozenBindings};
use super::error::ExprLowerError;
use super::lower::{lower_expr, lower_expr_tolerant};
use crate::hir::expr::LocalEnv;
use crate::hir::expr::{
    AssertBody, ConstRef, Expr, ExprKind, FunctionRef, LocalId, MatchPattern, PatternBinding,
};
use crate::hir::expr::{CheckedAssertBody, CheckedExpr};
use crate::hir::expr::{collect_expr_dependencies, visit_expr};
use crate::hir::lower::GenericScope;
use crate::hir::lower::ModuleScope;
use crate::syntax::parser::Parser;

fn desugared_source(source: &str) -> ast::File {
    let raw = Parser::new(source).parse_file().unwrap();
    crate::desugar::desugared_ast::File::from(raw)
}

#[test]
fn lowering_checks_arity_of_scalar_kernels_and_real_overloads_only() {
    for function in BuiltinFn::all() {
        let expected = match function {
            BuiltinFn::Scalar(scalar) => Some(scalar.arity()),
            BuiltinFn::Complex(ComplexFn::Absolute | ComplexFn::Exponential) => Some(1),
            _ => None,
        };
        assert_eq!(lowering_arity(function), expected, "`{function}`");
    }
    assert_eq!(lowering_arity(BuiltinFn::parse("clamp").unwrap()), Some(3));
    assert_eq!(lowering_arity(BuiltinFn::parse("complex").unwrap()), None);
}

#[test]
fn local_env_layers_frames_without_cloning() {
    let a = LocalId(0);
    let b = LocalId(1);
    let c = LocalId(2);

    let root: LocalEnv<'_, i32> = LocalEnv::root();
    assert_eq!(root.get(a), None);

    let outer = root.child(vec![(a, 1)]);
    assert_eq!(outer.get(a), Some(&1));
    assert_eq!(outer.get(b), None);

    let inner = outer.child(vec![(b, 2)]);
    assert_eq!(inner.get(a), Some(&1));
    assert_eq!(inner.get(b), Some(&2));

    // A child frame never leaks into its parent.
    assert_eq!(outer.get(b), None);

    let seeded = LocalEnv::from_bindings(vec![(c, 7)]);
    assert_eq!(seeded.get(c), Some(&7));
}

#[test]
fn local_env_bind_rebinds_in_place() {
    let a = LocalId(0);
    let b = LocalId(1);
    let root: LocalEnv<'_, i32> = LocalEnv::root();
    let mut frame = root.child(Vec::new());

    // Iterating binders rebind the same id once per element.
    for value in 0..3 {
        frame.bind(a, value);
        assert_eq!(frame.get(a), Some(&value));
    }
    frame.bind(b, 10);
    assert_eq!(frame.get(a), Some(&2));
    assert_eq!(frame.get(b), Some(&10));
}

#[test]
fn body_identity_and_source_map_do_not_depend_on_unique_spans() {
    let span = Span::new(0, 1);
    let leaf = Expr::<Draft>::new(ExprKind::Bool(true), span);
    let body = CheckedExpr::finish(Expr::new(
        ExprKind::If {
            condition: Box::new(leaf.clone()),
            then_branch: Box::new(leaf.clone()),
            else_branch: Box::new(leaf),
        },
        span,
    ))
    .unwrap();
    let mut ids = std::collections::HashSet::new();
    visit_expr(&body, &mut |expr| {
        assert!(ids.insert(expr.id().clone()));
        assert_eq!(body.source_map().span(expr.id()).unwrap(), span);
    });
    assert_eq!(ids.len(), 4);
    let mut shifted = (*body).clone();
    shifted.span = Span::new(99, 1);
    assert_eq!(shifted.id(), body.id());
    assert_eq!(body.source_map().span(shifted.id()).unwrap(), span);
    let rebuilt = CheckedExpr::finish(shifted.into_draft_for_test()).unwrap();
    assert_ne!(rebuilt.id(), body.id());
    assert!(rebuilt.source_map().span(body.id()).is_err());
}

#[test]
fn tolerance_operands_have_distinct_ids_even_with_identical_source_coordinates() {
    let leaf = || Box::new(Expr::new(ExprKind::Number(1.0), Span::new(0, 1)));
    let body = CheckedAssertBody::finish(AssertBody::Tolerance {
        actual: leaf(),
        expected: leaf(),
        tolerance: leaf(),
    })
    .unwrap();
    let ids = body
        .expressions()
        .map(|expr| expr.id().clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| body.source_map().span(id).is_ok()));
}

#[test]
fn strict_lowering_publishes_identity_and_source_coverage_for_every_child() {
    let owner = DagId::root_in_package("test", "identities");
    let file = desugared_source("node value: Dimensionless = 1.0 + 2.0;");
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();
    let body = lower_expr(
        node_value(&file, "value"),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap();
    let mut count = 0;
    visit_expr(&body, &mut |expr| {
        assert_eq!(body.source_map().span(expr.id()).unwrap(), expr.span);
        count += 1;
    });
    assert_eq!(count, 3);
}

fn node_value<'a>(file: &'a ast::File, name: &str) -> &'a ast::Expr {
    file.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Node(node) if node.name.value.as_str() == name => {
                node.definition.formula()
            }
            _ => None,
        })
        .expect("source should contain requested node")
}

fn resolver_with_import(
    lib_id: &DagId,
    main_id: &DagId,
    lib: &ast::File,
    main: &ast::File,
) -> ModuleResolver {
    // Every import of `main` names `lib`.
    ModuleResolver::build(
        [
            (lib_id.clone(), lib.declarations.as_slice()),
            (main_id.clone(), main.declarations.as_slice()),
        ],
        &|owner: &DagId, _: &crate::syntax::ast::ModulePath| {
            (owner == main_id).then(|| lib_id.clone())
        },
    )
    .unwrap()
}

#[test]
fn lowers_qualified_index_variant_literal_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index Phase = { Burn, Coast };");
    let main_source = "import lib as mission; node phase: Dimensionless = mission::Phase#Burn;";
    let main = desugared_source(main_source);
    let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&main, "phase"),
        ExprLoweringContext::new(
            ModuleScope::new(&main_id, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::VariantLiteral(variant) = expr.kind() else {
        panic!("expected variant literal, got {expr:?}");
    };
    assert_eq!(variant.variant.index().owner(), &lib_id);
    assert_eq!(variant.variant.index().as_str(), "Phase");
    assert_eq!(variant.variant.variant().as_str(), "Burn");
    // Segment spans address exactly the written path parts.
    let slice =
        |span: crate::syntax::span::Span| &main_source[span.offset()..span.offset() + span.len()];
    assert_eq!(slice(variant.variant_span), "Burn");
    assert_eq!(
        slice(variant.index_span.expect("written index path")),
        "mission::Phase"
    );
}

#[test]
fn lowers_qualified_quantity_literal_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub base dim Currency; pub base unit credit: Currency;");
    let main =
        desugared_source("import lib as schema; node amount: Dimensionless = 1.0 schema::credit;");
    let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&main, "amount"),
        ExprLoweringContext::new(
            ModuleScope::new(&main_id, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::QuantityLiteral { unit, .. } = expr.kind() else {
        panic!("expected quantity literal, got {expr:?}");
    };
    let [term] = unit.terms.as_slice() else {
        panic!("expected one unit term, got {:?}", unit.terms);
    };
    assert_eq!(term.name.value.spelling().to_string(), "schema::credit");
    assert_eq!(term.name.value.static_definition().owner(), &lib_id);
    assert_eq!(term.name.value.static_definition().as_str(), "credit");
}

#[test]
fn lowers_qualified_nullary_constructor_const_ref_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type BurnKind { Impulsive, Coast }");
    let main =
        desugared_source("import lib as mission; node burn: Dimensionless = mission::Impulsive;");
    let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&main, "burn"),
        ExprLoweringContext::new(
            ModuleScope::new(&main_id, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::ConstRef(target) = expr.kind() else {
        panic!("expected const-like ref, got {expr:?}");
    };
    let ConstRef::Constructor(constructor) = &target.value else {
        panic!("expected constructor, got {target:?}");
    };
    assert_eq!(constructor.owner(), &lib_id);
    assert_eq!(constructor.as_str(), "Impulsive");
}

#[test]
fn lowers_unambiguous_timezone_datetime_to_resolved_hir() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source(
        "node t: Datetime = datetime(\"2024-07-15T17:30:00\", \"America/New_York\");",
    );
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&file, "t"),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::FnCall { args, .. } = expr.kind() else {
        panic!("expected function call, got {expr:?}");
    };
    let [datetime, time_zone] = args.as_slice() else {
        panic!("expected two arguments, got {args:?}");
    };
    let (ExprKind::ZonedDateTimeLiteral(datetime), ExprKind::IanaTimeZoneLiteral(time_zone)) =
        (datetime.kind(), time_zone.kind())
    else {
        panic!("expected resolved datetime and timezone arguments, got {args:?}");
    };
    assert_eq!(datetime.time_zone(), time_zone);
    assert_eq!(
        datetime.timestamp(),
        "2024-07-15T21:30:00Z".parse::<jiff::Timestamp>().unwrap()
    );
}

#[test]
fn lowers_epoch_static_scale_and_civil_literal_to_typed_hir() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("node t: Datetime<TT> = epoch<TT>(\"2024-11-05T12:00:00\");");
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&file, "t"),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::FnCall { callee, args } = expr.kind() else {
        panic!("expected function call, got {expr:?}");
    };
    let FunctionRef::Epoch { scale } = &callee.value else {
        panic!("expected typed epoch reference, got {callee:?}");
    };
    assert_eq!(scale.value, TimeScale::TT);
    assert_eq!(args.len(), 1);
    assert!(matches!(args[0].kind(), ExprKind::CivilDateTimeLiteral(_)));
}

#[test]
fn lowers_for_locals_to_lexical_ids() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source(
        "index Phase = { Burn }; node x: Dimensionless[Phase] = for p: Phase { p };",
    );
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&file, "x"),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::ForComp { bindings, body } = expr.kind() else {
        panic!("expected for comp, got {expr:?}");
    };
    let [binding] = bindings.as_slice() else {
        panic!("expected one binding, got {bindings:?}");
    };
    let ExprKind::LocalRef(local) = body.kind() else {
        panic!("expected local ref, got {body:?}");
    };
    assert_eq!(binding.local.id, local.value);
}

#[test]
fn lowers_qualified_constructor_match_pattern_and_binding() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type BurnKind { Impulsive(delta_v: Dimensionless), Coast }");
    let main = desugared_source(
        "import lib as mission; param burn: Dimensionless; \
         node dv: Dimensionless = match @burn { mission::Impulsive(delta_v: delta) => delta, mission::Coast => 0.0 };",
    );
    let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&main, "dv"),
        ExprLoweringContext::new(
            ModuleScope::new(&main_id, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap()
    .into_expr_for_test();

    let ExprKind::Match { arms, .. } = expr.kind() else {
        panic!("expected match, got {expr:?}");
    };
    let [first, _second] = arms.as_slice() else {
        panic!("expected two arms, got {arms:?}");
    };
    let MatchPattern::Constructor {
        constructor,
        bindings,
        ..
    } = &first.pattern
    else {
        panic!("expected constructor pattern, got {:?}", first.pattern);
    };
    assert_eq!(constructor.value.owner(), &lib_id);
    assert_eq!(constructor.value.as_str(), "Impulsive");
    let [PatternBinding::Bind { local, .. }] = bindings.as_slice() else {
        panic!("expected one field binding, got {bindings:?}");
    };
    let ExprKind::LocalRef(body_ref) = first.body.kind() else {
        panic!("expected local ref body, got {:?}", first.body);
    };
    assert_eq!(local.id, body_ref.value);
}

#[test]
fn collects_canonical_decl_dependencies_from_hir_expr() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub const node C: Dimensionless = 1.0; param p: Dimensionless;");
    let main = desugared_source(
        "import lib as mission; import lib::{p}; node x: Dimensionless = @p + @mission::C;",
    );
    let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
    let scope = GenericScope::new();

    let expr = lower_expr(
        node_value(&main, "x"),
        ExprLoweringContext::new(
            ModuleScope::new(&main_id, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap();
    let deps = collect_expr_dependencies(&expr);

    let frame = crate::ir::instance::frame::InstanceFrame::canonical(
        crate::tir::typed::frame_mint::CanonicalFrameMint::for_test(),
    );
    let graph_refs = deps
        .graph_refs
        .iter()
        .map(|reference| frame.resolve(reference))
        .collect::<Vec<_>>();
    assert!(deps.const_refs.is_empty());
    assert_eq!(graph_refs.len(), 2, "{graph_refs:?}");
    assert!(
        graph_refs
            .iter()
            .all(|reference| reference.owner() == &lib_id)
    );
    assert_eq!(
        graph_refs
            .iter()
            .map(crate::resolved_name::ResolvedName::as_str)
            .collect::<Vec<_>>(),
        ["C", "p"]
    );
}

fn lower_tolerant_node(source: &str, name: &str) -> Expr<Tolerant> {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source(source);
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();
    lower_expr_tolerant(
        node_value(&file, name),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
}

fn lower_strict_node(source: &str, name: &str) -> Result<CheckedExpr, ExprLowerError> {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source(source);
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();
    lower_expr(
        node_value(&file, name),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
}

#[test]
fn strict_lowering_rejects_with_the_first_tolerant_diagnostic_in_source_order() {
    for source in [
        "node out: Dimensionless = mystery(also_missing);",
        "node out: Dimensionless = first_missing + second_missing;",
        "param a: Dimensionless; node out: Dimensionless = -(@a * (@missing + other_missing));",
        "node out: Dimensionless = sin(first_missing) + mystery(second_missing);",
    ] {
        let tolerant = lower_tolerant_node(source, "out");
        let diagnostics = tolerant.diagnostics();
        assert!(diagnostics.len() >= 2, "source: {source}");
        let strict = lower_strict_node(source, "out").unwrap_err();
        assert_eq!(&strict, diagnostics[0], "source: {source}");
    }
}

#[test]
fn resolved_tolerant_tree_has_no_diagnostics_and_refines_to_the_same_shape() {
    let source = "param a: Dimensionless; node out: Dimensionless = -(@a * 2.0);";
    let tolerant = lower_tolerant_node(source, "out");
    assert!(tolerant.diagnostics().is_empty());
    let strict = lower_strict_node(source, "out").unwrap();
    let mut tolerant_spans = Vec::new();
    visit_expr(&tolerant, &mut |node| tolerant_spans.push(node.span));
    let mut strict_spans = Vec::new();
    visit_expr(&strict, &mut |node| strict_spans.push(node.span));
    assert_eq!(tolerant_spans, strict_spans);
}

fn graph_dependency_names(expr: &Expr<Tolerant>) -> BTreeSet<String> {
    collect_expr_dependencies(expr)
        .graph_refs
        .into_iter()
        .map(|name| name.as_str().to_string())
        .collect()
}

#[test]
fn failed_parent_nodes_retain_every_independent_expression_child() {
    let cases = [
        (
            "param a: Dimensionless; param b: Dimensionless; \
             node out: Dimensionless = mystery(@a, @b);",
            2,
            BTreeSet::from(["a".to_string(), "b".to_string()]),
        ),
        (
            "param a: Dimensionless; param b: Dimensionless; \
             node out: Dimensionless = Missing(first: @a, second: @b);",
            2,
            BTreeSet::from(["a".to_string(), "b".to_string()]),
        ),
        (
            "index Axis = { Good }; param a: Dimensionless; param b: Dimensionless; \
             node out: Dimensionless[Axis] = \
                 { Axis#Missing: @a, Axis#Good: @b };",
            2,
            BTreeSet::from(["a".to_string(), "b".to_string()]),
        ),
        (
            "type State { Good } param state: State; \
             param a: Dimensionless; param b: Dimensionless; \
             node out: Dimensionless = match @state { \
                 Missing => @a, Good => @b \
             };",
            3,
            BTreeSet::from(["state".to_string(), "a".to_string(), "b".to_string()]),
        ),
        (
            "param a: Dimensionless; param b: Dimensionless; \
             node out: Dimensionless = @missing(first: @a, second: @b)::result;",
            2,
            BTreeSet::from(["a".to_string(), "b".to_string()]),
        ),
    ];

    for (source, expected_children, expected_dependencies) in cases {
        let expr = lower_tolerant_node(source, "out");
        let diagnostics = expr.diagnostics();
        let ExprKind::Error(failure) = expr.kind() else {
            panic!("expected failed parent node, got {expr:?}");
        };
        let children = failure.children();
        assert_eq!(children.len(), expected_children, "source: {source}");
        assert!(!diagnostics.is_empty(), "source: {source}");
        assert_eq!(
            graph_dependency_names(&expr),
            expected_dependencies,
            "source: {source}"
        );
    }
}

#[test]
fn failed_parent_accumulates_nested_diagnostics_without_duplicates() {
    let expr = lower_tolerant_node("node out: Dimensionless = mystery(also_missing);", "out");
    let diagnostics = expr.diagnostics();

    assert!(matches!(expr.kind(), ExprKind::Error(_)));
    assert_eq!(diagnostics.len(), 2, "diagnostics: {diagnostics:?}");
    assert!(matches!(
        diagnostics[0],
        ExprLowerError::UnknownFunction { .. }
    ));
    assert!(matches!(
        diagnostics[1],
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownName { .. },
            ..
        }
    ));
}

#[test]
fn failed_binder_does_not_fabricate_lexical_locals_for_retained_body() {
    let expr = lower_tolerant_node(
        "param known: Dimensionless; \
         node out: Dimensionless = for item: Missing { item + @known };",
        "out",
    );
    let diagnostics = expr.diagnostics();

    assert!(matches!(expr.kind(), ExprKind::Error(_)));
    assert!(diagnostics.iter().any(|error| matches!(
        error,
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownName { name, .. },
            ..
        } if name.as_str() == "item"
    )));
    assert_eq!(
        graph_dependency_names(&expr),
        BTreeSet::from(["known".to_string()])
    );
}

#[test]
fn const_ref_binding_to_runtime_decl_is_rejected_by_decl_kind() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("param p: Dimensionless; node x: Dimensionless = p;");
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();
    let scoped_name = ScopedName::from(DeclName::expect_valid("p"));
    let bindings = HashMap::from([(
        scoped_name,
        ResolvedDeclName::for_test(owner.clone(), DeclName::expect_valid("p")),
    )]);

    let err = lower_expr(
        node_value(&file, "x"),
        ExprLoweringContext::with_overlay(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
            BindingOverlay::Frozen(FrozenBindings {
                unit_bindings: &HashMap::new(),
                decl_bindings: &bindings,
                instance_templates: &HashMap::new(),
            }),
        ),
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ExprLowerError::BareGraphDeclarationRef {
            kind: DeclSymbolKind::Param,
            ..
        }
    ));
}

#[test]
fn bare_graph_declaration_refs_require_at_sigil() {
    let cases = [
        ("param target: Dimensionless;", DeclSymbolKind::Param),
        ("node target: Dimensionless = 1.0;", DeclSymbolKind::Node),
        (
            "const node target: Dimensionless = 1.0;",
            DeclSymbolKind::Const,
        ),
    ];

    for (declaration, expected_kind) in cases {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source(&format!(
            "{declaration} node output: Dimensionless = target;"
        ));
        let resolver =
            ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
        let scope = GenericScope::new();

        let err = lower_expr(
            node_value(&file, "output"),
            ExprLoweringContext::new(
                ModuleScope::new(&owner, &resolver, &scope),
                &TimeZoneRegistry::bundled(),
            ),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ExprLowerError::BareGraphDeclarationRef { kind, .. } if kind == expected_kind
        ));
    }
}

#[test]
fn graph_ref_boundary_admits_only_readable_declaration_kinds() {
    use super::resolve::GraphRefBoundary;
    use crate::resolve::error::ExpectedDeclKind;

    let graph_values = [
        DeclSymbolKind::Const,
        DeclSymbolKind::Param,
        DeclSymbolKind::Node,
    ];
    let non_values = [
        DeclSymbolKind::Assert,
        DeclSymbolKind::Plot,
        DeclSymbolKind::Figure,
        DeclSymbolKind::Layer,
        DeclSymbolKind::Dag,
    ];
    for boundary in [GraphRefBoundary::Local, GraphRefBoundary::IncludedInstance] {
        for kind in graph_values {
            assert_eq!(boundary.check(kind), Ok(()), "{boundary:?} {kind}");
        }
        for kind in non_values {
            assert_eq!(
                boundary.check(kind),
                Err(ExpectedDeclKind::GraphValue),
                "{boundary:?} {kind}"
            );
        }
    }
    assert_eq!(
        GraphRefBoundary::ImportedDag.check(DeclSymbolKind::Const),
        Ok(())
    );
    for kind in graph_values.into_iter().skip(1).chain(non_values) {
        assert_eq!(
            GraphRefBoundary::ImportedDag.check(kind),
            Err(ExpectedDeclKind::InstanceIndependentConst),
            "{kind}"
        );
    }
}

#[test]
fn graph_ref_boundary_follows_the_alias_role() {
    use super::resolve::GraphRefBoundary;
    use crate::resolve::scope::ModuleAliasRole;

    assert_eq!(
        GraphRefBoundary::through_alias(ModuleAliasRole::ImportedDag),
        GraphRefBoundary::ImportedDag
    );
    assert_eq!(
        GraphRefBoundary::through_alias(ModuleAliasRole::IncludedInstance),
        GraphRefBoundary::IncludedInstance
    );
}

fn include_output_ref(member: &str) -> (ScopedName, ast::Expr) {
    use crate::syntax::module_name::{IncludeInstanceId, ScopeSegment};

    let scope = ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(7));
    let member = DeclName::expect_valid(member);
    let expr = ast::Expr::new(
        ast::ExprKind::GraphRef(ast::GraphRef::IncludeOutput {
            scope: scope.clone(),
            member: member.clone(),
            span: Span::new(3, 4),
        }),
        Span::new(3, 4),
    );
    (ScopedName::in_scope(scope, member), expr)
}

#[test]
fn include_output_ref_resolves_only_through_instance_bindings() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("param p: Dimensionless; assert checked = @p > 0.0;");
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();
    let time_zones = TimeZoneRegistry::bundled();
    let no_units = HashMap::new();
    let no_templates = HashMap::new();
    let module = ModuleScope::new(&owner, &resolver, &scope);
    let context = || ExprLoweringContext::new(module, &time_zones);
    let bound = |bindings| {
        ExprLoweringContext::with_overlay(
            module,
            &time_zones,
            BindingOverlay::Frozen(FrozenBindings {
                unit_bindings: &no_units,
                decl_bindings: bindings,
                instance_templates: &no_templates,
            }),
        )
    };

    // Unbound: an include output has no source path to fall back on.
    let (name, expr) = include_output_ref("p");
    let err = lower_expr(&expr, context()).unwrap_err();
    assert_eq!(
        err,
        ExprLowerError::UnknownGraphRef {
            name: name.clone(),
            span: Span::new(3, 4),
        }
    );

    // Bound: the binding's target is the reference, kind-checked as an
    // included instance's output.
    let target = ResolvedDeclName::for_test(owner.clone(), DeclName::expect_valid("p"));
    let bindings = HashMap::from([(name, target.clone())]);
    let lowered = lower_expr(&expr, bound(&bindings)).unwrap();
    let ExprKind::GraphRef(reference) = lowered.kind() else {
        panic!("expected a graph reference, got {:?}", lowered.kind());
    };
    assert_eq!(
        crate::ir::instance::frame::InstanceFrame::canonical(
            crate::tir::typed::frame_mint::CanonicalFrameMint::for_test(),
        )
        .resolve(&reference.value),
        target
    );

    // A bound output that is not a graph value is rejected.
    let (name, expr) = include_output_ref("checked");
    let assertion = ResolvedDeclName::for_test(owner.clone(), DeclName::expect_valid("checked"));
    let bindings = HashMap::from([(name, assertion)]);
    let err = lower_expr(&expr, bound(&bindings)).unwrap_err();
    assert!(matches!(
        err,
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnexpectedDeclKind {
                expected: crate::resolve::error::ExpectedDeclKind::GraphValue,
                actual: DeclSymbolKind::Assert,
                ..
            },
            ..
        }
    ));
}

#[test]
fn qualified_source_graph_ref_requires_a_module_alias() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("param p: Dimensionless; node out: Dimensionless = @nope::p;");
    let resolver =
        ModuleResolver::without_edges([(owner.clone(), file.declarations.as_slice())]).unwrap();
    let scope = GenericScope::new();

    let err = lower_expr(
        node_value(&file, "out"),
        ExprLoweringContext::new(
            ModuleScope::new(&owner, &resolver, &scope),
            &TimeZoneRegistry::bundled(),
        ),
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownModuleAlias { alias, .. },
            ..
        } if alias.as_str() == "nope"
    ));
}
