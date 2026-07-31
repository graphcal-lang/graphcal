//! Type inference for expressions.
//!
//! Inference walks module-aware HIR exclusively (see [`hir`]); the shared
//! typing-rule kernels live in [`rules`]. The former resolved-syntax-AST
//! walker was retired once every boundary expression (declaration bodies,
//! domain bounds, CLI overrides) gained a stored HIR form (#765).

mod builtin_call;
pub(super) mod hir;
mod linear_algebra;
mod rules;

use super::InferredIndex;
use crate::registry::types::Registry;

/// Look up the canonical index definition for an inferred index identity.
///
/// Concrete finite structurals come from the registry; declared indexes come from the
/// DAG's semantic collection refs.
fn index_def_for_inferred<'a>(
    index: &InferredIndex,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    registry: &'a Registry,
) -> Option<&'a crate::registry::types::IndexDef> {
    if let Some(finite_index) = index.concrete_finite_index() {
        return registry.indexes.get_finite_index(finite_index);
    }
    let resolved = index.declared_resolved()?;
    dag.map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.index_defs.get(resolved))
}

/// Return an inferred axis's cardinality only after it has become concrete.
fn concrete_cardinality_for_inferred(
    index: &InferredIndex,
    dag: Option<&crate::tir::typed::DagTIR>,
    registry: &Registry,
) -> Option<usize> {
    index_def_for_inferred(index, dag, registry)
        .and_then(crate::registry::types::IndexDef::concrete_cardinality)
        .map(crate::registry::index::IndexCardinality::get)
}
