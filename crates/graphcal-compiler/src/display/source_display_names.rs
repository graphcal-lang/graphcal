//! The readable names a project supplies for runtime identities its source
//! cannot spell from the evaluated root.

use crate::display::include_scope_names::IncludeScopeNames;
use crate::display::module_paths::ModulePaths;

/// Readable names for the runtime identities an evaluation reports that the
/// root's source cannot spell: the private scopes of its anonymous includes,
/// and the modules outside its subtree that it invokes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceDisplayNames {
    include_scopes: IncludeScopeNames,
    module_paths: ModulePaths,
}

impl SourceDisplayNames {
    /// Names from the root's anonymous include scopes and the project's
    /// module paths.
    #[must_use]
    pub const fn new(include_scopes: IncludeScopeNames, module_paths: ModulePaths) -> Self {
        Self {
            include_scopes,
            module_paths,
        }
    }

    /// Readable names of the root's anonymous include scopes.
    #[must_use]
    pub const fn include_scopes(&self) -> &IncludeScopeNames {
        &self.include_scopes
    }

    /// Source module paths of the project's loaded files.
    #[must_use]
    pub const fn module_paths(&self) -> &ModulePaths {
        &self.module_paths
    }
}
