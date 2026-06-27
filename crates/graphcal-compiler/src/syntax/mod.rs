//! Graphcal Syntax: lexer, parser, and AST definitions.

pub mod ast;
pub mod attribute;
pub mod comments;
pub mod decl_name;
pub mod desugar;
pub mod dimension;
pub mod function_name;
pub mod index_name;
pub mod lexer;
pub mod local_name;
pub mod module_name;
pub mod module_resolve;
pub mod names;
pub mod nat;
pub mod non_empty;
pub mod parser;
pub mod phase;
pub mod span;
pub mod token;
pub mod type_name;
pub mod visitor;
