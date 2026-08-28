use super::*;
use crate::builtin::BuiltinConst;
use crate::registry::time_scale::TimeScale;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::parser::Parser;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(source.to_string()))
}

fn parse_and_desugar(source: &str) -> crate::desugar::desugared_ast::File {
    let raw_file = Parser::new(source).parse_file().unwrap();
    crate::syntax::desugar::desugar_multi_decls_in_file(raw_file)
}

fn parse_and_resolve(source: &str) -> Result<CollectedFile, GraphcalError> {
    let file = parse_and_desugar(source);
    resolve(&file, &make_src(source))
}

/// Run the full per-file pipeline (desugar → IR → HIR/TIR) so tests can
/// observe reference resolution and the HIR-derived dependency graph.
fn compile_to_tir(source: &str) -> Result<crate::tir::typed::TIR, GraphcalError> {
    let file = parse_and_desugar(source);
    let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
    let ir = crate::ir::lower::lower(&file, &src)?;
    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    resolver
        .add_module(ir.dag_id().clone(), &file.declarations)
        .unwrap();
    let mut project_types = crate::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_local_hir(&ir).unwrap();
    crate::tir::typed::type_resolve_with_modules(ir, &src, &resolver, &project_types)
}

/// Dependency names of `decl` in `map`, as leaf strings.
fn dep_names_of<'a>(
    map: &'a std::collections::HashMap<
        ResolvedDeclName,
        std::collections::BTreeSet<ResolvedDeclName>,
    >,
    decl: &str,
) -> Vec<&'a str> {
    match map.iter().find(|(key, _)| key.as_str() == decl) {
        Some((_, dependencies)) => dependencies
            .iter()
            .map(crate::syntax::names::ResolvedName::as_str)
            .collect(),
        None => Vec::new(),
    }
}

#[test]
fn source_level_min_i32_dimension_exponent_formats_exactly() {
    let tir = compile_to_tir(
        "pub base dim X;\n\
         pub base dim Y;\n\
         pub dim Huge = X^-1073741824;\n\
         pub dim Mixed = Y * Huge^2;\n",
    )
    .unwrap();
    let mixed = tir.registry.dimensions.get_dimension("Mixed").unwrap();

    assert_eq!(
        mixed
            .try_format_with(tir.registry.dimensions.base_dim_names())
            .unwrap(),
        "Y / X^2147483648"
    );
}

#[test]
fn resolve_rocket_ksr() {
    let source = include_str!("../../../../../tests/fixtures/valid/rocket.gcl");
    let file = parse_and_desugar(source);
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert_eq!(resolved.consts.len(), 1);
    assert_eq!(resolved.params.len(), 3);
    assert_eq!(resolved.nodes.len(), 3);
}

#[test]
fn resolve_constants_ksr() {
    let source = include_str!("../../../../../tests/fixtures/valid/constants.gcl");
    let file = parse_and_desugar(source);
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert_eq!(resolved.consts.len(), 4);
    assert_eq!(resolved.params.len(), 1);
    assert_eq!(resolved.nodes.len(), 2);
}

#[test]
fn resolve_duplicate_name() {
    let err = parse_and_resolve("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
        .unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}

#[test]
fn resolve_rejects_type_index_name_collision() {
    let err =
        parse_and_resolve("type M { Mk(v: Dimensionless) }\npub index M = { A, B };").unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DuplicateName { ref name, .. } if name == "M"
    ));
}

#[test]
fn resolve_rejects_dimension_index_name_collision() {
    let err = parse_and_resolve("dim M = Length;\npub index M = { A, B };").unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DuplicateName { ref name, .. } if name == "M"
    ));
}

#[test]
fn resolve_rejects_dimension_type_name_collision() {
    let err = parse_and_resolve("dim M = Length;\ntype M { Mk(v: Dimensionless) }").unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DuplicateName { ref name, .. } if name == "M"
    ));
}

