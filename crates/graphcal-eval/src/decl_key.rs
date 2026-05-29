use graphcal_compiler::syntax::names::{DeclName, ResolvedName, ScopedName, namespace};
use graphcal_compiler::tir::typed::DagTIR;

/// Runtime key for a value declaration during evaluation.
///
/// Module-aware TIRs use canonical `ResolvedName<Decl>` identities so same-leaf
/// declarations from different modules/DAGs cannot collide. Standalone or
/// compatibility TIRs keep the legacy `ScopedName` key at the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeDeclKey {
    Resolved(ResolvedName<namespace::Decl>),
    Legacy(ScopedName),
}

impl RuntimeDeclKey {
    #[must_use]
    pub(crate) const fn resolved(name: ResolvedName<namespace::Decl>) -> Self {
        Self::Resolved(name)
    }

    #[must_use]
    pub(crate) const fn legacy(name: ScopedName) -> Self {
        Self::Legacy(name)
    }

    /// Build the key for a declaration owned by `dag`.
    ///
    /// Only DAGs that carry HIR-derived dependency sidecars opt into resolved
    /// routing; otherwise the legacy standalone key is preserved.
    #[must_use]
    pub(crate) fn for_local_decl(dag: &DagTIR, name: &ScopedName) -> Self {
        if dag.resolved_deps.is_some()
            && let Some(key) = dag.resolved_decl_key_for_local(name)
        {
            return Self::Resolved(key);
        }
        Self::Legacy(name.clone())
    }

    /// Build the key for a visible declaration name in `dag`.
    ///
    /// Imported/selective names are resolved through `DagTIR::resolved_decl_bindings`
    /// when the DAG is using canonical declaration routing. Legacy callers keep
    /// the original scoped key.
    #[must_use]
    pub(crate) fn for_visible_name(dag: &DagTIR, name: &ScopedName) -> Self {
        if dag.resolved_deps.is_some() {
            if let Some(resolved) = dag
                .resolved_decl_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(name))
            {
                return Self::Resolved(resolved.clone());
            }
            if let Some(key) = dag.resolved_decl_key_for_local(name) {
                return Self::Resolved(key);
            }
        }
        Self::Legacy(name.clone())
    }

    #[must_use]
    pub(crate) const fn as_resolved(&self) -> Option<&ResolvedName<namespace::Decl>> {
        match self {
            Self::Resolved(name) => Some(name),
            Self::Legacy(_) => None,
        }
    }

    #[must_use]
    pub(crate) fn member(&self) -> &str {
        match self {
            Self::Resolved(name) => name.as_str(),
            Self::Legacy(name) => name.member(),
        }
    }

    #[must_use]
    pub(crate) fn to_decl_name(&self) -> DeclName {
        match self {
            Self::Resolved(name) => DeclName::from_atom(name.atom().clone()),
            Self::Legacy(name) => DeclName::new(name.member()),
        }
    }
}

impl From<ResolvedName<namespace::Decl>> for RuntimeDeclKey {
    fn from(name: ResolvedName<namespace::Decl>) -> Self {
        Self::Resolved(name)
    }
}

impl From<ScopedName> for RuntimeDeclKey {
    fn from(name: ScopedName) -> Self {
        Self::Legacy(name)
    }
}

impl std::fmt::Display for RuntimeDeclKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolved(name) => name.fmt(f),
            Self::Legacy(name) => name.fmt(f),
        }
    }
}
