// miette derive macro generates code with false-positive unused_assignments warnings
#![allow(unused_assignments)]

pub mod builtins;
pub mod const_eval;
pub mod dag;
pub mod dim_check;
pub mod error;
pub mod eval;
pub mod eval_expr;
pub mod prelude;
pub mod registry;
pub mod resolve;
