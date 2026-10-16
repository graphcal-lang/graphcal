//! View model — the presentation projection of one checked file for the
//! Graphcal view screen (the spreadsheet-like rendering of a module).
//!
//! Like [`crate::graph_ir`], this is a one-way projection: TIR (plus an
//! optional evaluation) → view model → JSON. The view model is the wire
//! boundary shared by the CLI, the language server, and the browser engine,
//! so its fields are already rendered strings; every typed lookup happens in
//! `project.rs` before the model is built.
//!
//! The structure (nodes, axes, dependencies, source positions) comes from the
//! checked program and is always complete. Values are attached per node from
//! an evaluation and are absent for a library file, an unevaluated project,
//! or a node whose evaluation failed.

mod cells;
mod model;
mod project;
#[cfg(test)]
mod tests;
mod value;
mod wire_key;

pub use model::{
    ViewCell, ViewElement, ViewEntry, ViewIndex, ViewIndexEntry, ViewInstance, ViewModel,
    ViewModule, ViewNode, ViewNodeKind, ViewOrigin, ViewPosition, ViewRange, ViewReduce,
    ViewSource, ViewValue,
};
pub use project::{ViewProjection, ViewProjectionError, project_view};
pub use wire_key::{ViewIndexId, ViewNodeId};
