// miette derive macro generates code with false-positive unused_assignments warnings
#![allow(unused_assignments)]
// KasuriError is inherently large (NamedSource + SourceSpans + diagnostic strings)
// and is only constructed on the error path, so the size is acceptable.
#![allow(clippy::result_large_err)]

pub mod builtins;
pub mod const_eval;
pub mod dag;
pub mod dim_check;
pub mod error;
pub mod eval;
pub mod eval_expr;
pub mod fn_check;
pub mod loader;
pub mod prelude;
pub mod registry;
pub mod resolve;
