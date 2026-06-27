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
