//! Graphcal Syntax: lexer, parser, and AST definitions.

pub mod ast;
pub mod attribute;
pub mod comments;
pub mod desugar;
pub mod dimension;
pub mod lexer;
pub mod module_resolve;
pub mod names;
pub mod nat;
pub mod non_empty;
pub mod parser;
pub mod phase;
pub mod span;
pub mod token;
pub mod visitor;
