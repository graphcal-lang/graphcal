use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::ir::module_interface::ModuleInterface;

use super::super::include_static_bindings::{CoveredStaticBindings, StaticBindingResolution};
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

/// The resolution of an include that binds nothing.
struct NothingBound;

impl StaticBindingResolution for NothingBound {
    fn index(
        &self,
        port: &graphcal_compiler::syntax::index_name::IndexName,
        _: &graphcal_compiler::semantic::index_def::IndexBindingTarget,
        _: Span,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedIndexName,
            InstanceIndexBindingTarget,
        ),
        PipelineError,
    > {
        panic!("unexpected index binding {port}")
    }

    fn struct_type(
        &self,
        port: &graphcal_compiler::syntax::type_name::StructTypeName,
        _: &graphcal_compiler::syntax::type_name::StructTypeName,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedStructTypeName,
            graphcal_compiler::resolved_name::ResolvedStructTypeName,
        ),
        PipelineError,
    > {
        panic!("unexpected type binding {port}")
    }

    fn dimension(
        &self,
        port: &graphcal_compiler::syntax::dimension::DimName,
        _: &graphcal_compiler::syntax::dimension::DimName,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedDimName,
            graphcal_compiler::resolved_name::ResolvedDimName,
        ),
        PipelineError,
    > {
        panic!("unexpected dimension binding {port}")
    }
}

/// The Static bindings of an include that binds nothing.
fn unbound(dependency: &ModuleInterface) -> Result<CoveredStaticBindings, PipelineError> {
    CoveredStaticBindings::check(
        dependency,
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        source(),
        Span::new(0, 0),
    )
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
    let bindings = unbound(&ModuleInterface::new(&declarations))
        .unwrap()
        .resolve(&NothingBound, Span::new(0, 0))
        .unwrap();
    check_generics_leakage(
        &declarations,
        StaticScope::new(&owner, &resolver),
        &reexports,
        &bindings,
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
fn bindings_leaving_a_required_port_unbound_are_rejected_before_leakage_analysis() {
    let declarations =
        parse_declarations("pub(bind) type Element; node output: Element = Missing;");
    let error = unbound(&ModuleInterface::new(&declarations)).unwrap_err();

    match error {
        PipelineError::Semantic(SemanticError::Located(diagnostic)) => assert!(
            matches!(
                diagnostic.kind,
                graphcal_compiler::semantic_error::SemanticErrorKind::Index(
                    graphcal_compiler::semantic_error::index::IndexError::RequiredStaticInputNotBound {
                        kind: graphcal_compiler::static_interface::StaticInputKind::Type,
                        ..
                    }
                )
            ),
            "{diagnostic:?}"
        ),
        other => panic!("expected I010, got {other:?}"),
    }
}
