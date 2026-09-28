//! Lowering of desugared syntax expressions into HIR expression trees.
//!
//! This is the single name-resolution stage of the pipeline: syntactic
//! reference paths ([`crate::syntax::ast::UnresolvedRef`]) are classified and
//! resolved here, in one pass, against the lexical scope and the module-aware
//! resolver. Source paths (`NamePath` / `IdentPath` / `ScopedName`) are
//! consumed at this boundary.
//!
//! Lowering is diagnostic-accumulating: the walk produces
//! [`Tolerant`](tolerant::Tolerant) HIR, where a reference that cannot be
//! resolved becomes an explicit error node carrying its diagnostic, so IDE
//! consumers can keep working on incomplete code. The strict entry points
//! ([`lower::lower_expr`], [`lower::lower_assert_body`]) refine that tree
//! into [`Strict`](crate::hir::expr::Strict) HIR, which cannot represent an
//! error node, so the batch pipeline never sees one.
//!
//! - [`lower`]: entry points and the structural lowering walk.
//! - [`context`]: the inputs that scope one lowering run.
//! - [`error`]: lowering diagnostics.
//! - `lowerer`: lowerer state and lexical scopes.
//! - `resolve`: value-position name resolution.
//! - `call`: function-call application and datetime literals.
//! - [`tolerant`]: the tolerant completeness and its refinement to strict HIR.

mod call;
pub mod context;
pub mod error;
pub mod lower;
mod lowerer;
mod resolve;
#[cfg(test)]
mod tests;
pub mod tolerant;
