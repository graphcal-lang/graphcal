//! Graphcal registry: shared types, error definitions, builtins, and the unit/dimension registry.

pub mod builtins;
pub mod declared_type;
pub mod error;
pub mod format;
pub mod manifest;
pub mod prelude;
#[expect(
    clippy::module_inception,
    reason = "preserved from pre-merge crate structure"
)]
pub mod registry;
pub mod resolve_types;
pub mod runtime_value;
pub mod time_scale;
