//! Function namespace names.

use crate::syntax::names::{NameDef, NameNamespace};

/// Function namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FnNameNamespace {}

impl NameNamespace for FnNameNamespace {
    const DISPLAY_NAME: &'static str = "FnName";
}

/// Name of a function (e.g., `"sqrt"`, `"lerp"`).
pub type FnName = NameDef<FnNameNamespace>;

/// Function-parameter namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FnParamNameNamespace {}

impl NameNamespace for FnParamNameNamespace {
    const DISPLAY_NAME: &'static str = "FnParamName";
}

/// Name of a parameter in a function signature (e.g., `"x"`, `"window"`).
///
/// Carried for diagnostics, hover, and signature help; parameters are
/// positional at call sites.
pub type FnParamName = NameDef<FnParamNameNamespace>;
