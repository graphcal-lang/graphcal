//! Type inference for expressions.
//!
//! Inference walks module-aware HIR exclusively (see [`hir`]); the shared
//! typing-rule kernels live in [`rules`]. The former resolved-syntax-AST
//! walker was retired once every boundary expression (declaration bodies,
//! domain bounds, CLI overrides) gained a stored HIR form (#765).

mod builtin_call;
pub(super) mod hir;
mod rules;

use super::InferredIndex;
use crate::registry::types::Registry;

/// Look up the canonical index definition for an inferred index identity.
///
/// Concrete Nat ranges come from the registry; declared indexes come from the
/// DAG's semantic collection refs.
fn index_def_for_inferred<'a>(
    index: &InferredIndex,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    registry: &'a Registry,
) -> Option<&'a crate::registry::types::IndexDef> {
    if let Some(nat_range) = index.concrete_nat_range() {
        return registry.indexes.get_nat_range(nat_range);
    }
    let resolved = index.declared_resolved()?;
    dag.map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.index_defs.get(resolved))
}
