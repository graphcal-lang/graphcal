//! A typed client for Graphcal batch model evaluation.
//!
//! The client runs `graphcal model describe` once, hands out typed handles to
//! the model's ports, and starts one `graphcal model serve` process per
//! session. Arrow IPC stays internal: requests are built from Rust values and
//! results are read through the same handles.
//!
//! ```no_run
//! use graphcal_model_client::{Bool, GraphcalCommand, Key, Model, Quantity};
//! use graphcal_model_protocol::RowOutcome;
//!
//! let model = Model::describe(GraphcalCommand::new("reliability.gcl"))?;
//! let load = model.input::<Quantity>("load")?;
//! let mode = model.input::<Key>("mode")?;
//! let failure = model.output::<Bool>("failure")?;
//!
//! let mut session = model.session().vary(&load).vary(&mode).output(&failure).spawn()?;
//! let degraded = mode.variant("Degraded")?;
//! let request = session
//!     .batch()
//!     .column(&load, [2_000.0, 9_000.0])?
//!     .column(&mode, [degraded.clone(), degraded])?
//!     .build()?;
//! let result = session.evaluate(&request)?;
//! for outcome in result.outcomes() {
//!     match outcome {
//!         RowOutcome::Success(row) => println!("failure: {}", row.get(&failure)?),
//!         RowOutcome::Failure(failure) => println!("failed: {}", failure.message()),
//!     }
//! }
//! session.shutdown()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod batch;
mod command;
mod error;
mod evaluation;
mod identity;
mod kind;
mod model;
mod session;

pub use batch::{BatchBuilder, Request};
pub use command::GraphcalCommand;
pub use error::ClientError;
pub use evaluation::{Evaluation, Row};
#[doc(hidden)]
pub use identity::PortIdentity;
pub use kind::{
    Algebraic, Bool, Complex, Datetime, Indexed, IndexedDetail, Int, Key, KeyVariant, Kind,
    Quantity, QuantityLike,
};
pub use model::{InputHandle, Model, OutputHandle};
pub use session::{Session, SessionBuilder};

#[cfg(test)]
mod tests;
