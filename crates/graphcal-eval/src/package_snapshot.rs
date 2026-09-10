//! Capture a package's authenticated source and executable artifact closure.
//!
//! Both lock generation and loading call this boundary. Plugin discovery reads
//! captured source bytes, and the resulting snapshot is the only reader used
//! after verification; neither source nor executable bytes are reread from disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use graphcal_compiler::syntax::{ast::DeclKind, parser::Parser, plugin::PluginSourceKind};
use graphcal_io::{CancellationSignal, FileSystemReader, SourceTreeHashLimits, SourceTreeSnapshot};
use thiserror::Error;

/// Capture the source tree and every Wasm import declared anywhere inside it.
///
/// # Errors
/// Fails on invalid source, unsafe/missing artifacts, cancellation or resource limits.
pub fn capture_package(
    fs: &dyn FileSystemReader,
    root: &Path,
    source_dir: &Path,
    limits: SourceTreeHashLimits,
    cancellation: &dyn CancellationSignal,
) -> Result<SourceTreeSnapshot, PackageSnapshotError> {
    let mut snapshot =
        graphcal_io::capture_source_tree(fs, root, source_dir, limits, cancellation)?;
    let mut plugins = BTreeSet::new();
    for (path, bytes) in snapshot
        .files()
        .filter(|(path, _)| path.extension().is_some_and(|ext| ext == "gcl"))
    {
        if cancellation.is_cancelled() {
            return Err(graphcal_io::SourceTreeHashError::Cancelled.into());
        }
        let source =
            std::str::from_utf8(bytes).map_err(|_| PackageSnapshotError::InvalidSource {
                path: path.to_path_buf(),
                message: "source is not UTF-8".to_string(),
            })?;
        let ast = Parser::with_name(source, &path.display().to_string())
            .parse_file()
            .map_err(|error| PackageSnapshotError::InvalidSource {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let mut pending = vec![ast.declarations.as_slice()];
        while let Some(declarations) = pending.pop() {
            for declaration in declarations {
                match &declaration.kind {
                    DeclKind::PluginImport(plugin)
                        if plugin.path.value.source_kind() == PluginSourceKind::WasmModule =>
                    {
                        plugins.insert(graphcal_package::PluginArtifactPath::new(
                            plugin.path.value.as_str(),
                        )?);
                    }
                    DeclKind::Dag(dag) => pending.push(&dag.body),
                    _ => {}
                }
            }
        }
    }
    for plugin in plugins {
        snapshot.capture_artifact(
            fs,
            root,
            Path::new(&plugin.to_string()),
            limits,
            cancellation,
        )?;
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_io::{
        ByteLimit, FileSystemReader, InMemoryFileSystem, NeverCancel, VirtualAbsolutePath,
    };

    #[test]
    fn nested_imports_extend_hash_coverage_and_execution_uses_captured_bytes() {
        let root = Path::new("/package");
        let mut fs = InMemoryFileSystem::new();
        for (path, content) in [
            ("graphcal.toml", "[package]\nname='test'\n"),
            (
                "src/test/lib.gcl",
                "pub dag calc { import plugin \"plugins/kernel.wasm\" as k { fn scale(x: Dimensionless) -> Dimensionless; } }",
            ),
            ("plugins/kernel.wasm", "first"),
        ] {
            fs.add_file(
                VirtualAbsolutePath::new(root.join(path)).unwrap(),
                content.to_string(),
            )
            .unwrap();
        }
        let snapshot = capture_package(
            &fs,
            root,
            Path::new("src"),
            SourceTreeHashLimits::unbounded(),
            &NeverCancel,
        )
        .unwrap();
        assert_eq!(snapshot.hash().files(), 3);
        fs.add_file(
            VirtualAbsolutePath::new(root.join("plugins/kernel.wasm")).unwrap(),
            "changed".to_string(),
        )
        .unwrap();
        let changed = capture_package(
            &fs,
            root,
            Path::new("src"),
            SourceTreeHashLimits::unbounded(),
            &NeverCancel,
        )
        .unwrap();
        assert_ne!(snapshot.hash().sha256(), changed.hash().sha256());
        let frozen = snapshot.mount(root).unwrap();
        assert_eq!(
            frozen
                .read_bytes_bounded(
                    &root.join("plugins/kernel.wasm"),
                    ByteLimit::new(100),
                    &NeverCancel
                )
                .unwrap(),
            b"first"
        );
    }
}

/// Failure to establish a complete, authenticated package snapshot.
#[derive(Debug, Error)]
pub enum PackageSnapshotError {
    #[error(transparent)]
    Tree(#[from] graphcal_io::SourceTreeHashError),
    #[error(transparent)]
    PluginPath(#[from] graphcal_package::PluginArtifactPathError),
    #[error("could not inspect plugin imports in {}: {message}", path.display())]
    InvalidSource { path: PathBuf, message: String },
}
