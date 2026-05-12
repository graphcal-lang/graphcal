//! Generic desugaring pipeline.
//!
//! Transforms `File<Raw>` (parser output, with all surface sugar) into
//! `File<Desugared>` (canonical form consumed by name resolution, IR lowering,
//! TIR, and evaluation).
//!
//! # Architecture
//!
//! Each surface sugar implements [`DesugarSugar`] — a node-level transform
//! that maps one raw sugar variant to its desugared equivalent. The generic
//! walker in this module traverses the AST, dispatching `Sugar(_)` arms to
//! the appropriate [`DesugarSugar`] impl while every other variant is
//! rebuilt phase-by-phase with its children desugared recursively.
//!
//! ```text
//! File<Raw> ──┬─► (walker) ──┬─► File<Desugared>
//!             │               │
//!             ├─ MultiDecl ───┘ via DesugarSugar
//!             ├─ TableLiteral ──► (desugar to MapLiteral)
//!             └─ (future) TupleMatch ──► (desugar to If/BinOp)
//! ```
//!
//! # Adding a new customer
//!
//! 1. Add a variant to [`crate::syntax::phase::RawDeclSugar`] or
//!    [`crate::syntax::phase::RawExprSugar`].
//! 2. Implement [`DesugarSugar`] for that variant in a submodule of this
//!    module.
//! 3. Wire it into the walker's `Sugar(_)` dispatch.
//!
//! No changes to downstream consumers are needed — they only see
//! `File<Desugared>` and never observed the sugar in the first place.
//!
//! # Span fidelity
//!
//! Desugaring preserves source spans: every synthesized node carries the
//! span of the surface construct it came from. Diagnostics emitted on
//! desugared AST still point at the original source.

pub mod convert;
pub mod desugared_ast;
pub mod resolved_ast;

/// Single-node desugaring step.
///
/// Implementors map one surface sugar form to its desugared equivalent.
/// The output type is generic so different sugars can desugar to different
/// shapes — e.g., `MultiDecl` desugars to `Vec<Declaration<Desugared>>`,
/// while `TableLiteral` desugars to `ExprKind<Desugared>`.
pub trait DesugarSugar {
    /// The raw surface form being desugared.
    type RawNode;
    /// The shape produced by desugaring (often a single node, sometimes
    /// a `Vec` for one-to-many expansions).
    type DesugaredOut;

    /// Perform the desugaring.
    fn desugar_sugar(raw: Self::RawNode) -> Self::DesugaredOut;
}
