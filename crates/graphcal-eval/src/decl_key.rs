use graphcal_compiler::syntax::names::{DeclName, ResolvedName, ScopedName, namespace};
use graphcal_compiler::tir::typed::DagTIR;

/// Runtime key for a value declaration during evaluation.
///
/// Runtime maps use canonical `ResolvedName<Decl>` identities so same-leaf
/// declarations from different modules/DAGs cannot collide. Standalone TIRs
/// synthesize those identities from the DAG owner plus the declaration leaf.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeDeclKey {
    Resolved(ResolvedName<namespace::Decl>),
}

impl RuntimeDeclKey {
    #[must_use]
    pub(crate) const fn resolved(name: ResolvedName<namespace::Decl>) -> Self {
        Self::Resolved(name)
    }

    fn local_or_leaf(dag: &DagTIR, name: &ScopedName) -> Self {
        dag.resolved_decl_key_for_local(name).map_or_else(
            || {
                Self::Resolved(ResolvedName::from_def(
                    dag.dag_id.clone(),
                    DeclName::new(name.member()),
                ))
            },
            Self::Resolved,
        )
    }

    /// Build the key for a declaration owned by `dag`.
    #[must_use]
    pub(crate) fn for_local_decl(dag: &DagTIR, name: &ScopedName) -> Self {
        Self::local_or_leaf(dag, name)
    }

    /// Build the key for a visible declaration name in `dag`.
    ///
    /// Imported/selective names are resolved through the DAG semantic binding
    /// map; otherwise the DAG owner plus leaf name provides the standalone
    /// identity.
    #[must_use]
    pub(crate) fn for_visible_name(dag: &DagTIR, name: &ScopedName) -> Self {
        if let Some(resolved) = dag.semantic.decl_bindings.get(name) {
            return Self::Resolved(resolved.clone());
        }
        Self::local_or_leaf(dag, name)
    }

    #[must_use]
    pub(crate) const fn as_resolved(&self) -> &ResolvedName<namespace::Decl> {
        match self {
            Self::Resolved(name) => name,
        }
    }

    #[must_use]
    pub(crate) fn member(&self) -> &str {
        match self {
            Self::Resolved(name) => name.as_str(),
        }
    }
}

impl From<ResolvedName<namespace::Decl>> for RuntimeDeclKey {
    fn from(name: ResolvedName<namespace::Decl>) -> Self {
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
