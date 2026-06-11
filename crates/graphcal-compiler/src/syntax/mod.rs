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

use std::sync::Arc;

/// Build a [`miette::NamedSource`] from `(name, source)`.
///
/// The source is shared through `Arc` so diagnostics built from the same file
/// do not re-copy it. Prefer this helper over `NamedSource::new(name, Arc::new(source))`
/// at call sites — it centralizes the wrapping convention.
#[must_use]
pub fn named_source<N: Into<String>, S: Into<Arc<String>>>(
    name: N,
    source: S,
) -> miette::NamedSource<Arc<String>> {
    miette::NamedSource::new(name.into(), source.into())
}
