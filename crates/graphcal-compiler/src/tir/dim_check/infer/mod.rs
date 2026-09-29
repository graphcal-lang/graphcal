//! Type inference for expressions.
//!
//! Inference walks module-aware HIR exclusively (see [`hir`]); the shared
//! typing-rule kernels live in [`rules`]. The former resolved-syntax-AST
//! walker was retired once every boundary expression (declaration bodies,
//! domain bounds) gained a stored HIR form (#765). Closed external binding
//! values are lowered independently and checked through the same HIR rules.

mod complex;
pub(super) mod hir;
mod linear_algebra;
mod rules;

use crate::registry::checked_type::{IndexTypeRef, Symbolic};
/// Look up an inferred index through the project-wide semantic authority.
fn index_def_for_inferred<'a>(
    index: &IndexTypeRef<Symbolic>,
    tir: &'a crate::tir::typed::UncheckedTir,
) -> Option<std::borrow::Cow<'a, crate::registry::types::IndexDef>> {
    tir.index_def(index)
}

/// Return an inferred axis's cardinality only after it has become concrete.
fn concrete_cardinality_for_inferred(
    index: &IndexTypeRef<Symbolic>,
    tir: &crate::tir::typed::UncheckedTir,
) -> Option<usize> {
    index_def_for_inferred(index, tir)
        .and_then(|definition| definition.concrete_cardinality())
        .map(crate::registry::index::IndexCardinality::get)
}
