//! Graphcal Language Server Protocol implementation.

mod convert;
mod diagnostics;
mod document_symbols;
mod goto_definition;
mod hover;
mod references;
pub mod server;
mod symbol_table;

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    server::run().await;
}
