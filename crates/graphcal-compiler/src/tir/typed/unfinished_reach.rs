//! The unfinished declarations a checked file is answerable for.
//!
//! A file accounts for its own unfinished declarations, used or not, and for
//! every unfinished declaration of another module that one of its
//! declarations reaches. An unfinished declaration of an imported module that
//! nothing in the file reaches belongs to that module alone, so it does not
//! make the file incomplete; checking the module itself reports it.

use std::collections::BTreeSet;

use crate::resolved_name::ResolvedDeclName;

use super::checked::{CheckedDagRegistry, CheckedTir};
use super::checked_dag::CheckedDag;
use super::checked_instance::CheckedInstance;

impl CheckedTir {
    /// The unfinished declarations this file is answerable for: those of its
    /// own DAGs, unused ones included, and those any of its declarations or
    /// include sites reaches through reads, inline DAG calls, includes,
    /// imports, and the scales of the runtime units its literals use.
    ///
    /// Each one is named as its source defines it: an unfinished declaration
    /// copied into an include instance is named by its template declaration,
    /// so every include of one template counts once.
    #[must_use]
    pub fn reachable_unfinished(&self) -> BTreeSet<ResolvedDeclName> {
        let registry = self.dag_registry();
        let own_dags = self
            .local_dags()
            .filter(|(_, dag)| !dag.is_semantic_instance())
            .map(|(_, dag)| dag)
            .collect::<Vec<_>>();
        let mut pending = own_dags
            .iter()
            .flat_map(|dag| dag.declarations().map(|view| view.identity().clone()))
            .chain(
                own_dags
                    .iter()
                    .flat_map(|dag| dag.semantic_instances())
                    .filter_map(|record| registry.semantic_instance(record))
                    .flat_map(exposed_targets),
            )
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut unfinished = BTreeSet::new();
        while let Some(declaration) = pending.pop() {
            if !visited.insert(declaration.clone()) {
                continue;
            }
            let Some(dag) = registry.get(declaration.owner()) else {
                continue;
            };
            if dag.is_unfinished(&declaration) {
                unfinished.insert(source_declaration(registry, declaration.clone()));
            }
            pending.extend(reads(dag, &declaration));
        }
        unfinished
    }
}

/// Every instance declaration an include site exposes: its values,
/// assertions, and requested plots. An exposed declaration that the
/// including DAG materializes as a local alias is also reached through
/// that alias.
fn exposed_targets(instance: CheckedInstance<'_>) -> Vec<ResolvedDeclName> {
    instance
        .output_projections()
        .map(|projection| projection.target)
        .chain(
            instance
                .assertion_projections()
                .map(|projection| projection.target),
        )
        .chain(
            instance
                .plot_projections()
                .map(|projection| projection.target),
        )
        .collect()
}

/// Every declaration `declaration` of `dag` reads directly, including the
/// outputs of the inline DAG calls it makes.
fn reads(dag: &CheckedDag, declaration: &ResolvedDeclName) -> Vec<ResolvedDeclName> {
    let dependencies = dag.dependencies();
    [
        &dependencies.const_deps,
        &dependencies.runtime_deps,
        &dependencies.assert_deps,
        &dependencies.presentation_deps,
    ]
    .into_iter()
    .filter_map(|map| map.get(declaration))
    .flatten()
    .cloned()
    .chain(
        dependencies
            .call_reads
            .get(declaration)
            .into_iter()
            .flatten()
            .map(|call| call.output.clone()),
    )
    .collect()
}

/// The declaration a source file defines that `declaration` is, following
/// include instances back to their templates.
fn source_declaration(
    registry: &CheckedDagRegistry,
    mut declaration: ResolvedDeclName,
) -> ResolvedDeclName {
    while let Some(template) = registry.template_declaration_of(&declaration) {
        declaration = template;
    }
    declaration
}
