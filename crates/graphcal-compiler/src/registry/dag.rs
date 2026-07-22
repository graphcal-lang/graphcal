use std::collections::HashMap;

use crate::desugar::desugared_ast::DagDecl;
use crate::syntax::decl_name::DeclName;

/// Registry of `dag` declaration bodies accessible by name within a file.
///
/// Populated at IR lowering time with the raw AST body for each declared `dag`.
/// Used during dim-checking (and later, evaluation) to resolve inline DAG
/// invocations `@dag(args).out` against the called `dag`'s `pub param` and
/// `pub node` signatures.
#[derive(Debug, Default, Clone)]
pub struct DagRegistry {
    /// Dag bodies keyed by their declaration name. Dags live in the
    /// declaration namespace, so the key is the typed [`DeclName`] like
    /// every other registry — not a bare `String`.
    pub(crate) dags: HashMap<DeclName, DagDecl>,
}

impl DagRegistry {
    /// Return the AST body of the named `dag`, if one is declared in this file.
    #[must_use]
    pub(crate) fn get(&self, name: &str) -> Option<&DagDecl> {
        self.dags.get(name)
    }

    /// Iterate over all registered dags.
    #[cfg(test)]
    pub(crate) fn all_dags(&self) -> impl Iterator<Item = (&DeclName, &DagDecl)> {
        self.dags.iter()
    }
}
