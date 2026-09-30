//! Canonical nominal-override dependencies of already-checked source modules,
//! the input that refines include override obligations at instantiation.

use std::collections::{HashMap, HashSet};

use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedStructTypeName};

/// Canonical nominal identity whose use in a parameter default is queried at
/// an include boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NominalOverrideIdentity {
    Index(ResolvedIndexName),
    Type(ResolvedStructTypeName),
}

/// Canonical nominal dependencies keyed by the producer parameter default that
/// uses them.
pub type OverrideDependencySummary = HashMap<ResolvedDeclName, HashSet<NominalOverrideIdentity>>;

/// Canonical override dependency summaries of already-checked source modules.
///
/// HIR records each possible nominal override before substituting an include
/// instance. Once a source module has been checked, its canonical dependency
/// summary distinguishes a true dependency on the replaced nominal from an
/// unrelated use of the replacement type or index itself.
#[derive(Debug, Default)]
pub struct CheckedOverrideDependencies {
    summary: OverrideDependencySummary,
    complete_owners: HashSet<crate::dag_id::DagId>,
}

impl CheckedOverrideDependencies {
    /// Summaries `summary` collected from the modules owning `complete_owners`.
    #[must_use]
    pub const fn new(
        summary: OverrideDependencySummary,
        complete_owners: HashSet<crate::dag_id::DagId>,
    ) -> Self {
        Self {
            summary,
            complete_owners,
        }
    }

    /// Refine include override obligations of every local body. The
    /// refinement is applied to every materialized semantic instance, not only
    /// the entry DAG, because each instance owns its own executable
    /// reconciliation facts.
    pub(in crate::tir::typed) fn reconcile(&self, tir: &mut super::program::UncheckedTir) {
        for dag in tir.dags.values_mut() {
            dag.semantic
                .override_reconciliations
                .retain(|_, reconciliations| {
                    reconciliations.retain_mut(|reconciliation| {
                        if !self
                            .complete_owners
                            .contains(reconciliation.source_decl.owner())
                        {
                            return true;
                        }
                        let dependencies = self.summary.get(&reconciliation.source_decl);
                        reconciliation.targets.retain(|target| {
                            let source = match target {
                                crate::ir::override_reconciliation::OverrideTarget::Index {
                                    source,
                                    ..
                                } => NominalOverrideIdentity::Index(source.clone()),
                                crate::ir::override_reconciliation::OverrideTarget::Type {
                                    source,
                                    ..
                                } => NominalOverrideIdentity::Type(source.clone()),
                            };
                            dependencies.is_some_and(|dependencies| dependencies.contains(&source))
                        });
                        !reconciliation.targets.is_empty()
                    });
                    !reconciliations.is_empty()
                });
        }
    }
}
