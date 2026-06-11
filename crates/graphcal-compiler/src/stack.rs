//! Stack-growth helper for recursive walkers over user expressions.
//!
//! The compiler pipeline (desugar, HIR lowering, dependency
//! collection, dimension inference), the evaluators, the formatter, and the
//! LSP symbol walker all recurse once per expression-tree level. Structural
//! *nesting* is bounded by the parser
//! ([`crate::syntax::parser::MAX_NESTING_DEPTH`]), but left-nested operator
//! *chains* (`1.0 + 1.0 + …`) are parsed iteratively and produce trees whose
//! depth equals the chain length — a few hundred terms is realistic in
//! engineering files and used to overflow the stack in debug builds.
//!
//! Every recursive walker calls [`with_stack_growth`] once per recursion
//! level so the stack grows on demand instead of aborting the process.

/// Red zone: grow when less than this much stack remains.
const RED_ZONE: usize = 64 * 1024;

/// Size of each new stack segment allocated by [`stacker::maybe_grow`].
const STACK_PER_GROWTH: usize = 1024 * 1024;

/// Run `f`, growing the stack on demand when the remaining stack is low.
///
/// Call this at the entry of every function that recurses once per
/// expression-tree level. The check is a cheap stack-pointer comparison in
/// the common (no-growth) case.
pub fn with_stack_growth<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(RED_ZONE, STACK_PER_GROWTH, f)
}
