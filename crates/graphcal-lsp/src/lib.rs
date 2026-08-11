//! Graphcal Language Server Protocol implementation.
#![allow(
    clippy::disallowed_methods,
    reason = "the LSP is an imperative shell where missing editor metadata deliberately degrades to an empty response"
)]

mod code_actions;
mod completion;
mod convert;
mod cursor_context;
mod diagnostics;
mod document_links;
mod document_symbols;
mod formatting;
mod goto_definition;
mod hover;
mod inlay_hints;
mod nominal_type_index;
mod project_symbols;
mod references;
mod rename;
mod resolve;
pub mod server;
mod signature_help;
mod symbol_identity;
mod symbol_table;

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    server::run().await;
}
