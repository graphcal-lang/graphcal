//! Failures of the project loader: reading, locating, and resolving source
//! files and manifests before any module is compiled.
//!
//! The loader is the shell that owns file names and source text, so its
//! located diagnostics carry the [`RegisteredSource`] they point into.

use std::path::PathBuf;

use graphcal_compiler::source_registry::RegisteredSource;
use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

use graphcal_compiler::import_cycle::ImportCycle;
use graphcal_compiler::syntax::module_path_key::ModulePathKey;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_package::PackageName;

/// Why a project could not be loaded.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum LoadError {
    #[error("file not found: {}", path.display())]
    #[diagnostic(code(graphcal::M000), help("check that the file path is correct"))]
    FileNotFound { path: PathBuf },

    /// `reason` is host text (an OS path or overlay-capability failure).
    #[error("invalid source path `{}`: {reason}", path.display())]
    #[diagnostic(
        code(graphcal::M023),
        help("Graphcal source files must be UTF-8 `.gcl` files")
    )]
    InvalidSourcePath { path: PathBuf, reason: String },

    #[error("circular import detected: {cycle}")]
    #[diagnostic(
        code(graphcal::M001),
        help("files cannot import each other in a cycle")
    )]
    CircularImport { cycle: ImportCycle },

    /// `message` is host text (TOML, lockfile, and filesystem failures).
    #[error("failed to parse graphcal.toml: {message}")]
    #[diagnostic(code(graphcal::M015))]
    ManifestError { message: String },

    #[error("imported file not found: {path}")]
    #[diagnostic(code(graphcal::M002))]
    ImportFileNotFound {
        path: ModulePathKey,
        #[source_code]
        src: RegisteredSource,
        #[label("referenced here")]
        span: SourceSpan,
    },

    #[error("import path `{path}` resolves outside the project root")]
    #[diagnostic(
        code(graphcal::M008),
        help(
            "imports must reference files within the project directory tree; place a `graphcal.toml` in an ancestor directory to widen the project root"
        )
    )]
    ImportOutsideRoot {
        path: ModulePathKey,
        #[source_code]
        src: RegisteredSource,
        #[label("resolves outside project root")]
        span: SourceSpan,
    },

    #[error("module path starts with `{path_first}` but package name is `{package_name}`")]
    #[diagnostic(
        code(graphcal::M013),
        help("module paths must start with the package name from graphcal.toml")
    )]
    PackageNameMismatch {
        path_first: NameAtom,
        package_name: PackageName,
        #[source_code]
        src: RegisteredSource,
        #[label("should start with `{package_name}`")]
        span: SourceSpan,
    },

    #[error("standard library modules are not yet implemented")]
    #[diagnostic(
        code(graphcal::M014),
        help(
            "the graphcal standard library (graphcal/math, etc.) will be available in a future release"
        )
    )]
    StdlibNotImplemented {
        path: ModulePathKey,
        #[source_code]
        src: RegisteredSource,
        #[label("stdlib not yet available")]
        span: SourceSpan,
    },

    #[error("cross-file import `{path}` from a file outside any package")]
    #[diagnostic(
        code(graphcal::M017),
        help(
            "a Graphcal file is either part of a real package (lives at `<source_dir>/<package>.gcl` or under `<source_dir>/<package>/`) or a standalone virtual-package script. Standalone files may only reference their own top-level decls (via `import <file_stem>::{{...}};`) or their own inline DAGs. To pull symbols from a sibling file, add a `graphcal.toml` and place this file inside the package's namespace directory."
        )
    )]
    CrossFileImportInVirtualPackage {
        path: ModulePathKey,
        #[source_code]
        src: RegisteredSource,
        #[label("not reachable from a virtual-package file")]
        span: SourceSpan,
    },

    #[error("a file-root `import` cannot target its own file `{path}`")]
    #[diagnostic(
        code(graphcal::M033),
        help(
            "top-level declarations are already in scope; self-imports are only meaningful inside isolated inline DAG bodies"
        )
    )]
    FileRootSelfImport {
        path: ModulePathKey,
        #[source_code]
        src: RegisteredSource,
        #[label("this import resolves to its own file root")]
        span: SourceSpan,
    },

    #[error("`{stem}` in a DAG body of a package file does not name this file")]
    #[diagnostic(code(graphcal::M037))]
    PackageFileStemInDagBody {
        /// The one-segment path, which is the file's own stem.
        stem: NameAtom,
        /// How to name this file, or the module `stem` names.
        #[help]
        help: String,
        #[source_code]
        src: RegisteredSource,
        #[label("spelled like this file's stem")]
        span: SourceSpan,
    },

    #[error("unknown dependency `{name}` in package `{package}`")]
    #[diagnostic(
        code(graphcal::M035),
        help(
            "module paths must start with the package name or a dependency declared in graphcal.toml; run `graphcal deps lock` after changing dependencies"
        )
    )]
    UnknownDependency {
        /// The module path's first segment.
        name: NameAtom,
        /// The importing package.
        package: PackageName,
        #[source_code]
        src: RegisteredSource,
        #[label("not a dependency of `{package}`")]
        span: SourceSpan,
    },

    /// An existing source file that could not be read. `reason` is host text
    /// (an OS error or the exhausted loader budget).
    #[error("could not read source file `{}`: {reason}", path.display())]
    #[diagnostic(
        code(graphcal::M039),
        help(
            "Graphcal source files must be readable regular `.gcl` files within the loader's resource limits"
        )
    )]
    SourceReadFailed { path: PathBuf, reason: String },

    /// A package source file whose path names no module. `reason` says which
    /// directory name or file stem is not a module path segment.
    #[error("invalid module file name `{}`: {reason}", path.display())]
    #[diagnostic(
        code(graphcal::M040),
        help(
            "every directory and `.gcl` file stem below the source directory is a module path segment, so it must be a lower_snake_case Graphcal identifier that is not a keyword; rename it (for example `my-helpers.gcl` to `my_helpers.gcl`)"
        )
    )]
    InvalidModuleFileName { path: PathBuf, reason: String },

    /// A module file reached under a spelling that differs from its directory
    /// entry, which a case-insensitive filesystem would otherwise accept.
    #[error("{}", spelling_mismatch_message(path, on_disk.as_deref()))]
    #[diagnostic(
        code(graphcal::M041),
        help(
            "module paths must match file and directory names exactly, including letter case, so a project loads the same way on every filesystem; correct the spelling or rename the file"
        )
    )]
    ModuleFileSpellingMismatch {
        /// The path as requested, up to the first mismatched component.
        path: PathBuf,
        /// The directory entry that differs only in letter case, if any.
        on_disk: Option<String>,
    },
}

