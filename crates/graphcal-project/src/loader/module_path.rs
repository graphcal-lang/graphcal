//! Span-free module-path keys and the module targets the loader resolves
//! them to.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::syntax::ast::{Ident, ModulePath};
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_package::PackageName;

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

/// A reserved standard-library namespace (Concept §6.2): a module path whose
/// first segment is one of these names never resolves to a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReservedNamespace {
    /// `graphcal.…`
    Graphcal,
    /// `std.…`
    Std,
}

impl ReservedNamespace {
    /// The reserved namespace a first module-path segment names, if any. This
    /// is the only place the reserved spellings are recognized.
    pub(super) fn parse(segment: &NameAtom) -> Option<Self> {
        match segment.as_str() {
            "graphcal" => Some(Self::Graphcal),
            "std" => Some(Self::Std),
            _ => None,
        }
    }
}

/// A module path whose first segment selects a package, split into that
/// selector and the module segments that walk the selected package.
#[derive(Debug, Clone, Copy)]
pub(super) struct PackageSelector<'p> {
    path: &'p ModulePath,
}

impl<'p> PackageSelector<'p> {
    /// Classify the first segment of `path` once: a reserved namespace, or a
    /// package selector.
    ///
    /// # Errors
    ///
    /// Returns the [`ReservedNamespace`] the first segment names.
    pub(super) fn classify(path: &'p ModulePath) -> Result<Self, ReservedNamespace> {
        ReservedNamespace::parse(path.segments.first().name.atom()).map_or(Ok(Self { path }), Err)
    }

    /// The selector: the module path's first segment.
    pub(super) fn name(self) -> &'p NameAtom {
        self.path.segments.first().name.atom()
    }

    /// Whether the selector names `package` by its real package name.
    pub(super) fn names(self, package: &PackageName) -> bool {
        self.name().as_str() == package.as_str()
    }

    /// Segments after the selector, which walk the selected package's module
    /// namespace.
    pub(super) fn module_segments(self) -> &'p [Ident] {
        self.path.segments.split_first().1
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::syntax::ast::DeclKind;
    use graphcal_compiler::syntax::parser::Parser;

    use super::*;

    fn with_import_path(source: &str, check: impl FnOnce(&ModulePath)) {
        let file = Parser::new(source).parse_file().unwrap();
        let DeclKind::Import(import) = &file.declarations[0].kind else {
            panic!("expected an import");
        };
        check(import.path());
    }

    #[test]
    fn reserved_first_segments_classify_as_reserved_namespaces() {
        with_import_path("import std.math::{x};", |path| {
            assert_eq!(
                PackageSelector::classify(path).err(),
                Some(ReservedNamespace::Std)
            );
        });
        with_import_path("import graphcal::{x};", |path| {
            assert_eq!(
                PackageSelector::classify(path).err(),
                Some(ReservedNamespace::Graphcal)
            );
        });
    }

    #[test]
    fn package_selector_splits_the_first_segment_from_the_module_segments() {
        with_import_path("import pkg.lib.inner::{x};", |path| {
            let selector = PackageSelector::classify(path).unwrap();
            assert_eq!(selector.name().as_str(), "pkg");
            assert!(selector.names(&PackageName::new("pkg").unwrap()));
            assert!(!selector.names(&PackageName::new("other").unwrap()));
            let segments = selector
                .module_segments()
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(segments, ["lib", "inner"]);
        });
        with_import_path("import pkg::{x};", |path| {
            assert!(
                PackageSelector::classify(path)
                    .unwrap()
                    .module_segments()
                    .is_empty()
            );
        });
    }
}
