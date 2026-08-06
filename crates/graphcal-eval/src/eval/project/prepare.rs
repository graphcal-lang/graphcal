//! Transition from a checked semantic project to a reusable runtime plan.

use graphcal_compiler::desugar::desugared_ast::{DeclKind, File};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::TIR;

use crate::eval::types::CompileError;
use crate::host_fns::HostFunctionRegistry;
use crate::project_compiler::{CheckedProject, CheckedProjectRuntimeParts};

use super::PreparedProject;

fn tir_has_required_indexes(tir: &TIR) -> bool {
    tir.root_declared_indexes()
        .any(graphcal_compiler::registry::types::IndexDef::is_required)
        || tir
            .root()
            .semantic()
            .collection_refs
            .index_defs
            .values()
            .any(|definition| definition.is_required())
}

fn first_required_index_diagnostic(tir: &TIR, ast: &File) -> Option<(String, miette::SourceSpan)> {
    for idx_def in tir.root_declared_indexes() {
        if idx_def.is_required() {
            let span = ast
                .declarations
                .iter()
                .find_map(|declaration| {
                    if let DeclKind::Index(index) = &declaration.kind
                        && index.name.value.as_str() == idx_def.name.as_str()
                    {
                        Some(declaration.span.into())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| miette::SourceSpan::from((0, 0)));
            return Some((idx_def.name.to_string(), span));
        }
    }

    tir.root()
        .semantic()
        .collection_refs
        .index_defs
        .values()
        .find(|definition| definition.is_required())
        .map(|definition| {
            // Owner-qualified required indexes can enter through module type
            // resolution. The reference span is not retained in index_defs, so
            // point at the importing file as a whole rather than fabricating a
            // declaration span.
            (
                definition.name.to_string(),
                miette::SourceSpan::from((0, 0)),
            )
        })
}

pub(super) fn prepare_checked_project(
    checked: CheckedProject,
    host_fns: &HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, CompileError> {
    cancellation.checkpoint()?;
    let CheckedProjectRuntimeParts {
        compiled,
        source,
        root_ast,
        module_resolver,
    } = checked.into_runtime_parts();
    if tir_has_required_indexes(&compiled.tir)
        && let Some((name, span)) = first_required_index_diagnostic(&compiled.tir, &root_ast)
    {
        return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
            name,
            src: source,
            span,
        }));
    }

    let plan = crate::exec_plan::compile_checked_with_cancellation(
        &compiled.tir,
        &compiled.checked_execution_facts,
        &source,
        cancellation,
    )?;
    PreparedProject::from_compiled(
        compiled,
        plan,
        source,
        host_fns.clone(),
        module_resolver,
        &root_ast,
    )
}

impl CheckedProject {
    /// Continue to runtime preparation with callable host functions.
    ///
    /// # Errors
    ///
    /// Returns a plan, input-interface, plugin, or cancellation diagnostic.
    pub fn prepare_with_host_fns_and_cancellation(
        self,
        host_fns: &HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<PreparedProject, CompileError> {
        prepare_checked_project(self, host_fns, cancellation)
    }

    /// Continue to runtime preparation without a cancellation deadline.
    ///
    /// # Errors
    ///
    /// Returns a plan, input-interface, or plugin diagnostic.
    pub fn prepare_with_host_fns(
        self,
        host_fns: &HostFunctionRegistry,
    ) -> Result<PreparedProject, CompileError> {
        self.prepare_with_host_fns_and_cancellation(
            host_fns,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }
}
