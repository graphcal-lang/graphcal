//! Graphcal Syntax: lexer, parser, and AST definitions.

pub mod ast;
pub mod comments;
pub mod dag_id;
pub mod dimension;
pub mod lexer;
pub mod name_resolve;
pub mod names;
pub mod parser;
pub mod span;
pub mod token;
pub mod visitor;
