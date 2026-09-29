//! Value-independent TODO reachability across call outputs, including unselected branches.
//!
//! Explicit call arguments cut off parameter defaults. No expression is executed
//! and no hypothetical runtime value is introduced during this analysis.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::expr::{Expr, ExprKind, LocalDecl, visit_expr};
use graphcal_compiler::node_unavailable::NodeUnavailable;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::texpr::{TExpr, TExprKind, TNodeRef, visit_tnodes};
use miette::NamedSource;

use crate::execution_plan::{ExecPlan, PlannedBody};

type BoundParameters = BTreeSet<ResolvedDeclName>;
/// A call output and the parameters its call binds explicitly.
pub type Query = (ResolvedDeclName, BoundParameters);
type Origins = BTreeSet<ResolvedDeclName>;

/// The inline calls an expression's availability depends on.
///
/// References through `@name` are body handles, resolved only in the scope
/// the expression runs in; see [`graph_refs`].
pub trait ExpressionDependencies {
    /// Every inline DAG call's output and explicitly bound parameters.
    fn dag_calls(&self) -> Vec<Query>;
}

impl ExpressionDependencies for Expr {
    fn dag_calls(&self) -> Vec<Query> {
        calls(self, &BoundParameters::new())
    }
}

/// Declarations a checked tree references through `@name`, including
/// unselected branches, as handles of the scope the tree runs in.
pub fn graph_refs(tree: &TExpr) -> BTreeSet<LocalDecl> {
    let mut refs = BTreeSet::new();
    visit_tnodes(TNodeRef::Value(tree), &mut |node| {
        if let TNodeRef::Value(expr) = node
            && let TExprKind::GraphRef(target) = expr.kind()
        {
            refs.insert(target.value.clone());
        }
    });
    refs
}

impl ExpressionDependencies for TExpr {
    fn dag_calls(&self) -> Vec<Query> {
        let mut calls = Vec::new();
        visit_tnodes(TNodeRef::Value(self), &mut |node| {
            if let TNodeRef::Value(expr) = node
                && let TExprKind::DagCall { args, output, .. } = expr.kind()
            {
                calls.push((
                    output.value.clone(),
                    args.iter().map(|binding| binding.target.clone()).collect(),
                ));
            }
        });
        calls
    }
}

pub fn collect(
    expression: &(impl ExpressionDependencies + ?Sized),
    plan: &ExecPlan<'_>,
    source: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<Vec<(ResolvedDeclName, NodeUnavailable)>, GraphcalError> {
    if !plan.has_unfinished_definitions() {
        return Ok(Vec::new());
    }
    let mut analysis = Analysis {
        plan,
        source,
        cancellation,
        memo: HashMap::new(),
        active: HashSet::new(),
    };
    expression
        .dag_calls()
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
    plan: &'a ExecPlan<'a>,
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
        let plan = self.plan;
        let declaration = plan.declaration(name).ok_or_else(|| {
            self.invalid(format!(
                "declaration `{name}` has no prepared physical location"
            ))
        })?;
        let mut origins = Origins::new();
        match declaration.body() {
            PlannedBody::Todo => {
                origins.insert(name.clone());
            }
            PlannedBody::Expression { root, .. } => {
                for dependency in declaration.reads() {
                    origins.extend(self.declaration(dependency, bound)?);
                }
                for (output, parameters) in calls(root.get(), bound) {
                    origins.extend(self.declaration(&output, &parameters)?);
                }
            }
            // Required ports have no default; constants cannot contain TODOs.
            PlannedBody::Supplied => {}
        }
        self.active.remove(&query);
        self.memo.insert(query, origins.clone());
        Ok(origins)
    }
}
