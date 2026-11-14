//! LSP server: the service assembly and the stdio entry point.
//!
//! - `backend` holds the server state and the operations handlers share;
//! - `handlers` implements the protocol handlers;
//! - `analysis_worker` runs, stores, and publishes one analysis pass;
//! - `scheduler`, `revisions`, and `documents` hold the scheduling,
//!   revision, and open-document state the backend coordinates;
//! - `client_shape` adapts results to the client's negotiated shapes;
//! - `custom_protocol` declares Graphcal-specific requests and notifications.

mod analysis_worker;
mod backend;
mod client_shape;
mod custom_protocol;
mod documents;
mod handlers;
mod revisions;
mod scheduler;
#[cfg(test)]
mod tests;

pub(crate) use backend::Backend;
#[cfg(test)]
pub(crate) use custom_protocol::BlockingFeatureRequest;
use custom_protocol::ViewModelRequest;

use tower_lsp::{ClientSocket, LspService, Server};

pub(crate) fn service() -> (LspService<Backend>, ClientSocket) {
    let builder = LspService::build(Backend::new)
        .custom_method(ViewModelRequest::METHOD, Backend::view_model);
    #[cfg(test)]
    let builder = builder.custom_method(BlockingFeatureRequest::METHOD, Backend::blocking_feature);
    builder.finish()
}

/// Start the LSP server, reading from stdin and writing to stdout.
pub(crate) async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = service();
    Server::new(stdin, stdout, socket).serve(service).await;
}