fn spelling_mismatch_message(path: &std::path::Path, on_disk: Option<&str>) -> String {
    on_disk.map_or_else(
        || {
            format!(
                "module file `{}` matches no directory entry exactly",
                path.display()
            )
        },
        |on_disk| {
            format!(
                "module file `{}` is spelled `{on_disk}` on disk",
                path.display()
            )
        },
    )
}

impl LoadError {
    /// The named source a located loader diagnostic points into; `None` for
    /// failures that precede any readable source.
    #[must_use]
    pub const fn registered_source(&self) -> Option<&RegisteredSource> {
        match self {
            Self::FileNotFound { .. }
            | Self::InvalidSourcePath { .. }
            | Self::CircularImport { .. }
            | Self::ManifestError { .. }
            | Self::SourceReadFailed { .. }
            | Self::InvalidModuleFileName { .. }
            | Self::ModuleFileSpellingMismatch { .. } => None,
            Self::ImportFileNotFound { src, .. }
            | Self::ImportOutsideRoot { src, .. }
            | Self::PackageNameMismatch { src, .. }
            | Self::StdlibNotImplemented { src, .. }
            | Self::CrossFileImportInVirtualPackage { src, .. }
            | Self::FileRootSelfImport { src, .. }
            | Self::PackageFileStemInDagBody { src, .. }
            | Self::UnknownDependency { src, .. } => Some(src),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn source_less_failures_keep_their_codes_and_have_no_source() {
        for (error, code) in [
            (
                LoadError::FileNotFound {
                    path: PathBuf::from("a.gcl"),
                },
                "graphcal::M000",
            ),
            (
                LoadError::InvalidSourcePath {
                    path: PathBuf::from("a.txt"),
                    reason: "wrong extension".to_owned(),
                },
                "graphcal::M023",
            ),
            (
                LoadError::ManifestError {
                    message: "bad".to_owned(),
                },
                "graphcal::M015",
            ),
            (
                LoadError::SourceReadFailed {
                    path: PathBuf::from("main.gcl"),
                    reason: "cannot read non-regular file".to_owned(),
                },
                "graphcal::M039",
            ),
            (
                LoadError::InvalidModuleFileName {
                    path: PathBuf::from("src/pkg/MyModule.gcl"),
                    reason: "`MyModule` is not lower_snake_case".to_owned(),
                },
                "graphcal::M040",
            ),
            (
                LoadError::ModuleFileSpellingMismatch {
                    path: PathBuf::from("src/pkg/Lib.gcl"),
                    on_disk: Some("lib.gcl".to_owned()),
                },
                "graphcal::M041",
            ),
        ] {
            assert_eq!(
                error.code().map(|code| code.to_string()).as_deref(),
                Some(code)
            );
            assert!(error.registered_source().is_none());
        }
    }

    fn atom(name: &str) -> NameAtom {
        NameAtom::parse(name).unwrap()
    }

    #[test]
    fn located_failures_point_into_their_source() {
        let src = RegisteredSource::detached("main.gcl", Arc::new("import x;".to_owned()));
        let error = LoadError::FileRootSelfImport {
            path: ModulePathKey::new(graphcal_compiler::syntax::non_empty::NonEmpty::singleton(
                atom("x"),
            )),
            src,
            span: (0, 6).into(),
        };
        assert_eq!(
            error.registered_source().map(RegisteredSource::name),
            Some("main.gcl")
        );
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("graphcal::M033")
        );
    }

    #[test]
    fn unknown_dependency_names_the_selector_and_the_importing_package() {
        let src =
            RegisteredSource::detached("main.gcl", Arc::new("import units.si::{x};".to_owned()));
        let error = LoadError::UnknownDependency {
            name: atom("units"),
            package: PackageName::new("mission").unwrap(),
            src,
            span: (7, 8).into(),
        };
        assert_eq!(
            error.to_string(),
            "unknown dependency `units` in package `mission`"
        );
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("graphcal::M035")
        );
        assert_eq!(
            error.registered_source().map(RegisteredSource::name),
            Some("main.gcl")
        );
    }
}