#[test]
fn term_value_and_static_index_may_share_a_name() {
    parse_and_resolve("param M: Dimensionless = 1.0;\npub index M = { A, B };").unwrap();
}

#[test]
fn static_index_and_prelude_unit_resolve_independently() {
    compile_to_tir(
        "pub index s = { A };
         param sample: Time[s] = { s#A: 1.0 s };",
    )
    .unwrap();
}

#[test]
fn resolve_rejects_builtin_dimension_shadowing() {
    let err = parse_and_resolve("dim Velocity = Length / Time;").unwrap_err();
    assert!(matches!(err, GraphcalError::BuiltinNameShadowed { name, .. } if name == "Velocity"));
}

#[test]
fn resolve_rejects_builtin_unit_shadowing() {
    let err = parse_and_resolve("unit m: Length = 1.0 m;").unwrap_err();
    assert!(matches!(err, GraphcalError::BuiltinNameShadowed { name, .. } if name == "m"));
}

#[test]
fn resolve_rejects_every_builtin_constant_spelling_for_graph_values() {
    for builtin in BuiltinConst::ALL {
        for declaration in [
            format!("param {builtin}: Dimensionless = 2.0;"),
            format!("node {builtin}: Dimensionless = 2.0;"),
            format!("const node {builtin}: Dimensionless = 2.0;"),
        ] {
            let err = parse_and_resolve(&declaration).unwrap_err();
            assert!(matches!(
                err,
                GraphcalError::BuiltinNameShadowed { name, .. } if name == builtin.as_str()
            ));
        }
    }
}

#[test]
fn resolve_allows_every_time_scale_spelling_for_graph_values() {
    for scale in TimeScale::ALL {
        for declaration in [
            format!("param {scale}: Dimensionless = 2.0;"),
            format!("node {scale}: Dimensionless = 2.0;"),
            format!("const node {scale}: Dimensionless = 2.0;"),
        ] {
            parse_and_resolve(&declaration).unwrap();
        }
    }
}

#[test]
fn resolve_unknown_graph_ref() {
    // Unknown `@` targets are rejected by HIR lowering during type
    // resolution — the IR collection pass no longer classifies references.
    let err = compile_to_tir("node x: Dimensionless = @nonexistent + 1.0;").unwrap_err();
    assert!(
        err.to_string().contains("nonexistent"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_unknown_bare_name_is_rejected_in_hir_lowering() {
    // A bare name that matches nothing in scope is rejected by HIR lowering
    // with an unknown-name diagnostic; there is no fallback classification.
    let err = compile_to_tir("node x: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
    assert!(
        err.to_string().contains("NONEXISTENT"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_at_in_const() {
    let err =
        compile_to_tir("param p: Dimensionless = 1.0;\nconst node bad: Dimensionless = @p * 2.0;")
            .unwrap_err();
    assert!(matches!(err, GraphcalError::GraphRefInConst { .. }));
}

#[test]
fn parser_accepts_any_const_casing() {
    let file = Parser::new("const node BAD_NAME: Dimensionless = 42.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
}

#[test]
fn parser_accepts_any_param_casing() {
    let file = Parser::new("param BAD: Dimensionless = 42.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
}

#[test]
fn resolve_builtin_const_recognized() {
    let resolved = parse_and_resolve("node x: Dimensionless = PI * 2.0;").unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_builtin_function_recognized() {
    let resolved = parse_and_resolve(
        "param x: Dimensionless = 4.0;\n\
         node root: Dimensionless = sqrt(@x);\n\
         node lower: Dimensionless = least(@x, 5.0);\n\
         node upper: Dimensionless = greatest(@x, 3.0);",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 3);
}

#[test]
fn resolve_unknown_function() {
    let err = compile_to_tir("node x: Dimensionless = unknown_fn(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn named_arguments_on_builtin_report_positional_call_syntax() {
    let err = compile_to_tir("node angle: Angle = atan2(y: 1.0 m, x: 2.0 m);").unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::NamedArgumentsOnFunction {
            ref name,
            ref positional_call,
            ..
        } if name == "atan2" && positional_call == "atan2(y_value, x_value)"
    ));
}

#[test]
fn named_arguments_on_extern_report_positional_call_syntax() {
    let err = compile_to_tir(
        r#"
import plugin "graphcal:demo" as demo {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}
node x: Dimensionless = demo::lerp(a: 1.0, b: 2.0, t: 0.5);
"#,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::NamedArgumentsOnFunction {
            ref name,
            ref positional_call,
            ..
        } if name == "demo::lerp"
            && positional_call == "demo::lerp(a_value, b_value, t_value)"
    ));
}

#[test]
fn unknown_named_call_reports_one_unknown_term_callee() {
    let err = compile_to_tir("node x: Dimensionless = Missing(value: 1.0);").unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::UnknownFunction { ref name, .. } if name == "Missing"
    ));
}

#[test]
fn obsolete_extremum_function_names_are_rejected() {
    for obsolete in ["min", "max"] {
        let source = format!("node x: Dimensionless = {obsolete}(1.0, 2.0);");
        match compile_to_tir(&source).unwrap_err() {
            GraphcalError::UnknownFunction { name, .. } => assert_eq!(name, obsolete),
            other => panic!("obsolete function call should be unknown: {other:?}"),
        }
    }
}

#[test]
fn binary_selection_functions_have_fixed_arity() {
    let err = compile_to_tir("node x: Dimensionless = least(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_wrong_arity() {
    let err = compile_to_tir("node x: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_const_deps_extracted() {
    let tir = compile_to_tir(
        "const node a: Dimensionless = 1.0;\nconst node b: Dimensionless = @a + 2.0;",
    )
    .unwrap();
    let deps = &tir.root().semantic.dependencies;
    assert_eq!(dep_names_of(&deps.const_deps, "b"), ["a"]);
}

#[test]
fn resolve_runtime_deps_extracted() {
    let tir =
        compile_to_tir("param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;").unwrap();
    let deps = &tir.root().semantic.dependencies;
    assert_eq!(dep_names_of(&deps.runtime_deps, "c"), ["a", "b"]);
}

// --- Additional error path tests ---

#[test]
fn resolve_duplicate_param_name() {
    let err = parse_and_resolve("param x: Dimensionless = 1.0;\nparam x: Dimensionless = 2.0;")
        .unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}

#[test]
fn resolve_duplicate_const_name() {
    let err =
        parse_and_resolve("const node a: Dimensionless = 1.0;\nconst node a: Dimensionless = 2.0;")
            .unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}

#[test]
fn resolve_duplicate_node_name() {
    let err = parse_and_resolve(
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;\nnode y: Dimensionless = @x + 1.0;",
    )
    .unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}

#[test]
fn resolve_constructor_collision_with_node() {
    let err = parse_and_resolve(
        "type Student { Student(mass: Dimensionless), }\nnode Student: Dimensionless = 1.0;",
    )
    .unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DuplicateName { ref name, .. } if name == "Student"
    ));
}

#[test]
fn resolve_const_collision_with_param() {
    // const and param both use lower_snake_case — different names → no collision
    let resolved =
        parse_and_resolve("const node a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;")
            .unwrap();
    assert_eq!(resolved.consts.len(), 1);
    assert_eq!(resolved.params.len(), 1);
}

#[test]
fn resolve_unknown_bare_name_in_const_becomes_local_ref() {
    // After lifting casing requirements, bare `NONEXISTENT` is parsed as an unresolved path
    // and resolved to LocalRef (fallback). The resolve pass no longer rejects it;
    // the error is caught later in the TIR dim-check phase as UnknownLocalRef.
    let resolved = parse_and_resolve("const node a: Dimensionless = NONEXISTENT + 1.0;").unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_unknown_function_in_const() {
    let err = compile_to_tir("const node a: Dimensionless = unknown_fn(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity_in_const() {
    let err = compile_to_tir("const node a: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_unknown_graph_ref_in_node() {
    let err = compile_to_tir("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @z + 1.0;")
        .unwrap_err();
    assert!(err.to_string().contains('z'), "unexpected error: {err}");
}

#[test]
fn resolve_unknown_function_in_node() {
    let err = compile_to_tir("param x: Dimensionless = 1.0;\nnode y: Dimensionless = bad_fn(@x);")
        .unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity_in_node() {
    let err =
        compile_to_tir("param x: Dimensionless = 1.0;\nnode y: Dimensionless = sqrt(@x, @x);")
            .unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_const_with_if_else() {
    let resolved =
        parse_and_resolve("const node a: Dimensionless = if 1.0 > 0.0 { 1.0 } else { 0.0 };")
            .unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_const_with_unary_op() {
    let resolved = parse_and_resolve("const node a: Dimensionless = -42.0;").unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_node_with_struct() {
    let resolved = parse_and_resolve(
        r"
        type Pair { Pair(a: Dimensionless, b: Dimensionless) }
        param x: Dimensionless = 1.0;
        node p: Pair = Pair(a: @x, b: @x + 1.0);
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_node_with_field_access() {
    let resolved = parse_and_resolve(
        r"
        type Pair { Pair(a: Dimensionless, b: Dimensionless) }
        param x: Dimensionless = 1.0;
        node p: Pair = Pair(a: @x, b: @x + 1.0);
        node val: Dimensionless = @p.a;
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 2);
}

#[test]
fn resolve_node_with_convert() {
    let resolved =
        parse_and_resolve("param x: Length = 1000.0 m;\nnode y: Length = @x -> km;").unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_import_decl_skipped() {
    // import declarations should not be treated as param/node/const
    let source = "import helper::{something};";
    let file = parse_and_desugar(source);
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert!(resolved.params.is_empty());
    assert!(resolved.nodes.is_empty());
    assert!(resolved.consts.is_empty());
}

#[test]
fn resolve_indexed_param() {
    let resolved = parse_and_resolve(
        r"
        pub index Color = { Red, Green, Blue };
        param values: Dimensionless[Color] = {
            Color#Red: 1.0,
            Color#Green: 2.0,
            Color#Blue: 3.0,
        };
    ",
    )
    .unwrap();
    assert_eq!(resolved.params.len(), 1);
}

#[test]
fn resolve_for_comprehension() {
    let resolved = parse_and_resolve(
        r"
        pub index Color = { Red, Green, Blue };
        param values: Dimensionless[Color] = {
            Color#Red: 1.0,
            Color#Green: 2.0,
            Color#Blue: 3.0,
        };
        node doubled: Dimensionless[Color] = for c: Color { @values[c] * 2.0 };
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_scan_expression() {
    let resolved = parse_and_resolve(
        r"
        pub index Step = { First, Second, Third };
        param vals: Dimensionless[Step] = {
            Step#First: 1.0,
            Step#Second: 2.0,
            Step#Third: 3.0,
        };
        node cumul: Dimensionless[Step] = scan(@vals, 0.0, |acc, val| acc + val);
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_unfold_self_edge_is_an_ordinary_dependency() {
    // The previous state is a lexical binding. An explicit @x reference in
    // the body remains an ordinary self-edge even when indexed by prev_t.
    let source = r"
        index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
        node x: Dimensionless[TimeStep] = unfold(
            TimeStep,
            1.0,
            |prev_x, prev_t, t| @x[prev_t] * 2.0
        );
    ";
    let tir = compile_to_tir(source).unwrap();
    let deps = &tir.root().semantic.dependencies;
    assert!(
        dep_names_of(&deps.runtime_deps, "x").contains(&"x"),
        "explicit unfold self-reference must remain in runtime_deps"
    );
}

#[test]
fn resolve_unfold_init_self_edge_retained() {
    let source = r"
        index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
        node x: Dimensionless[TimeStep] = unfold(
            TimeStep,
            sum(@x),
            |prev_x, prev_t, t| @x[prev_t] * 2.0
        );
    ";
    let tir = compile_to_tir(source).unwrap();
    let deps = &tir.root().semantic.dependencies;
    assert!(
        dep_names_of(&deps.runtime_deps, "x").contains(&"x"),
        "unfold init self-reference must remain a runtime dependency"
    );
}

// --- Visibility tests ---

#[test]
fn resolve_required_param_is_an_annotation_free_input_port() {
    // `param` never carries `pub`; the declaration kind directly creates a
    // named input port, and omitting the default makes that port required.
    let source = r"
        param x: Dimensionless;
    ";
    parse_and_resolve(source).unwrap();
}

// `pub param` / `pub(bind) param` are rejected at parse time; see
// `syntax::parser::decl::tests` for parser-level coverage.

#[test]
fn resolve_required_index_must_be_bindable() {
    let source = r"
        index Phase;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::RequiredItemMustBeBindable { kind, .. } if kind == "index")
    );
}

#[test]
fn resolve_required_pub_index_still_needs_bind() {
    // `pub index Phase;` is now rejected: required indexes must be
    // `pub(bind)` because A4 forces bindability.
    let source = r"
        pub index Phase;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::RequiredItemMustBeBindable { kind, .. } if kind == "index")
    );
}

#[test]
fn resolve_pub_bind_required_index_ok() {
    let source = r"
        pub(bind) index Phase;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_required_type_must_be_bindable() {
    let source = r"
        type Element;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::RequiredItemMustBeBindable { kind, .. } if kind == "type")
    );
}

#[test]
fn resolve_pub_bind_required_type_ok() {
    let source = r"
        pub(bind) type Element;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_required_dim_must_be_bindable() {
    let source = r"
        dim D;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::RequiredItemMustBeBindable { kind, .. } if kind == "dim"));
}

#[test]
fn resolve_pub_bind_required_dim_ok() {
    let source = r"
        pub(bind) dim D;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_dim() {
    // V003 is triggered by a pub node (not pub param, which is rejected).
    let source = r"
        dim Speed = Length / Time;
        param kmh: Speed = 36.0 km/h;
        pub node speed: Speed = @kmh;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PrivateInPublic { ref_name, .. } if ref_name == "Speed"));
}

#[test]
fn resolve_private_in_public_ok_when_dim_is_pub() {
    let source = r"
        pub dim Speed = Length / Time;
        param kmh: Speed = 36.0 km/h;
        pub node speed: Speed = @kmh;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_ok_for_builtin_dims() {
    // Built-in dimensions (Length, Time, etc.) don't need to be `pub`.
    let source = r"
        param origin: Length = 1.0 m;
        pub node distance: Length = @origin;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_index_in_type() {
    let source = r"
        pub index Phase = { Alpha, Beta };
        index Step = { Xray, Yankee };
        pub node costs: Dimensionless[Phase, Step] = { Phase#Alpha: { Step#Xray: 1.0, Step#Yankee: 2.0 }, Phase#Beta: { Step#Xray: 3.0, Step#Yankee: 4.0 } };
    ";
    let err = parse_and_resolve(source).unwrap_err();
    // May get PubIndexVariantLiteral before PrivateInPublic.
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref ref_name, .. } if ref_name == "Step")
            || matches!(err, GraphcalError::PubIndexVariantLiteral { .. }),
        "expected PrivateInPublic or PubIndexVariantLiteral error, got: {err:?}"
    );
}

#[test]
fn resolve_external_surface_keeps_explicit_exports_and_input_ports_distinct() {
    let source = r"
        pub dim Speed = Length / Time;
        pub dim GravityAccel = Length / Time^2;
        pub const node g0: GravityAccel = 9.80665 m/s^2;
        param input_speed: Speed = 10.0 m/s;
        node speed: Speed = @input_speed;
    ";
    let resolved = parse_and_resolve(source).unwrap();
    for name in ["Speed", "GravityAccel"] {
        assert!(
            resolved
                .external_surface
                .is_static_explicit_export(&crate::syntax::names::NameAtom::parse(name).unwrap())
        );
    }
    assert!(
        resolved
            .external_surface
            .is_explicit_export(&DeclName::expect_valid("g0"))
    );
    let input = DeclName::expect_valid("input_speed");
    assert!(resolved.external_surface.is_input_port(&input));
    assert!(resolved.external_surface.is_externally_nameable(&input));
    assert!(resolved.external_surface.can_select_output(&input));
    assert!(!resolved.external_surface.is_explicit_export(&input));
    assert!(
        !resolved
            .external_surface
            .is_externally_nameable(&DeclName::expect_valid("speed"))
    );
}

#[test]
fn resolve_param_default_with_pub_bind_variant_literal_ok() {
    // A10(a): `param` is an input port, so a variant literal of a
    // `pub(bind)` index in a param default is allowed — V005 at the
    // include site will ensure the importer re-binds the param when it
    // rebinds the index.
    let source = r"
        pub(bind) index Phase = { Design, Build, Test };
        param cost: Dimensionless[Phase] = {
            Phase#Design: 100.0,
            Phase#Build: 200.0,
            Phase#Test: 50.0,
        };
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_node_with_pub_bind_variant_literal_fires_v004() {
    // A10(c): `node` is non-bindable, so a variant literal of a
    // `pub(bind)` index in a node body would orphan under rebinding.
    let source = r"
        pub(bind) index Phase = { Design, Build, Test };
        param cost: Dimensionless[Phase] = {
            Phase#Design: 1.0,
            Phase#Build: 2.0,
            Phase#Test: 3.0,
        };
        node design_cost: Dimensionless = @cost[Phase#Design];
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_const_with_pub_bind_variant_literal_fires_v004() {
    let source = r"
        pub(bind) index Phase = { Design, Build };
        pub const node costs: Dimensionless[Phase] = {
            Phase#Design: 1.0,
            Phase#Build: 2.0,
        };
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_private_assert_with_pub_bind_variant_literal_ok() {
    // A10(b) carve-out: private sink kinds are pruned from the merged
    // IR when the file is used as a library, so literal mentions of
    // `Phase#v` cannot orphan anything under override.
    let source = r"
        pub(bind) index Phase = { Design, Build };
        param cost: Dimensionless[Phase] = {
            Phase#Design: 1.0,
            Phase#Build: 2.0,
        };
        assert design_cheap = @cost[Phase#Design] < 10.0;
    ";
    compile_to_tir(source).unwrap();
}

#[test]
fn resolve_public_assert_with_pub_bind_variant_literal_fires_v004() {
    // A10(b): public sinks travel with the include and must abstract
    // over pub(bind) indexes.
    let source = r"
        pub(bind) index Phase = { Design, Build };
        param cost: Dimensionless[Phase] = {
            Phase#Design: 1.0,
            Phase#Build: 2.0,
        };
        pub assert design_cheap = @cost[Phase#Design] < 10.0;
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_node_with_plain_pub_variant_literal_ok() {
    // Plain `pub` (fixed) indexes are not bindable, so A10 does not
    // fire on their variant literals; importers cannot override them.
    let source = r"
        pub index Phase = { Design, Build };
        pub const node costs: Dimensionless[Phase] = {
            Phase#Design: 1.0,
            Phase#Build: 2.0,
        };
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_param_with_private_dim_fires_v003() {
    // A param signature is externally nameable as an input port, so a private
    // dimension in that signature is V003 (A9 case 1).
    let source = r"
        dim Speed = Length / Time;
        param speed: Speed = 10.0 m/s;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PrivateInPublic { ref_name, .. } if ref_name == "Speed"));
}

#[test]
fn resolve_param_with_pub_dim_ok() {
    let source = r"
        pub dim Speed = Length / Time;
        param speed: Speed = 10.0 m/s;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_pub_dim_with_private_dim_fires_v003() {
    // A9 case 1 also applies to dim/unit/type/index signatures.
    let source = r"
        dim Inner = Length;
        pub dim Outer = Inner / Time;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Dimension && ref_name == "Inner")
    );
}

#[test]
fn resolve_pub_type_with_private_field_type_fires_v003() {
    let source = r"
        type Inner { Inner }
        pub type Outer { Outer(inner: Inner) }
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Type && ref_name == "Inner")
    );
}

#[test]
fn resolve_pub_union_type_with_private_payload_type_fires_v003() {
    // Under the constructor-list union design, variants no longer
    // reference other types by name in the union signature. The A9
    // dependency from a `pub` union to a private type now flows through
    // a variant's payload field type. (See issue #601.)
    let source = r"
        type Inner { Inner }
        pub type Result {
          Ok,
          Err(detail: Inner),
        }
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Type && ref_name == "Inner")
    );
}

#[test]
fn resolve_pub_type_with_private_type_default_fires_v003() {
    let source = r"
        type Secret { Secret }
        pub type Wrapper<T: Type = Secret> { Wrapper(value: T) }
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Type
                && ref_kind == DeclarationKind::Type
                && ref_name == "Secret")
    );
}

#[test]
fn resolve_pub_type_with_private_dimension_default_fires_v003() {
    let source = r"
        dim SecretDim = Length;
        pub type Wrapper<D: Dim = SecretDim> { Wrapper(value: D) }
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref_kind, ref_name, .. }
            if ref_kind == DeclarationKind::Dimension && ref_name == "SecretDim")
    );
}

#[test]
fn resolve_pub_type_with_private_index_default_fires_v003() {
    let source = r"
        index SecretIndex = { One };
        pub type Wrapper<I: Index = SecretIndex> { Wrapper(value: Key<I>) }
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref_kind, ref_name, .. }
            if ref_kind == DeclarationKind::Index && ref_name == "SecretIndex")
    );
}

#[test]
fn resolve_pub_type_checks_nested_generic_default_dependencies() {
    let source = r"
        type Secret { Secret }
        pub type PublicBox<T: Type> { PublicBox(value: T) }
        pub type Wrapper<T: Type = PublicBox<Secret>> { Wrapper(value: T) }
    ";
    let err = compile_to_tir(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref_name, .. }
            if ref_name == "Secret")
    );
}

#[test]
fn generic_parameter_cannot_shadow_private_static_type() {
    let source = r"
        type Shadow { Shadow }
        pub type Wrapper<Shadow: Type, T: Type = Shadow> { Wrapper(value: T) }
    ";
    assert!(matches!(
        compile_to_tir(source),
        Err(GraphcalError::EvalError { ref message, .. })
            if message.contains("shadows a visible Static name")
    ));
}

#[test]
fn public_generic_defaults_are_accepted() {
    let source = r"
        pub dim PublicDim = Length;
        pub index PublicIndex = { One };
        pub type PublicType { PublicType }
        pub type Wrapper<
            D: Dim = PublicDim,
            I: Index = PublicIndex,
            T: Type = PublicType,
        > { Wrapper(value: T, key: Key<I>, quantity: D) }
    ";
    compile_to_tir(source).unwrap();
}

#[test]
fn resolve_pub_bind_index_with_private_dim_fires_v003() {
    // A required coordinate index carries a dimension constraint that participates
    // in A9 case 1.
    let source = r"
        dim Rate = Time^-1;
        pub(bind) index Channel: Rate;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Index && ref_name == "Rate")
    );
}

#[test]
fn resolve_pub_unit_with_private_dim_fires_v003() {
    let source = r"
        dim Currency = Length;
        pub const unit usd: Currency = 1.0 m;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == DeclarationKind::Unit && ref_name == "Currency")
    );
}
