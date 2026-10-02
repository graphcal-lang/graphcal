//! Semantic core values of the type system.
//!
//! Checked types, index definitions and the concrete axes, keys, and nominal
//! values built from them, unit scales, base-dimension metadata, time scales
//! and zones, the scalar built-in catalog, and the Graphcal prelude catalog.

pub mod aliased_table;
pub mod applied_constructor;
pub mod checked_type;
pub mod dimension_table;
pub mod index_axis;
pub mod index_def;
pub mod key_value;
pub mod prelude;
pub mod scalar_function;
pub mod struct_value;
pub mod time_scale;
pub mod time_zone;
pub mod unit_scale;
