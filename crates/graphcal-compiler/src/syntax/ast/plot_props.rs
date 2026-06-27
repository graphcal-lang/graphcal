//! Syntax-level plot property names.
//!
//! Semantic property registries live in [`crate::plot_props`]. This module
//! only owns the AST-facing name newtype used by parsed plot, mark, figure,
//! and layer fields before validation resolves them to typed semantic
//! property enums.

use crate::syntax::names::{NameDef, NameNamespace};

/// Plot/figure/layer property namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlotPropertyNameNamespace {}

impl NameNamespace for PlotPropertyNameNamespace {
    const DISPLAY_NAME: &'static str = "PlotPropertyName";
}

/// Name of an open plot/figure/layer property (e.g., `"title"`, `"width"`,
/// `"stroke_width"`).
pub type PlotPropertyName = NameDef<PlotPropertyNameNamespace>;
