//! Type inference for expressions.
//!
//! Inference walks module-aware HIR exclusively (see [`hir`]); the shared
//! typing-rule kernels live in [`rules`]. The former resolved-syntax-AST
//! walker was retired once every boundary expression (declaration bodies,
//! domain bounds) gained a stored HIR form (#765). Closed external binding
//! values are lowered independently and checked through the same HIR rules.

mod builtin_call;
mod complex;
pub(super) mod hir;
mod linear_algebra;
mod rules;

pub(in crate::tir::dim_check) use rules::resolve_unit_dimension_or_diagnose;

use super::InferredIndex;
/// Look up an inferred index through the project-wide semantic authority.
fn index_def_for_inferred<'a>(
    index: &InferredIndex,
    tir: &'a crate::tir::typed::TIR,
) -> Option<&'a crate::registry::types::IndexDef> {
    tir.index_def(index.type_ref())
}

/// Return an inferred axis's cardinality only after it has become concrete.
fn concrete_cardinality_for_inferred(
    index: &InferredIndex,
    tir: &crate::tir::typed::TIR,
) -> Option<usize> {
    index_def_for_inferred(index, tir)
        .and_then(crate::registry::types::IndexDef::concrete_cardinality)
        .map(crate::registry::index::IndexCardinality::get)
}
