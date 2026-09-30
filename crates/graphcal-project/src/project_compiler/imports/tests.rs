use super::*;

fn interface(source: &str) -> ModuleInterface {
    let raw = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    ModuleInterface::new(&graphcal_compiler::desugar::desugared_ast::File::from(raw).declarations)
}

#[test]
fn dependency_interface_preserves_typed_non_param_categories() {
    let dep = interface(
        "const node fixed: Dimensionless = 1.0;\n\
             node computed: Dimensionless = 2.0;\n\
             assert check = true;\n\
             param input: Dimensionless = 1.0;\n\
             plot chart = { mark: point, encode: { x: 1.0 } };",
    );
    let name = |spelling| NameAtom::parse(spelling).unwrap();

    assert_eq!(
        non_param_binding_kind(&dep, &name("fixed")),
        Some(DeclarationKind::ConstNode)
    );
    assert_eq!(
        non_param_binding_kind(&dep, &name("computed")),
        Some(DeclarationKind::Node)
    );
    assert_eq!(
        non_param_binding_kind(&dep, &name("check")),
        Some(DeclarationKind::Assert)
    );
    assert_eq!(non_param_binding_kind(&dep, &name("input")), None);
    assert_eq!(non_param_binding_kind(&dep, &name("chart")), None);
    assert!(declares_runtime_value(&dep, &name("input")));
    assert!(declares_runtime_value(&dep, &name("computed")));
    assert!(!declares_runtime_value(&dep, &name("fixed")));
}

#[test]
fn only_value_declarations_are_graph_values() {
    let values = [
        IntroducedKind::Param,
        IntroducedKind::Node,
        IntroducedKind::ConstNode,
    ];
    let others = [
        IntroducedKind::Assert,
        IntroducedKind::Plot,
        IntroducedKind::Figure,
        IntroducedKind::Layer,
        IntroducedKind::Dag,
        IntroducedKind::Constructor,
        IntroducedKind::BaseDimension,
        IntroducedKind::Dimension,
        IntroducedKind::Unit,
        IntroducedKind::Type,
        IntroducedKind::Index,
    ];
    assert!(values.into_iter().all(is_graph_value_kind));
    assert!(!others.into_iter().any(is_graph_value_kind));
}

#[test]
fn static_bindability_follows_the_declared_role() {
    let dep = interface("pub(bind) type Open;\ntype Closed { Closed }\npub(bind) index Axis;\n");
    let name = |spelling| NameAtom::parse(spelling).unwrap();
    assert!(static_input_is_bindable(
        &dep,
        StaticInputKind::Type,
        &name("Open")
    ));
    assert!(!static_input_is_bindable(
        &dep,
        StaticInputKind::Type,
        &name("Closed")
    ));
    assert!(static_input_is_bindable(
        &dep,
        StaticInputKind::Index,
        &name("Axis")
    ));
    assert!(!static_input_is_bindable(
        &dep,
        StaticInputKind::Dimension,
        &name("Axis")
    ));
}

#[test]
fn include_surface_outputs_expose_ports_and_exported_values() {
    let dep = interface(
        "param input: Dimensionless = 1.0;\n\
             pub node output: Dimensionless = @input;\n\
             node helper: Dimensionless = @input;\n\
             pub const node limit: Dimensionless = 2.0;\n\
             pub assert ok = true;",
    );
    let prefix = ScopeSegment::Named(ModuleAliasName::expect_valid("inst"));
    let scoped =
        |spelling: &str| ScopedName::in_scope(prefix.clone(), DeclName::expect_valid(spelling));
    assert_eq!(
        include_surface_outputs(&dep, &prefix, None),
        vec![scoped("input"), scoped("output"), scoped("limit")]
    );
    let alias = |original: &str, local: &str| ImportAlias {
        original: DeclName::expect_valid(original),
        local: DeclName::expect_valid(local),
    };
    assert_eq!(
        include_surface_outputs(
            &dep,
            &prefix,
            Some(&[
                alias("helper", "h"),
                alias("ok", "o"),
                alias("missing", "m")
            ]),
        ),
        vec![ScopedName::local(DeclName::expect_valid("h"))]
    );
}

#[test]
fn selective_import_records_only_the_canonical_hir_target() {
    let src = NamedSource::new("test.gcl", Arc::new(String::new()));
    let mut imported_names = ImportedValueNames::default();
    let mut imported_bindings = HashMap::new();
    let owner = graphcal_compiler::dag_id::DagId::root_in_package("test", "dep");

    import_selective_resolved_item(
        graphcal_compiler::resolved_name::ResolvedDeclName::for_test(
            owner.clone(),
            DeclName::expect_valid("g0"),
        ),
        &DeclName::expect_valid("local_g0"),
        Span::new(0, 2),
        &src,
        &mut imported_names,
        &mut imported_bindings,
        None,
    )
    .unwrap();

    let lexical = ScopedName::local(DeclName::expect_valid("local_g0"));
    assert_eq!(
        &imported_bindings[&lexical],
        &graphcal_compiler::resolved_name::ResolvedDeclName::for_test(
            owner,
            DeclName::expect_valid("g0"),
        )
    );
}
