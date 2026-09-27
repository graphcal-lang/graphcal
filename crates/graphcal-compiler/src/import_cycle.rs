//! Typed import chain reported when a source file imports itself transitively.
//!
//! The loader detects the cycle on its typed file keys; this module keeps the
//! diagnostic payload structured until the error is rendered.

use std::fmt;
use std::path::PathBuf;

use crate::dag_id::DagPackageId;

/// One source file on an import chain, identified the way its loader named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportChainFile {
    /// A file of a single-package project, identified by its canonical path.
    Path(PathBuf),
    /// A file of a locked multi-package project: the owning package instance
    /// and the canonical path inside that package's source authority.
    Package {
        /// Owning package instance.
        package: DagPackageId,
        /// Canonical path of the file.
        path: PathBuf,
    },
}

impl fmt::Display for ImportChainFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(f, "{}", path.display()),
            Self::Package { package, path } => write!(f, "{package}:{}", path.display()),
        }
    }
}

/// Import chain that closes a cycle.
///
/// `loading` is the stack of files still being loaded, from the root file to
/// the importer, when `repeated` (a file already on that stack) was imported
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCycle {
    loading: Vec<ImportChainFile>,
    repeated: ImportChainFile,
}

impl ImportCycle {
    /// Close the chain of files being loaded with the file imported again.
    #[must_use]
    pub const fn new(loading: Vec<ImportChainFile>, repeated: ImportChainFile) -> Self {
        Self { loading, repeated }
    }

    /// Files being loaded, from the root file to the importer that closed the cycle.
    #[must_use]
    pub fn loading(&self) -> &[ImportChainFile] {
        &self.loading
    }

    /// File imported again while it was still being loaded.
    #[must_use]
    pub const fn repeated(&self) -> &ImportChainFile {
        &self.repeated
    }
}

impl fmt::Display for ImportCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for file in &self.loading {
            write!(f, "{file} -> ")?;
        }
        write!(f, "{}", self.repeated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_loading_stack_then_repeated_file() {
        let cycle = ImportCycle::new(
            vec![
                ImportChainFile::Path(PathBuf::from("/p/main.gcl")),
                ImportChainFile::Path(PathBuf::from("/p/lib.gcl")),
            ],
            ImportChainFile::Path(PathBuf::from("/p/main.gcl")),
        );
        assert_eq!(
            cycle.to_string(),
            "/p/main.gcl -> /p/lib.gcl -> /p/main.gcl"
        );
        assert_eq!(cycle.loading().len(), 2);
        assert_eq!(
            cycle.repeated(),
            &ImportChainFile::Path(PathBuf::from("/p/main.gcl"))
        );
    }

    #[test]
    fn package_files_are_qualified_by_package() {
        let file = ImportChainFile::Package {
            package: DagPackageId::new("dep"),
            path: PathBuf::from("/cache/dep/src/dep.gcl"),
        };
        let cycle = ImportCycle::new(Vec::new(), file);
        assert_eq!(cycle.to_string(), "dep:/cache/dep/src/dep.gcl");
    }
}
