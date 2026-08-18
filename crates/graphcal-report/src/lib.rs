//! Rendering of evaluated Graphcal projects into shareable documents.
//!
//! This crate is the functional core for plot and report rendering: pure
//! projections from evaluated specs ([`graphcal_eval::eval::PlotSpec`] and
//! friends) to Vega-Lite JSON ([`vega`]) and self-contained HTML pages
//! ([`plot_page`]). All file, process, and network I/O stays in the consumers
//! (CLI, wasm adapter).

pub(crate) mod escape;
pub mod plot_page;
pub mod report_html;
pub mod report_ir;
pub mod report_markdown;
pub mod value_display;
pub mod vega;
pub(crate) mod vega_assets;
