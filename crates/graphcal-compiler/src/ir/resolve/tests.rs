use super::*;
use crate::syntax::parser::Parser;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(source.to_string()))
}

fn parse_and_desugar(source: &str) -> crate::desugar::resolved_ast::File {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let mut desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    crate::syntax::ast::desugar_tuple_matches(&mut desugared);
    crate::syntax::name_resolve::resolve_name_refs(desugared)
}

fn parse_and_resolve(source: &str) -> Result<ResolvedFile, GraphcalError> {
    let file = parse_and_desugar(source);
    resolve(&file, &make_src(source))
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
fn resolve_unknown_graph_ref() {
    let err = parse_and_resolve("node x: Dimensionless = @nonexistent + 1.0;").unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
}

#[test]
fn resolve_unknown_bare_name_becomes_local_ref() {
    // After lifting casing requirements, bare `NONEXISTENT` is parsed as an unresolved path
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
    let b_deps = &resolved.const_deps[&ScopedName::local("b")];
    assert!(b_deps.contains(&ScopedName::local("a")));
    assert_eq!(b_deps.len(), 1);
}

#[test]
fn resolve_runtime_deps_extracted() {
    let resolved =
        parse_and_resolve("param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;").unwrap();
    let c_deps = &resolved.runtime_deps[&ScopedName::local("c")];
    assert!(c_deps.contains(&ScopedName::local("a")));
    assert!(c_deps.contains(&ScopedName::local("b")));
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
        type Pair { Pair(a: Dimensionless, b: Dimensionless) }
        param x: Dimensionless = 1.0;
        node p: Pair = Pair(a: @x, b: @x + 1.0);
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let p_deps = &resolved.runtime_deps[&ScopedName::local("p")];
    assert!(p_deps.contains(&ScopedName::local("x")));
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
    let source = "import helper.{something};";
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
            Color.Red: 1.0,
            Color.Green: 2.0,
            Color.Blue: 3.0,
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
            Color.Red: 1.0,
            Color.Green: 2.0,
            Color.Blue: 3.0,
        };
        node doubled: Dimensionless[Color] = for c: Color { @values[c] * 2.0 };
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let deps = &resolved.runtime_deps[&ScopedName::local("doubled")];
    assert!(deps.contains(&ScopedName::local("values")));
}

#[test]
fn resolve_scan_expression() {
    let resolved = parse_and_resolve(
        r"
        pub index Step = { First, Second, Third };
        param vals: Dimensionless[Step] = {
            Step.First: 1.0,
            Step.Second: 2.0,
            Step.Third: 3.0,
        };
        node cumul: Dimensionless[Step] = scan(@vals, 0.0, |acc, val| acc + val);
    ",
    )
    .unwrap();
    assert_eq!(resolved.nodes.len(), 1);
    let deps = &resolved.runtime_deps[&ScopedName::local("cumul")];
    assert!(deps.contains(&ScopedName::local("vals")));
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
    let x_deps = &resolved.runtime_deps[&ScopedName::local("x")];
    assert!(
        !x_deps.contains(&ScopedName::local("x")),
        "unfold self-reference should be excluded from runtime_deps"
    );
}

// --- Visibility tests ---

#[test]
fn resolve_required_param_is_implicitly_bindable() {
    // Post-A5: `param` never carries `pub`; a bare required param is
    // implicitly visible + bindable at the include site.
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
        dim Velocity = Length / Time;
        param kmh: Velocity = 36.0 km/h;
        pub node speed: Velocity = @kmh;
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
        param kmh: Velocity = 36.0 km/h;
        pub node speed: Velocity = @kmh;
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
        pub node costs: Dimensionless[Phase, Step] = { Phase.Alpha: { Step.Xray: 1.0, Step.Yankee: 2.0 }, Phase.Beta: { Step.Xray: 3.0, Step.Yankee: 4.0 } };
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
fn resolve_param_default_with_pub_bind_variant_literal_ok() {
    // A10(a): `param` is implicitly bindable, so a variant literal of a
    // `pub(bind)` index in a param default is allowed — V005 at the
    // include site will ensure the importer re-binds the param when it
    // rebinds the index.
    let source = r"
        pub(bind) index Phase = { Design, Build, Test };
        param cost: Dimensionless[Phase] = {
            Phase.Design: 100.0,
            Phase.Build: 200.0,
            Phase.Test: 50.0,
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
            Phase.Design: 1.0,
            Phase.Build: 2.0,
            Phase.Test: 3.0,
        };
        node design_cost: Dimensionless = @cost[Phase.Design];
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_const_with_pub_bind_variant_literal_fires_v004() {
    let source = r"
        pub(bind) index Phase = { Design, Build };
        pub const node costs: Dimensionless[Phase] = {
            Phase.Design: 1.0,
            Phase.Build: 2.0,
        };
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_private_assert_with_pub_bind_variant_literal_ok() {
    // A10(b) carve-out: private sink kinds are pruned from the merged
    // IR when the file is used as a library, so literal mentions of
    // `Phase.v` cannot orphan anything under override.
    let source = r"
        pub(bind) index Phase = { Design, Build };
        param cost: Dimensionless[Phase] = {
            Phase.Design: 1.0,
            Phase.Build: 2.0,
        };
        assert design_cheap = @cost[Phase.Design] < 10.0;
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_public_assert_with_pub_bind_variant_literal_fires_v004() {
    // A10(b): public sinks travel with the include and must abstract
    // over pub(bind) indexes.
    let source = r"
        pub(bind) index Phase = { Design, Build };
        param cost: Dimensionless[Phase] = {
            Phase.Design: 1.0,
            Phase.Build: 2.0,
        };
        pub assert design_cheap = @cost[Phase.Design] < 10.0;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(matches!(err, GraphcalError::PubIndexVariantLiteral { .. }));
}

#[test]
fn resolve_node_with_plain_pub_variant_literal_ok() {
    // Plain `pub` (fixed) indexes are not bindable, so A10 does not
    // fire on their variant literals; importers cannot override them.
    let source = r"
        pub index Phase = { Design, Build };
        pub const node costs: Dimensionless[Phase] = {
            Phase.Design: 1.0,
            Phase.Build: 2.0,
        };
    ";
    parse_and_resolve(source).unwrap();
}

#[test]
fn resolve_param_with_private_dim_fires_v003() {
    // `param` is implicitly visible (A5 §4.0), so a private dim in a
    // param's signature is V003 (A9 case 1).
    let source = r"
        dim Velocity = Length / Time;
        param speed: Velocity = 10.0 m/s;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { ref_name, .. } if ref_name == "Velocity")
    );
}

#[test]
fn resolve_param_with_pub_dim_ok() {
    let source = r"
        pub dim Velocity = Length / Time;
        param speed: Velocity = 10.0 m/s;
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
            if pub_kind == "dim" && ref_name == "Inner")
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
            if pub_kind == "type" && ref_name == "Inner")
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
            if pub_kind == "type" && ref_name == "Inner")
    );
}

#[test]
fn resolve_pub_bind_index_with_private_dim_fires_v003() {
    // A required range index carries a dim constraint that participates
    // in A9 case 1.
    let source = r"
        dim Frequency = Time^-1;
        pub(bind) index Channel: Frequency;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == "index" && ref_name == "Frequency")
    );
}

#[test]
fn resolve_pub_unit_with_private_dim_fires_v003() {
    let source = r"
        dim Currency = Length;
        pub unit usd: Currency = 1.0 m;
    ";
    let err = parse_and_resolve(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::PrivateInPublic { pub_kind, ref_name, .. }
            if pub_kind == "unit" && ref_name == "Currency")
    );
}
