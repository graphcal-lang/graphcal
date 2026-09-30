use super::*;

fn parse_declarations(source: &str) -> Vec<graphcal_compiler::desugar::desugared_ast::Declaration> {
    let raw = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    graphcal_compiler::desugar::desugared_ast::File::from(raw).declarations
}

fn source() -> NamedSource<Arc<String>> {
    NamedSource::new("main.gcl", Arc::new(String::new()))
}

fn dependency() -> DagId {
    DagId::from_virtual_relative_path(std::path::Path::new("dep.gcl")).unwrap()
}

fn importer() -> DagId {
    DagId::from_virtual_relative_path(std::path::Path::new("main.gcl")).unwrap()
}

fn resolver(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
) -> graphcal_compiler::resolve::ModuleResolver {
    let mut tables = graphcal_compiler::resolve::builder::SymbolTables::default();
    tables.add_file(dependency(), declarations).unwrap();
    tables
        .scopes(&graphcal_compiler::resolve::builder::NoModuleTargets)
        .unwrap()
        .freeze()
        .unwrap()
}

#[test]
fn unsubstituted_dependency_name_is_not_probed_in_the_importer() {
    let declarations = parse_declarations(
        "type Inner { Inner(value: Int), } node output: Inner = Inner(value: 1);",
    );
    let reexports = HashSet::from([NameAtom::parse("output").unwrap()]);
    let importer_interface =
        ModuleInterface::new(&parse_declarations("type Inner { Inner(value: Int), }"));

    let resolver = resolver(&declarations);
    let owner = dependency();
    check_generics_leakage(
        &declarations,
        StaticScope::new(&owner, &resolver),
        &reexports,
        &StaticSubstitution::default(),
        &importer(),
        &importer_interface,
        &source(),
        Span::new(0, 0),
    )
    .unwrap();
}

#[test]
fn missing_required_substitution_is_an_internal_error() {
    let declarations =
        parse_declarations("pub(bind) type Element; node output: Element = Missing;");
    let reexports = HashSet::from([NameAtom::parse("output").unwrap()]);

    let resolver = resolver(&declarations);
    let owner = dependency();
    let error = check_generics_leakage(
        &declarations,
        StaticScope::new(&owner, &resolver),
        &reexports,
        &StaticSubstitution::default(),
        &importer(),
        &ModuleInterface::default(),
        &source(),
        Span::new(0, 0),
    )
    .unwrap_err();

    match error {
        CompileError::Eval(GraphcalError::InternalError { message, .. }) => assert!(
            message.contains(
                "required type binding `Element` is absent during generic-leakage analysis"
            ),
            "{message}"
        ),
        other => panic!("expected internal error, got {other:?}"),
    }
}
