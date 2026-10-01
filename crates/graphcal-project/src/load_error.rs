//! Failures of the project loader: reading, locating, and resolving source
//! files and manifests before any module is compiled.
//!
//! The loader is the shell that owns file names and source text, so its
//! located diagnostics carry the [`NamedSource`] they point into.

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::import_cycle::ImportCycle;

/// Why a project could not be loaded.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum LoadError {
    #[error("file not found: {path}")]
    #[diagnostic(code(graphcal::M000), help("check that the file path is correct"))]
    FileNotFound { path: String },

    #[error("invalid source path `{path}`: {reason}")]
    #[diagnostic(
        code(graphcal::M023),
        help("Graphcal source files must be UTF-8 `.gcl` files")
    )]
    InvalidSourcePath { path: String, reason: String },

    #[error("circular import detected: {cycle}")]
    #[diagnostic(
        code(graphcal::M001),
        help("files cannot import each other in a cycle")
    )]
    CircularImport { cycle: ImportCycle },

    #[error("failed to parse graphcal.toml: {message}")]
    #[diagnostic(code(graphcal::M015))]
    ManifestError { message: String },

    #[error("imported file not found: {path}")]
    #[diagnostic(code(graphcal::M002))]
    ImportFileNotFound {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("resolves outside project root")]
        span: SourceSpan,
    },

    #[error("module path starts with `{path_first}` but package name is `{package_name}`")]
    #[diagnostic(
        code(graphcal::M013),
        help("module paths must start with the package name from graphcal.toml")
    )]
    PackageNameMismatch {
        path_first: String,
        package_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this import resolves to its own file root")]
        span: SourceSpan,
    },
}

impl LoadError {
    /// The named source a located loader diagnostic points into; `None` for
    /// failures that precede any readable source.
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::FileNotFound { .. }
            | Self::InvalidSourcePath { .. }
            | Self::CircularImport { .. }
            | Self::ManifestError { .. } => None,
            Self::ImportFileNotFound { src, .. }
            | Self::ImportOutsideRoot { src, .. }
            | Self::PackageNameMismatch { src, .. }
            | Self::StdlibNotImplemented { src, .. }
            | Self::CrossFileImportInVirtualPackage { src, .. }
            | Self::FileRootSelfImport { src, .. } => Some(src),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_less_failures_keep_their_codes_and_have_no_source() {
        for (error, code) in [
            (
                LoadError::FileNotFound {
                    path: "a.gcl".to_owned(),
                },
                "graphcal::M000",
            ),
            (
                LoadError::InvalidSourcePath {
                    path: "a.txt".to_owned(),
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
        ] {
            assert_eq!(
                error.code().map(|code| code.to_string()).as_deref(),
                Some(code)
            );
            assert!(error.named_source().is_none());
        }
    }

    #[test]
    fn located_failures_point_into_their_source() {
        let src = NamedSource::new("main.gcl", Arc::new("import x;".to_owned()));
        let error = LoadError::FileRootSelfImport {
            path: "x".to_owned(),
            src,
            span: (0, 6).into(),
        };
        assert_eq!(
            error.named_source().map(NamedSource::name),
            Some("main.gcl")
        );
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("graphcal::M033")
        );
    }
}
