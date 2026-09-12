//! Value-independent TODO reachability across call outputs, including unselected branches.
//!
//! Explicit call arguments cut off parameter defaults. No expression is executed
//! and no hypothetical runtime value is introduced during this analysis.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::expr::{Expr, ExprKind, visit_expr};
use graphcal_compiler::node_unavailable::NodeUnavailable;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::model::TIR;
use miette::NamedSource;

use crate::decl_key::RuntimeDeclKey;
use crate::execution_plan::ExecPlan;

type BoundParameters = BTreeSet<ResolvedDeclName>;
type Query = (ResolvedDeclName, BoundParameters);
type Origins = BTreeSet<ResolvedDeclName>;

pub fn collect(
    expression: &Expr,
    tir: &TIR,
    plan: &ExecPlan,
    source: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<Vec<(ResolvedDeclName, NodeUnavailable)>, GraphcalError> {
    if !plan.has_unfinished_definitions {
        return Ok(Vec::new());
    }
    let mut analysis = Analysis {
        tir,
        plan,
        source,
        cancellation,
        memo: HashMap::new(),
        active: HashSet::new(),
    };
    calls(expression, &BoundParameters::new())
        .into_iter()
        .try_fold(Vec::new(), |mut results, (output, bound)| {
            let origins = analysis.declaration(&output, &bound)?;
            if let Ok(unfinished) = NonEmpty::try_from_vec(origins.into_iter().collect()) {
                results.push((
                    output,
                    NodeUnavailable::Blocked {
                        unfinished,
                        failed_deps: Vec::new(),
                    },
                ));
            }
            Ok(results)
        })
}

fn calls(expression: &Expr, bound: &BoundParameters) -> Vec<Query> {
    let mut calls = Vec::new();
    visit_expr(expression, &mut |expression| {
        if let ExprKind::DagCall { args, output, .. } = expression.kind() {
            let parameters = bound
                .iter()
                .cloned()
                .chain(args.iter().map(|binding| binding.target.value.clone()))
                .collect();
            calls.push((output.value.clone(), parameters));
        }
    });
    calls
}

struct Analysis<'a> {
    tir: &'a TIR,
    plan: &'a ExecPlan,
    source: &'a NamedSource<Arc<String>>,
    cancellation: &'a CancellationToken,
    memo: HashMap<Query, Origins>,
    active: HashSet<Query>,
}

impl Analysis<'_> {
    fn invalid(&self, message: impl Into<String>) -> GraphcalError {
        GraphcalError::internal_error(message, self.source, DiagnosticAnchor::WholeFile)
    }

    fn declaration(
        &mut self,
        name: &ResolvedDeclName,
        bound: &BoundParameters,
    ) -> Result<Origins, GraphcalError> {
        graphcal_compiler::stack::with_stack_growth(|| self.declaration_inner(name, bound))
    }

    fn declaration_inner(
        &mut self,
        name: &ResolvedDeclName,
        bound: &BoundParameters,
    ) -> Result<Origins, GraphcalError> {
        self.cancellation.checkpoint()?;
        if bound.contains(name) {
            return Ok(Origins::new());
        }
        let query = (name.clone(), bound.clone());
        if let Some(origins) = self.memo.get(&query) {
            return Ok(origins.clone());
        }
        if !self.active.insert(query.clone()) {
            return Err(self.invalid(format!("cyclic checked call dependency at `{name}`")));
        }
        let key = RuntimeDeclKey::resolved(name.clone());
        let owner = self
            .plan
            .declaration_locations
            .body_for(&key)
            .map_err(|error| self.invalid(error.to_string()))?;
        let dag = self
            .tir
            .dag_registry()
            .get(owner)
            .ok_or_else(|| self.invalid(format!("checked declaration `{name}` has no body")))?;
        let mut origins = Origins::new();
        match (dag.todo(name), dag.runtime_expr(name)) {
            (Some(_), _) => {
                origins.insert(name.clone());
            }
            (None, Some(expression)) => {
                let callable = self
                    .plan
                    .callable(owner)
                    .map_err(|error| self.invalid(error.to_string()))?;
                let dependencies = callable.dependencies.get(&key).ok_or_else(|| {
                    self.invalid(format!("checked declaration `{name}` has no dependencies"))
                })?;
                for dependency in dependencies {
                    origins.extend(self.declaration(dependency.as_resolved(), bound)?);
                }
                for (output, parameters) in calls(expression, bound) {
                    origins.extend(self.declaration(&output, &parameters)?);
                }
            }
            // Required ports have no default; constants cannot contain TODOs.
            (None, None) => {}
        }
        self.active.remove(&query);
        self.memo.insert(query, origins.clone());
        Ok(origins)
    }
}
