//! Graphcal Syntax Crate
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]

pub mod ast;
pub mod dimension;
pub mod lexer;
pub mod names;
pub mod parser;
pub mod span;
pub mod token;
