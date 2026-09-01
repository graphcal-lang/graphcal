//! Transition from a checked semantic project to a reusable runtime plan.

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::static_interface::StaticInputKind;
use graphcal_compiler::registry::error::GraphcalError;

use crate::eval::types::CompileError;
use crate::host_fns::HostFunctionRegistry;
use crate::project_compiler::{CheckedProject, CheckedProjectRuntimeParts, ProjectCompiler};

use super::PreparedProject;

pub(super) fn prepare_checked_project(
    checked: CheckedProject,
    host_fns: &HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, CompileError> {
    cancellation.checkpoint()?;
    let CheckedProjectRuntimeParts {
        compiled,
        source,
        module_resolver,
    } = checked.into_runtime_parts();
    if let Some(index) = compiled.entry_interface.required_index() {
        let Some(span) = index.anchor().resolve(source.inner().len()) else {
            return Err(CompileError::Eval(GraphcalError::internal_error(
                format!(
                    "required index `{}` has no diagnostic source anchor",
                    index.name()
                ),
                &source,
                DiagnosticAnchor::Builtin,
            )));
        };
        return Err(CompileError::Eval(
            GraphcalError::RequiredStaticInputNotBound {
                kind: StaticInputKind::Index,
                name: index.name().to_string(),
                src: source,
                span: span.into(),
            },
        ));
    }

    let plan = crate::exec_plan::compile_checked_with_cancellation(
        &compiled.tir,
        &compiled.checked_execution_facts,
        &source,
        cancellation,
    )?;
    PreparedProject::from_compiled(compiled, plan, source, host_fns.clone(), module_resolver)
}

impl ProjectCompiler<'_, HostFunctionRegistry> {
    /// Check and prepare this configured session for repeated evaluation.
    ///
    /// # Errors
    ///
    /// Returns a compile, plan, interface, plugin, or cancellation diagnostic.
    pub fn prepare(self) -> Result<PreparedProject, CompileError> {
        let cancellation = self.cancellation_token().clone();
        let host_fns = self.callable_host().clone();
        self.check()?
            .prepare_with_host_fns_and_cancellation(&host_fns, &cancellation)
    }

    /// Check, prepare, bind, and evaluate one row.
    ///
    /// # Errors
    ///
    /// Returns a compile, binding, evaluation, or cancellation diagnostic.
    #[expect(
        clippy::implicit_hasher,
        reason = "public API accepts HashMap without requiring a specific hasher"
    )]
    pub fn eval(
        self,
        overrides: &std::collections::HashMap<
            graphcal_compiler::syntax::decl_name::DeclName,
            graphcal_compiler::desugar::desugared_ast::Expr,
        >,
    ) -> Result<crate::eval::types::EvalResult, CompileError> {
        let cancellation = self.cancellation_token().clone();
        let prepared = self.prepare()?;
        let mut bindings = prepared.binding_builder();
        for (name, expression) in overrides {
            cancellation.checkpoint()?;
            bindings.bind_expression(name, expression)?;
        }
        let row = bindings.finish()?;
        prepared.evaluate_with_cancellation(&row, &cancellation)
    }
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
