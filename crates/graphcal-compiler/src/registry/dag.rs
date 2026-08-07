use std::collections::HashMap;

use crate::desugar::desugared_ast::DagDecl;
use crate::syntax::decl_name::DeclName;

/// Registry of `dag` declaration bodies accessible by name within a file.
///
/// Populated while assembling frontend registries so nested DAG templates can
/// inherit their parent's syntax-time declarations. It is discarded when the
/// registry crosses into HIR; checked DAG calls use canonical `DagTIR` bodies.
#[derive(Debug, Default, Clone)]
pub struct DagRegistry {
    /// Dag bodies keyed by their declaration name. Dags live in the
    /// declaration namespace, so the key is the typed [`DeclName`] like
    /// every other registry — not a bare `String`.
    pub(crate) dags: HashMap<DeclName, DagDecl>,
}
