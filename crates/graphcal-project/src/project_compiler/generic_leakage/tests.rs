use std::sync::Arc;

use graphcal_compiler::ir::module_interface::ModuleInterface;

use super::*;
use crate::compile_error::PipelineError;

fn parse_declarations(source: &str) -> Vec<graphcal_compiler::desugar::desugared_ast::Declaration> {
    let raw = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    graphcal_compiler::desugar::desugared_ast::File::from(raw).declarations
}

fn source() -> graphcal_compiler::source_id::SourceId {
    graphcal_compiler::source_registry::SourceRegistry::new()
        .register("main.gcl", Arc::new(String::new()))
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
    let importer_owner = importer();
    check_generics_leakage(
        &declarations,
        StaticScope::new(&owner, &resolver),
        &reexports,
        &StaticSubstitution::default(),
        &IncludingModule {
            interface: &importer_interface,
            source: source(),
            scope: StaticScope::new(&importer_owner, &resolver),
        },
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
    let importer_owner = importer();
    let error = check_generics_leakage(
        &declarations,
        StaticScope::new(&owner, &resolver),
        &reexports,
        &StaticSubstitution::default(),
        &IncludingModule {
            interface: &ModuleInterface::default(),
            source: source(),
            scope: StaticScope::new(&importer_owner, &resolver),
        },
        Span::new(0, 0),
    )
    .unwrap_err();

    match error {
        PipelineError::Semantic(SemanticError::Internal(internal)) => assert!(
            internal.message().contains(
                "required type binding `Element` is absent during generic-leakage analysis"
            ),
            "{}",
            internal.message()
        ),
        other => panic!("expected internal error, got {other:?}"),
    }
}
