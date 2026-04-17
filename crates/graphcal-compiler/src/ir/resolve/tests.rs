#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]
use super::*;
use crate::syntax::parser::Parser;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(source.to_string()))
}

fn parse_and_resolve(source: &str) -> Result<ResolvedFile, GraphcalError> {
    let file = Parser::new(source).parse_file().unwrap();
    resolve(&file, &make_src(source))
}

#[test]
fn resolve_rocket_ksr() {
    let source = include_str!("../../../../../tests/fixtures/rocket.gcl");
    let file = Parser::new(source).parse_file().unwrap();
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert_eq!(resolved.consts.len(), 1);
    assert_eq!(resolved.params.len(), 3);
    assert_eq!(resolved.nodes.len(), 3);
}

#[test]
fn resolve_constants_ksr() {
    let source = include_str!("../../../../../tests/fixtures/constants.gcl");
    let file = Parser::new(source).parse_file().unwrap();
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
fn resolve_unknown_graph_ref() {
    let err = parse_and_resolve("node x: Dimensionless = @nonexistent + 1.0;").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
}

#[test]
fn resolve_unknown_bare_name_becomes_local_ref() {
    // After lifting casing requirements, bare `NONEXISTENT` is parsed as NameRef
    // and resolved to LocalRef (fallback). The resolve pass no longer rejects it;
    // the error is caught later in the TIR dim-check phase as UnknownLocalRef.
    let resolved = parse_and_resolve("node x: Dimensionless = NONEXISTENT + 1.0;").unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_at_in_const() {
    let err = parse_and_resolve(
        "param p: Dimensionless = 1.0;\nconst node bad: Dimensionless = @p * 2.0;",
    )
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
    let resolved =
        parse_and_resolve("param x: Dimensionless = 4.0;\nnode y: Dimensionless = sqrt(@x);")
            .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_unknown_function() {
    let err = parse_and_resolve("node x: Dimensionless = unknown_fn(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity() {
    let err = parse_and_resolve("node x: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_const_deps_extracted() {
    let resolved = parse_and_resolve(
        "const node a: Dimensionless = 1.0;\nconst node b: Dimensionless = @a + 2.0;",
    )
    .unwrap();
    let b_deps = &resolved.const_deps["b"];
    assert!(b_deps.contains("a"));
    assert_eq!(b_deps.len(), 1);
}

#[test]
fn resolve_runtime_deps_extracted() {
    let resolved =
        parse_and_resolve("param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;").unwrap();
    let c_deps = &resolved.runtime_deps["c"];
    assert!(c_deps.contains("a"));
    assert!(c_deps.contains("b"));
    assert_eq!(c_deps.len(), 2);
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
    // After lifting casing requirements, bare `NONEXISTENT` is parsed as NameRef
    // and resolved to LocalRef (fallback). The resolve pass no longer rejects it;
    // the error is caught later in the TIR dim-check phase as UnknownLocalRef.
    let resolved = parse_and_resolve("const node a: Dimensionless = NONEXISTENT + 1.0;").unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_unknown_function_in_const() {
    let err = parse_and_resolve("const node a: Dimensionless = unknown_fn(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity_in_const() {
    let err = parse_and_resolve("const node a: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::WrongArity { .. }));
}

#[test]
fn resolve_unknown_graph_ref_in_node() {
    let err = parse_and_resolve("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @z + 1.0;")
        .unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
}

#[test]
fn resolve_unknown_function_in_node() {
    let err =
        parse_and_resolve("param x: Dimensionless = 1.0;\nnode y: Dimensionless = bad_fn(@x);")
            .unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity_in_node() {
    let err =
        parse_and_resolve("param x: Dimensionless = 1.0;\nnode y: Dimensionless = sqrt(@x, @x);")
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
        type Pair { a: Dimensionless, b: Dimensionless }
        param x: Dimensionless = 1.0;
        node p: Pair = Pair { a: @x, b: @x + 1.0 };
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let p_deps = &resolved.runtime_deps["p"];
    assert!(p_deps.contains("x"));
}

#[test]
fn resolve_node_with_field_access() {
    let resolved = parse_and_resolve(
        r"
        type Pair { a: Dimensionless, b: Dimensionless }
        param x: Dimensionless = 1.0;
        node p: Pair = Pair { a: @x, b: @x + 1.0 };
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
    let source = r#"import "./helper.gcl" { something };"#;
    let file = Parser::new(source).parse_file().unwrap();
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert!(resolved.params.is_empty());
    assert!(resolved.nodes.is_empty());
    assert!(resolved.consts.is_empty());
}

#[test]
fn resolve_indexed_param() {
    let resolved = parse_and_resolve(
        r"
        index Color = { Red, Green, Blue };
        param values: Dimensionless[Color] = {
            Color::Red: 1.0,
            Color::Green: 2.0,
            Color::Blue: 3.0,
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
        index Color = { Red, Green, Blue };
        param values: Dimensionless[Color] = {
            Color::Red: 1.0,
            Color::Green: 2.0,
            Color::Blue: 3.0,
        };
        node doubled: Dimensionless[Color] = for c: Color { @values[c] * 2.0 };
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let deps = &resolved.runtime_deps["doubled"];
    assert!(deps.contains("values"));
}

#[test]
fn resolve_scan_expression() {
    let resolved = parse_and_resolve(
        r"
        index Step = { First, Second, Third };
        param vals: Dimensionless[Step] = {
            Step::First: 1.0,
            Step::Second: 2.0,
            Step::Third: 3.0,
        };
        node cumul: Dimensionless[Step] = scan(@vals, 0.0, |acc, val| acc + val);
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let deps = &resolved.runtime_deps["cumul"];
    assert!(deps.contains("vals"));
}

#[test]
fn resolve_unfold_self_edge_excluded() {
    // The unfold body references @x[prev_t], which creates a self-reference.
    // extract_all_refs should exclude this self-edge from runtime_deps.
    let source = r"
        index TimeStep = { First, Second, Third };
        node x: Dimensionless[TimeStep] = unfold(1.0, |prev_t, t| @x[prev_t] * 2.0);
    ";
    let resolved = parse_and_resolve(source).unwrap();
    let x_deps = &resolved.runtime_deps["x"];
    assert!(
        !x_deps.contains("x"),
        "unfold self-reference should be excluded from runtime_deps"
    );
}

// --- Visibility tests ---

#[test]
fn resolve_required_param_must_be_pub() {
    let source = r"
        param x: Dimensionless;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::RequiredItemMustBePub { kind, .. } if kind == "param"));
}

#[test]
fn resolve_required_index_must_be_pub() {
    let source = r"
        index Phase;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::RequiredItemMustBePub { kind, .. } if kind == "index"));
}

#[test]
fn resolve_pub_required_param_ok() {
    let source = r"
        pub param x: Dimensionless;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_pub_required_index_ok() {
    let source = r"
        pub index Phase;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_dim() {
    let source = r"
        dim Velocity = Length / Time;
        pub param speed: Velocity = 10.0 m/s;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref_name, .. } if ref_name == "Velocity")
    );
}

#[test]
fn resolve_private_in_public_ok_when_dim_is_pub() {
    let source = r"
        pub dim Velocity = Length / Time;
        pub param speed: Velocity = 10.0 m/s;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_ok_for_builtin_dims() {
    // Built-in dimensions (Length, Time, etc.) don't need to be `pub`.
    let source = r"
        pub param distance: Length = 1.0 m;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_private_in_public_index_in_type() {
    let source = r"
        pub index Phase = { Alpha, Beta };
        index Step = { Xray, Yankee };
        pub node costs: Dimensionless[Phase, Step] = { Phase::Alpha: { Step::Xray: 1.0, Step::Yankee: 2.0 }, Phase::Beta: { Step::Xray: 3.0, Step::Yankee: 4.0 } };
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
fn resolve_pub_names_collected() {
    let source = r"
        pub dim Velocity = Length / Time;
        pub dim Acceleration = Length / Time^2;
        pub const node g0: Acceleration = 9.80665 m/s^2;
        node speed: Velocity = 10.0 m/s;
    ";
    let resolved = parse_and_resolve(source).unwrap();
    assert!(resolved.pub_names.contains("Velocity"));
    assert!(resolved.pub_names.contains("Acceleration"));
    assert!(resolved.pub_names.contains("g0"));
    assert!(!resolved.pub_names.contains("speed"));
}

#[test]
fn resolve_non_pub_private_param_ok() {
    // Non-pub params can reference private dims freely.
    let source = r"
        dim Velocity = Length / Time;
        param speed: Velocity = 10.0 m/s;
    ";
    parse_and_resolve(source).unwrap();
}
