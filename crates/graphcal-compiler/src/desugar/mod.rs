//! Generic desugaring pipeline.
//!
//! Transforms `File<Raw>` (parser output, with all surface sugar) into
//! `File<Desugared>` (canonical form consumed by name resolution, IR lowering,
//! TIR, and evaluation).
//!
//! # Architecture
//!
//! The concrete walker in [`convert`] traverses the AST, dispatching
//! `Sugar(_)` arms to explicit conversion functions while every other
//! variant is rebuilt phase-by-phase with its children desugared
//! recursively.
//!
//! ```text
//! File<Raw> ──┬─► convert ──┬─► File<Desugared>
//!             │             │
//!             ├─ MultiDecl ─┘ (one-to-many declarations)
//!             └─ TableLiteral ─► MapLiteral
//! ```
//!
//! # Adding new sugar
//!
//! 1. Add a variant to [`crate::syntax::ast::RawDeclSugar`] or
//!    [`crate::syntax::ast::RawExprSugar`].
//! 2. Add the corresponding conversion helper in [`convert`].
//! 3. Wire it into the walker's `Sugar(_)` dispatch.
//!
//! No changes to downstream consumers are needed — they only see
//! `File<Desugared>` and never observe the sugar in the first place.
//!
//! # Span fidelity
//!
//! Desugaring preserves source spans: every synthesized node carries the
//! span of the surface construct it came from. Diagnostics emitted on
//! desugared AST still point at the original source.

pub mod convert;
pub mod desugared_ast;
