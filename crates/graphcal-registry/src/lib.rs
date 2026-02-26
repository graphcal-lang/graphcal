//! Graphcal registry: shared types, error definitions, builtins, and the unit/dimension registry.
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod builtins;
pub mod declared_type;
pub mod error;
pub mod format;
pub mod manifest;
pub mod prelude;
pub mod registry;
pub mod resolve_types;
pub mod runtime_value;
pub mod time_scale;
