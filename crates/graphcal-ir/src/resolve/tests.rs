#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]
use super::*;
use graphcal_syntax::parser::Parser;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(source.to_string()))
}

fn parse_and_resolve(source: &str) -> Result<ResolvedFile, GraphcalError> {
    let file = Parser::new(source).parse_file().unwrap();
    resolve(&file, &make_src(source))
}

#[test]
fn resolve_rocket_ksr() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let file = Parser::new(source).parse_file().unwrap();
    let resolved = resolve(&file, &make_src(source)).unwrap();
    assert_eq!(resolved.consts.len(), 1);
    assert_eq!(resolved.params.len(), 3);
    assert_eq!(resolved.nodes.len(), 3);
}

#[test]
fn resolve_constants_ksr() {
    let source = include_str!("../../../../tests/fixtures/constants.gcl");
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
fn resolve_unknown_const_ref() {
    let err = parse_and_resolve("node x: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownConstRef { .. }));
}

#[test]
fn resolve_at_in_const() {
    let err =
        parse_and_resolve("param p: Dimensionless = 1.0;\nconst BAD: Dimensionless = @p * 2.0;")
            .unwrap_err();
    assert!(matches!(err, GraphcalError::GraphRefInConst { .. }));
}

#[test]
fn parser_rejects_bad_const_casing() {
    let result = Parser::new("const bad_name: Dimensionless = 42.0;").parse_file();
    assert!(result.is_err());
}

#[test]
fn parser_rejects_bad_param_casing() {
    let result = Parser::new("param BAD: Dimensionless = 42.0;").parse_file();
    assert!(result.is_err());
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
    let resolved =
        parse_and_resolve("const A: Dimensionless = 1.0;\nconst B: Dimensionless = A + 2.0;")
            .unwrap();
    let b_deps = &resolved.const_deps["B"];
    assert!(b_deps.contains("A"));
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

// Phase 3: function resolution tests

#[test]
fn resolve_fn_collected() {
    let source = r"
        fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
        param val: Dimensionless = 1.0;
        node result: Dimensionless = double(@val);
    ";
    let resolved = parse_and_resolve(source).unwrap();
    assert_eq!(resolved.functions.len(), 1);
    assert_eq!(resolved.functions[0].name, "double");
}

#[test]
fn resolve_fn_duplicate_name_with_param() {
    let source = r"
        param x: Dimensionless = 1.0;
        fn x(a: Dimensionless) -> Dimensionless = a;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}

#[test]
fn resolve_fn_duplicate_name_with_const() {
    let source = r"
        const X: Dimensionless = 1.0;
        fn X(a: Dimensionless) -> Dimensionless = a;
    ";
    // This should fail at parse time (fn name must be lower_snake_case)
    let result = Parser::new(source).parse_file();
    assert!(result.is_err());
}

#[test]
fn resolve_at_in_fn_body() {
    let source = r"
        param val: Dimensionless = 1.0;
        fn bad(x: Dimensionless) -> Dimensionless = x + @val;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::GraphRefInFn { .. }));
}

#[test]
fn resolve_user_fn_call_in_node() {
    let source = r"
        fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
        param val: Dimensionless = 5.0;
        node result: Dimensionless = double(@val);
    ";
    let resolved = parse_and_resolve(source).unwrap();
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_user_fn_call_in_const() {
    let source = r"
        fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
        const FOUR: Dimensionless = double(2.0);
    ";
    let resolved = parse_and_resolve(source).unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_fn_not_in_source_order() {
    let source = r"
        fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
        param val: Dimensionless = 5.0;
        node result: Dimensionless = double(@val);
    ";
    let resolved = parse_and_resolve(source).unwrap();
    // Functions should NOT appear in source_order
    assert_eq!(resolved.source_order.len(), 2); // param + node only
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
    let err = parse_and_resolve("const A: Dimensionless = 1.0;\nconst A: Dimensionless = 2.0;")
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
    // const uses UPPER, param uses lower — no collision
    let resolved =
        parse_and_resolve("const A: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;").unwrap();
    assert_eq!(resolved.consts.len(), 1);
    assert_eq!(resolved.params.len(), 1);
}

#[test]
fn resolve_unknown_const_ref_in_const() {
    let err = parse_and_resolve("const A: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownConstRef { .. }));
}

#[test]
fn resolve_unknown_function_in_const() {
    let err = parse_and_resolve("const A: Dimensionless = unknown_fn(1.0);").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

#[test]
fn resolve_wrong_arity_in_const() {
    let err = parse_and_resolve("const A: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
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
fn resolve_const_with_block_expr() {
    let resolved =
        parse_and_resolve("const A: Dimensionless = { let x = 1.0; let y = 2.0; x + y };").unwrap();
    assert_eq!(resolved.consts.len(), 1);
    let a_deps = &resolved.const_deps["A"];
    assert!(a_deps.is_empty());
}

#[test]
fn resolve_const_with_if_else() {
    let resolved =
        parse_and_resolve("const A: Dimensionless = if 1.0 > 0.0 { 1.0 } else { 0.0 };").unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_const_with_unary_op() {
    let resolved = parse_and_resolve("const A: Dimensionless = -42.0;").unwrap();
    assert_eq!(resolved.consts.len(), 1);
}

#[test]
fn resolve_node_with_block() {
    let resolved = parse_and_resolve(
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = { let a = @x; a + 1.0 };",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let y_deps = &resolved.runtime_deps["y"];
    assert!(y_deps.contains("x"));
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
        index Color = { Red, Green, Blue }
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
        index Color = { Red, Green, Blue }
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
        index Step = { First, Second, Third }
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
fn resolve_fn_with_block_body() {
    let resolved = parse_and_resolve(
        r"
        fn compute(x: Dimensionless) -> Dimensionless {
            let a = x * 2.0;
            let b = a + 1.0;
            b
        }
        param val: Dimensionless = 5.0;
        node result: Dimensionless = compute(@val);
    ",
    )
    .unwrap();
    assert_eq!(resolved.functions.len(), 1);
    assert_eq!(resolved.nodes.len(), 1);
}

#[test]
fn resolve_duplicate_fn_name() {
    let source = r"
        fn foo(x: Dimensionless) -> Dimensionless = x;
        fn foo(x: Dimensionless) -> Dimensionless = x * 2.0;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DuplicateName { .. }));
}
