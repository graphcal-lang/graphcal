//! Names in the top-level declaration namespace.

use crate::syntax::names::{NameDef, NameNamespace, ResolvedName};

/// Const/param/node/assert/plot-family declaration namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeclNameNamespace {}

impl NameNamespace for DeclNameNamespace {
    const DISPLAY_NAME: &'static str = "DeclName";
}

/// Name of a const, param, node, assert, plot, figure, layer, or DAG declaration
/// (e.g., `"G0"`, `"dry_mass"`, `"dv_total"`).
pub type DeclName = NameDef<DeclNameNamespace>;

/// Module-resolved declaration name.
pub type ResolvedDeclName = ResolvedName<DeclNameNamespace>;
