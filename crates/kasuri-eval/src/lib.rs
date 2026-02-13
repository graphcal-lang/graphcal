// miette derive macro generates code with false-positive unused_assignments warnings
#![allow(unused_assignments)]
// KasuriError is inherently large (NamedSource + SourceSpans + diagnostic strings)
// and is only constructed on the error path, so the size is acceptable.
#![allow(clippy::result_large_err)]

pub(crate) mod builtins;
pub(crate) mod const_eval;
pub(crate) mod dag;
pub(crate) mod dim_check;
pub mod error;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod fn_check;
pub mod loader;
pub(crate) mod prelude;
pub(crate) mod registry;
pub(crate) mod resolve;
