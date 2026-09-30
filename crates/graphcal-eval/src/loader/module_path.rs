//! Span-free module-path keys and the module targets the loader resolves
//! them to.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::syntax::ast::ModulePath;

/// Loader-resolved identities for one module path.
///
/// A path may name a file-root DAG or an inline DAG inside a loaded file.
/// Consumers need the source file to retrieve compiled artifacts and the exact
/// module target for semantic name resolution, so both identities are retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleTarget {
    source_file: DagId,
    target: DagId,
}

impl ResolvedModuleTarget {
    pub(super) const fn in_file(source_file: DagId, target: DagId) -> Self {
        Self {
            source_file,
            target,
        }
    }

    pub(super) fn file_root(source_file: DagId) -> Self {
        Self::in_file(source_file.clone(), source_file)
    }

    /// Loaded file that owns the target's compiled artifacts.
    #[must_use]
    pub const fn source_file(&self) -> &DagId {
        &self.source_file
    }

    /// Exact file-root or inline-DAG module named by the source path.
    #[must_use]
    pub const fn target(&self) -> &DagId {
        &self.target
    }
}

/// Failure to associate a loader-resolved module with its owning source file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolvedModuleTargetError {
    /// The resolved identity is neither a loaded file root nor one of its inline DAGs.
    #[error("resolved module `{target}` is not owned by a loaded source file")]
    UnknownOwner { target: DagId },
}

/// Span-free identity for an `import`/`include` path.
///
/// Used as a `HashMap` key in `LoadedFile::resolved_imports` /
/// `LoadedDag::resolved_imports` so that two equal logical paths always
/// produce equal keys without depending on a shared join format
/// (e.g. `.` vs `/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePathKey(Vec<String>);

impl ModulePathKey {
    /// Build a key from a parsed [`ModulePath`] AST node. Segment names are
    /// cloned and spans are dropped — span-aware lookup is never useful at
    /// this layer.
    #[must_use]
    pub(crate) fn from_path(path: &ModulePath) -> Self {
        Self(path.segments.iter().map(|s| s.name.to_string()).collect())
    }

    /// Segments in order, without separators.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

/// Loader-side resolution status for an import inside an inline DAG body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineBodyImportResolution {
    /// The module path resolved to an exact module and its owning source file.
    Resolved(ResolvedModuleTarget),
    /// The loader could not resolve the path in its current project context.
    ///
    /// The import declaration remains in the DAG body so the downstream
    /// resolver can emit the user-facing diagnostic with the original span.
    Unresolved,
}

impl std::fmt::Display for ModulePathKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(seg)?;
        }
        Ok(())
    }
}
