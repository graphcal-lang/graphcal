//! View model — the presentation projection of one checked file for the
//! Graphcal view screen (the spreadsheet-like rendering of a module).
//!
//! Like [`crate::graph_ir`], this is a one-way projection: TIR (plus an
//! optional evaluation) → view model → JSON. The view model is the wire
//! boundary shared by the CLI, the language server, and the browser engine,
//! so its fields are already rendered strings; every typed lookup happens in
//! `project.rs` before the model is built.
//!
//! A root file has one view per target (`target.rs`): the root evaluated
//! alone, and each include instance its evaluation materializes, presented
//! as its template with the values the include's arguments give it.
//!
//! The structure (nodes, axes, dependencies, source positions) comes from the
//! checked program and is always complete. Values are attached per node from
//! an evaluation and are absent for a library file, an unevaluated project,
//! or a node whose evaluation failed.

mod cells;
mod model;
mod project;
mod target;
#[cfg(test)]
mod tests;
mod value;
mod wire_key;

pub use model::{
    ViewCall, ViewCell, ViewElement, ViewEntry, ViewIndex, ViewIndexEntry, ViewInstance, ViewModel,
    ViewModule, ViewNode, ViewNodeKind, ViewOrigin, ViewPosition, ViewRange, ViewSource,
    ViewSubject, ViewValue,
};
pub use project::{
    ViewProjection, ViewProjectionError, ViewProjections, project_view, project_views,
};
pub use target::{ViewModels, ViewTarget};
pub use wire_key::{ViewIndexId, ViewInstanceId, ViewNodeId};
