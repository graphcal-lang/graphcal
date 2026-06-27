//! Plot-family property names.

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
