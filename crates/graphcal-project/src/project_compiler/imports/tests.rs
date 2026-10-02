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
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("test.gcl", std::sync::Arc::new(String::new()));
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
        src,
        &mut imported_names,
        &mut imported_bindings,
        None,
    )
    .unwrap();

    let lexical = ScopedName::local(DeclName::expect_valid("local_g0"));
    assert_eq!(
        &imported_bindings[&lexical].value,
        &graphcal_compiler::resolved_name::ResolvedDeclName::for_test(
            owner,
            DeclName::expect_valid("g0"),
        )
    );
}

fn static_declarations(
    source: &str,
) -> Vec<graphcal_compiler::desugar::desugared_ast::Declaration> {
    let raw = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    graphcal_compiler::desugar::desugared_ast::File::from(raw).declarations
}

fn virtual_module(name: &str) -> graphcal_compiler::dag_id::DagId {
    graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(name))
        .unwrap()
}

#[test]
fn covered_bindings_resolve_every_port_and_target_canonically() {
    let template = static_declarations("pub(bind) dim Q;\npub(bind) type T;\npub(bind) index I;");
    let importer =
        static_declarations("dim Length;\ntype Box { Box(value: Int), }\nindex Phase = { A, B };");
    let (template_id, importer_id) = (virtual_module("dep.gcl"), virtual_module("main.gcl"));
    let resolver = graphcal_compiler::resolve::ModuleResolver::without_edges([
        (template_id.clone(), template.as_slice()),
        (importer_id.clone(), importer.as_slice()),
    ])
    .unwrap();
    let src = graphcal_compiler::source_registry::SourceRegistry::new()
        .register("main.gcl", std::sync::Arc::new(String::new()));
    let span = Span::new(0, 0);
    let covered = CoveredStaticBindings::check(
        &ModuleInterface::new(&template),
        HashMap::from([(
            IndexName::expect_valid("I"),
            IndexBindingTarget::Declared(IndexName::expect_valid("Phase")),
        )]),
        HashMap::new(),
        HashMap::from([(
            StructTypeName::expect_valid("T"),
            StructTypeName::expect_valid("Box"),
        )]),
        HashMap::from([(DimName::expect_valid("Q"), DimName::expect_valid("Length"))]),
        src,
        span,
    )
    .unwrap();
    let bindings = covered
        .resolve(
            &IncludeSite {
                template: &template_id,
                importer: StaticScope::new(&importer_id, &resolver),
                src,
            },
            span,
        )
        .unwrap();

    let substitution = bindings.substitution();
    let [(port, target)] = substitution.dimensions.iter().collect::<Vec<_>>()[..] else {
        panic!("{substitution:?}");
    };
    assert_eq!((port.owner(), port.as_str()), (&template_id, "Q"));
    assert_eq!((target.owner(), target.as_str()), (&importer_id, "Length"));
    let [(port, target)] = substitution.types.iter().collect::<Vec<_>>()[..] else {
        panic!("{substitution:?}");
    };
    assert_eq!((port.owner(), port.as_str()), (&template_id, "T"));
    assert_eq!((target.owner(), target.as_str()), (&importer_id, "Box"));
    let [(port, bound)] = bindings.indexes().iter().collect::<Vec<_>>()[..] else {
        panic!("{substitution:?}");
    };
    assert_eq!((port.owner(), port.as_str()), (&template_id, "I"));
    assert!(matches!(
        &bound.target,
        graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget::Declared(target)
            if target.owner() == &importer_id && target.as_str() == "Phase"
    ));
    assert_eq!(bindings.dimensions().len(), 1);
}

#[test]
fn a_target_the_importer_does_not_declare_is_a_user_error() {
    let template = static_declarations("pub(bind) index I;");
    let importer = static_declarations("dim Length;");
    let (template_id, importer_id) = (virtual_module("dep.gcl"), virtual_module("main.gcl"));
    let resolver = graphcal_compiler::resolve::ModuleResolver::without_edges([
        (template_id.clone(), template.as_slice()),
        (importer_id.clone(), importer.as_slice()),
    ])
    .unwrap();
    let src = graphcal_compiler::source_registry::SourceRegistry::new()
        .register("main.gcl", std::sync::Arc::new(String::new()));
    let covered = CoveredStaticBindings::check(
        &ModuleInterface::new(&template),
        HashMap::from([(
            IndexName::expect_valid("I"),
            IndexBindingTarget::Declared(IndexName::expect_valid("Length")),
        )]),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        src,
        Span::new(0, 0),
    )
    .unwrap();
    let error = covered
        .resolve(
            &IncludeSite {
                template: &template_id,
                importer: StaticScope::new(&importer_id, &resolver),
                src,
            },
            Span::new(0, 0),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            PipelineError::Semantic(SemanticError::Located(ref diagnostic))
                if matches!(
                    diagnostic.kind,
                    graphcal_compiler::semantic_error::SemanticErrorKind::Module(
                        ModuleError::IndexBindingNotAnIndex { .. }
                    )
                )
        ),
        "{error:?}"
    );
}
