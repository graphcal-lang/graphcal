//! Span-free module-path keys and the module targets the loader resolves
//! them to.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::syntax::ast::{Ident, ModulePath};
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_package::PackageName;

/// Loader-resolved identities for one module path, before the loaded project
/// places the module they name.
///
/// A path may name a file-root DAG or an inline DAG inside a loaded file. The
/// file is loaded, but nothing has checked yet that the inline DAG exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleTarget {
    source_file: DagId,
    target: DagId,
}

impl ModuleTarget {
    pub(super) const fn in_file(source_file: DagId, target: DagId) -> Self {
        Self {
            source_file,
            target,
        }
    }

    pub(super) fn file_root(source_file: DagId) -> Self {
        Self::in_file(source_file.clone(), source_file)
    }

    /// Exact file-root or inline-DAG module named by the source path.
    pub(super) const fn target(&self) -> &DagId {
        &self.target
    }

    /// Loaded file that owns the target.
    #[cfg(test)]
    pub(super) const fn source_file(&self) -> &DagId {
        &self.source_file
    }

    /// This target, naming the loaded module at `module`.
    pub(super) fn placed_at(self, module: LoadedModuleId) -> ResolvedModuleTarget {
        ResolvedModuleTarget {
            source_file: self.source_file,
            target: self.target,
            module,
        }
    }
}

/// The position of one loaded module (a file root or one of its inline DAGs)
/// in the [`LoadedFiles`](super::loaded_project::LoadedFiles) that placed it.
///
/// Issued only by that placement, so a lookup by id in the same project is
/// total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoadedModuleId {
    /// Position of the owning file, in dependency order.
    pub(super) file: usize,
    /// Position of the inline DAG among the file's inline DAGs (source
    /// preorder), or `None` for the file root.
    pub(super) inline: Option<usize>,
}

/// Loader-resolved identities for one module path, naming a loaded module.
///
/// A path may name a file-root DAG or an inline DAG inside a loaded file.
/// Consumers need the source file to retrieve compiled artifacts and the exact
/// module target for semantic name resolution, so both identities are retained,
/// together with the loaded module they name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleTarget {
    source_file: DagId,
    target: DagId,
    module: LoadedModuleId,
}

impl ResolvedModuleTarget {
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

    /// The loaded module named by the source path.
    #[must_use]
    pub(crate) const fn module(&self) -> LoadedModuleId {
        self.module
    }
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
            assert_eq!(
                PackageSelector::classify(path).unwrap().module_segments(),
                []
            );
        });
    }
}
