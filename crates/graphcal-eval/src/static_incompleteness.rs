//! Value-independent TODO reachability across call outputs, including unselected branches.
//!
//! Explicit call arguments cut off parameter defaults. No expression is executed
//! and no hypothetical runtime value is introduced during this analysis.

use std::collections::{BTreeSet, HashMap, HashSet};

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::expr::{Expr, ExprKind, visit_expr};
use graphcal_compiler::node_unavailable::NodeUnavailable;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::scoped_node::{NodeKind, ScopedNode};

use crate::execution_plan::{ComputedBody, ExecPlan, PlannedBody};

type BoundParameters = BTreeSet<ResolvedDeclName>;
/// A call output and the parameters its call binds explicitly.
pub type Query = (ResolvedDeclName, BoundParameters);
type Origins = BTreeSet<ResolvedDeclName>;

/// The inline calls an expression's availability depends on.
///
/// References through `@name` are body handles, resolved only in the scope
/// the expression runs in ([`ScopedNode::graph_refs`]).
pub trait ExpressionDependencies {
    /// Every inline DAG call's output and explicitly bound parameters.
    fn dag_calls(&self) -> Vec<Query>;
}

impl ExpressionDependencies for &Expr {
    fn dag_calls(&self) -> Vec<Query> {
        calls(self, &BoundParameters::new())
    }
}

impl ExpressionDependencies for ScopedNode<'_> {
    fn dag_calls(&self) -> Vec<Query> {
        let mut calls = Vec::new();
        self.visit(&mut |node| {
            if let NodeKind::DagCall { args, output, .. } = node.kind() {
                calls.push((
                    output.value.clone(),
                    args.get()
                        .iter()
                        .map(|binding| binding.target.clone())
                        .collect(),
                ));
            }
        });
        calls
    }
}

pub fn collect(
    expression: &(impl ExpressionDependencies + ?Sized),
    plan: &ExecPlan<'_>,
    source: SourceId,
    cancellation: &CancellationToken,
) -> Result<Vec<(ResolvedDeclName, NodeUnavailable)>, Outcome<SemanticError>> {
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
    source: SourceId,
    cancellation: &'a CancellationToken,
    memo: HashMap<Query, Origins>,
    active: HashSet<Query>,
}

impl Analysis<'_> {
    fn invalid(&self, message: impl Into<String>) -> SemanticError {
        SemanticError::internal_error(message, self.source, DiagnosticAnchor::WholeFile)
    }

    fn declaration(
        &mut self,
        name: &ResolvedDeclName,
        bound: &BoundParameters,
    ) -> Result<Origins, Outcome<SemanticError>> {
        graphcal_compiler::stack::with_stack_growth(|| self.declaration_inner(name, bound))
    }

    fn declaration_inner(
        &mut self,
        name: &ResolvedDeclName,
        bound: &BoundParameters,
    ) -> Result<Origins, Outcome<SemanticError>> {
        self.cancellation.checkpoint()?;
        if bound.contains(name) {
            return Ok(Origins::new());
        }
        let query = (name.clone(), bound.clone());
        if let Some(origins) = self.memo.get(&query) {
            return Ok(origins.clone());
        }
        if !self.active.insert(query.clone()) {
            return Err(self
                .invalid(format!("cyclic checked call dependency at `{name}`"))
                .into());
        }
        let plan = self.plan;
        let declaration = plan.declaration(name).ok_or_else(|| {
            self.invalid(format!(
                "declaration `{name}` has no prepared physical location"
            ))
        })?;
        let mut origins = Origins::new();
        match declaration.body() {
            PlannedBody::Computed(ComputedBody::Todo) => {
                origins.insert(name.clone());
            }
            PlannedBody::Computed(ComputedBody::Expression { root, .. }) => {
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
