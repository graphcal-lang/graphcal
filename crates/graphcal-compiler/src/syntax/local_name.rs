//! Local expression-binding names.

use crate::syntax::names::{NameDef, NameNamespace};

/// Local expression-binding namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocalNameNamespace {}

impl NameNamespace for LocalNameNamespace {
    const DISPLAY_NAME: &'static str = "LocalName";
}

/// Name of a local expression binding (e.g., `"x"`, `"stage_mass"`).
pub type LocalName = NameDef<LocalNameNamespace>;
