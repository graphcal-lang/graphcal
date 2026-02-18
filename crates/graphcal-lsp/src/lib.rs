//! Graphcal Language Server Protocol implementation.

mod completion;
mod convert;
mod cursor_context;
mod diagnostics;
mod document_symbols;
mod goto_definition;
mod hover;
mod inlay_hints;
mod references;
pub mod server;
mod signature_help;
mod symbol_table;

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    server::run().await;
}
