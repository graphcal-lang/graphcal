//! Typed import chain reported when a source file imports itself transitively.
//!
//! The loader detects the cycle on its [`crate::dependency_graph`] of typed
//! file keys; this module keeps the diagnostic payload structured until the
//! error is rendered.

use std::fmt;
use std::path::PathBuf;

use crate::dag_id::DagPackageId;
use crate::dependency_graph::Cycle;

/// One source file on an import chain, identified the way its loader named it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
/// `lead_in` is the chain of files, from the root file, that imports its way
/// to [`Cycle::entry`] without being on the cycle itself; `cycle` is the chain
/// of files that import each other in a circle, starting at the first of them
/// the loader reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCycle {
    lead_in: Vec<ImportChainFile>,
    cycle: Cycle<ImportChainFile>,
}

impl ImportCycle {
    /// The `cycle` reached from the root file through `lead_in`.
    #[must_use]
    pub const fn new(lead_in: Vec<ImportChainFile>, cycle: Cycle<ImportChainFile>) -> Self {
        Self { lead_in, cycle }
    }

    /// Files from the root file up to (excluding) the cycle's entry.
    #[must_use]
    pub fn lead_in(&self) -> &[ImportChainFile] {
        &self.lead_in
    }

    /// Files importing each other in a circle.
    #[must_use]
    pub const fn cycle(&self) -> &Cycle<ImportChainFile> {
        &self.cycle
    }
}

/// The chain from the root file around the cycle and back to its entry, e.g.
/// `main -> a -> b -> a`.
impl fmt::Display for ImportCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for file in self.lead_in.iter().chain(self.cycle.path()) {
            write!(f, "{file} -> ")?;
        }
        write!(f, "{}", self.cycle.entry())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_graph::DependencyGraph;

    fn path(name: &str) -> ImportChainFile {
        ImportChainFile::Path(PathBuf::from(name))
    }

    fn cycle(files: &[ImportChainFile]) -> Cycle<ImportChainFile> {
        let mut graph = DependencyGraph::new();
        for pair in files.windows(2) {
            graph.add_dependency(pair[0].clone(), pair[1].clone());
        }
        let last = files.last().unwrap().clone();
        graph.add_dependency(last, files[0].clone());
        graph.into_topo_order().unwrap_err()
    }

    #[test]
    fn renders_lead_in_then_cycle_back_to_its_entry() {
        let cycle = ImportCycle::new(
            vec![path("/p/main.gcl")],
            cycle(&[path("/p/lib.gcl"), path("/p/util.gcl")]),
        );
        assert_eq!(
            cycle.to_string(),
            "/p/main.gcl -> /p/lib.gcl -> /p/util.gcl -> /p/lib.gcl"
        );
        assert_eq!(cycle.lead_in(), [path("/p/main.gcl")]);
        assert_eq!(cycle.cycle().entry(), &path("/p/lib.gcl"));
    }

    #[test]
    fn package_files_are_qualified_by_package() {
        let file = ImportChainFile::Package {
            package: DagPackageId::new("dep"),
            path: PathBuf::from("/cache/dep/src/dep.gcl"),
        };
        let cycle = ImportCycle::new(Vec::new(), cycle(&[file]));
        assert_eq!(
            cycle.to_string(),
            "dep:/cache/dep/src/dep.gcl -> dep:/cache/dep/src/dep.gcl"
        );
    }
}
