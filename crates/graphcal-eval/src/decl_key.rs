use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::tir::typed::{DagTIR, DiagnosticDeclProbe};

/// Runtime key for a value declaration during evaluation.
///
/// Runtime maps use canonical `ResolvedName<Decl>` identities so same-leaf
/// declarations from different modules/DAGs cannot collide. Unknown names
/// cannot enter runtime maps through fabricated owner-plus-leaf identities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeDeclKey {
    Resolved(graphcal_compiler::syntax::decl_name::ResolvedDeclName),
}

impl RuntimeDeclKey {
    #[must_use]
    pub(crate) const fn resolved(
        name: graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    ) -> Self {
        Self::Resolved(name)
    }

    /// Build the key for a declaration authoritatively bound in `dag`.
    ///
    /// # Errors
    ///
    /// Returns a typed diagnostic probe when `name` has no canonical binding;
    /// evaluation must not fabricate an identity for that unknown name.
    pub(crate) fn for_local_decl(
        dag: &DagTIR,
        name: &ScopedName,
    ) -> Result<Self, DiagnosticDeclProbe> {
        dag.lookup_decl_identity(name)
            .into_bound()
            .map(Self::Resolved)
    }

    #[must_use]
    pub(crate) const fn as_resolved(
        &self,
    ) -> &graphcal_compiler::syntax::decl_name::ResolvedDeclName {
        match self {
            Self::Resolved(name) => name,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn member(&self) -> &str {
        match self {
            Self::Resolved(name) => name.as_str(),
        }
    }
}

impl From<graphcal_compiler::syntax::decl_name::ResolvedDeclName> for RuntimeDeclKey {
    fn from(name: graphcal_compiler::syntax::decl_name::ResolvedDeclName) -> Self {
        Self::Resolved(name)
    }
}

impl std::fmt::Display for RuntimeDeclKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolved(name) => name.fmt(f),
        }
    }
}
