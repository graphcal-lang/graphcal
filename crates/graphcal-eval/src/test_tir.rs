//! Test-only checked TIR for single-file programs, built with the compiler
//! alone (no project loading, imports, or inline DAGs).

use std::sync::Arc;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::ir::lower::lower;
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::syntax::parser::Parser;
use graphcal_compiler::tir::typed::ProjectTypeStore;
use miette::NamedSource;

/// Check a single-file program named `test.gcl`.
pub fn checked_tir_from_source(
    source: &str,
) -> Result<
    (
        graphcal_compiler::tir::typed::CheckedTir,
        NamedSource<Arc<String>>,
    ),
    GraphcalError,
> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
    let ir = lower(&file, &src).unwrap();
    let resolver =
        ModuleResolver::without_edges([(ir.dag_id().clone(), file.declarations.as_slice())])
            .unwrap();
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_module(ir.definitions()).unwrap();
    let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
    let signed =
        graphcal_compiler::tir::typed::resolve_hir_signature_with_modules_and_cancellation(
            ir,
            &src,
            &resolver,
            &project_types,
            &cancellation,
        )
        .unwrap();
    graphcal_compiler::tir::typed::TirDraft::resolve_root(
        signed,
        std::collections::HashMap::<_, _, std::hash::RandomState>::new(),
        &src,
        &resolver,
        Arc::new(project_types),
        &cancellation,
    )
    .unwrap()
    .instantiate(
        &graphcal_compiler::tir::typed::CheckedOverrideDependencies::default(),
        &src,
    )
    .unwrap()
    .check(&src, &cancellation)
    .map(|tir| (tir, src.clone()))
}
