//! Pure construction of the dependency-ordered loaded files from a
//! [`SourceSnapshot`].
//!
//! The builder performs no IO. It walks the snapshot depth-first from the root
//! in the loader's load order, reports the first failure in that order (read or
//! parse failures, unresolved or self-referencing imports, and import cycles),
//! assigns each file its [`DagId`], and connects every import/include path to
//! the exact module it names.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use super::inline_dags::lift_inline_dags;
use super::source_snapshot::{
    DependencySite, FetchedFile, FileRootDependencyKind, ModuleResolution, ParsedFile,
    ParsedSource, ResolveFailure, ResolvedFile, SourceKey, SourceSnapshot,
    collect_inline_dag_names, file_root_dependency, file_stem, loading_dependency_paths,
    outside_root,
};
use super::{LoadedFile, ModulePathKey, io_not_found};
use crate::dependency_ordered::DependencyOrdered;
use crate::eval::CompileError;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::Declaration;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::import_cycle::ImportCycle;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::ModulePath;

/// Build every loaded file reachable from the snapshot root, dependencies
/// before dependents and ending with the root.
///
/// A file or dependency path the snapshot does not record is reported as not
/// found.
///
/// # Errors
///
/// Returns the first failure in load order: a recorded read/parse failure, an
/// unresolved file-root import/include, an import outside the permitted root,
/// a file-root self import, an import cycle, or an invalid module path.
pub(super) fn build_loaded_files<K: SourceKey>(
    snapshot: SourceSnapshot<K>,
) -> Result<DependencyOrdered<LoadedFile>, CompileError> {
    let SourceSnapshot { root, files } = snapshot;
    let mut builder = Builder {
        unbuilt: files,
        loading: Vec::new(),
        built: HashMap::new(),
        dependencies: Vec::new(),
    };
    let root = builder.build_file(&root)?;
    Ok(DependencyOrdered::new(builder.dependencies, root))
}

struct Builder<K> {
    /// Fetched files not yet built.
    unbuilt: HashMap<K, FetchedFile<K>>,
    /// Files being built, from the root to the current file.
    loading: Vec<K>,
    /// Completed files and their identities.
    built: HashMap<K, DagId>,
    /// Completed files other than the root, in post-order.
    dependencies: Vec<LoadedFile>,
}

/// Owning file of an import/include path, known before this file's own
/// [`DagId`] is assigned.
enum ImportOwner {
    ThisFile,
    Dependency(DagId),
}

impl<K: SourceKey> Builder<K> {
    /// Identity of `file`, building it (and its dependencies) first when needed.
    fn dependency(&mut self, file: &K) -> Result<DagId, CompileError> {
        if let Some(dag_id) = self.built.get(file) {
            return Ok(dag_id.clone());
        }
        let loaded = self.build_file(file)?;
        let dag_id = loaded.dag_id.clone();
        self.dependencies.push(loaded);
        Ok(dag_id)
    }

    fn build_file(&mut self, file: &K) -> Result<LoadedFile, CompileError> {
        if self.loading.contains(file) {
            return Err(CompileError::Eval(GraphcalError::CircularImport {
                cycle: ImportCycle::new(
                    self.loading.iter().map(SourceKey::chain_file).collect(),
                    file.chain_file(),
                ),
            }));
        }
        let parsed = self
            .unbuilt
            .remove(file)
            .ok_or_else(|| io_not_found(file.path()))??;
        self.loading.push(file.clone());

        let stem = file_stem(file.path());
        let named_source = parsed.named_source();
        reject_file_root_stem_imports(&parsed.ast().declarations, stem, named_source)?;
        let imports = self.load_file_root_dependencies(file, &parsed)?;
        self.load_dag_body_dependencies(file, &parsed)?;

        let location = parsed.location();
        let dag_id = DagId::from_relative_path(location.package.clone(), &location.relative_path)
            .map_err(|error| {
            CompileError::Eval(GraphcalError::internal_error(
                format!(
                    "invalid module path `{}`: {error}",
                    location.relative_path.display()
                ),
                named_source,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
        let resolved_imports = imports
            .into_iter()
            .map(|(key, (owner, resolved))| {
                let source_file = match owner {
                    ImportOwner::ThisFile => dag_id.clone(),
                    ImportOwner::Dependency(dependency) => dependency,
                };
                (key, resolved.target_from(&source_file))
            })
            .collect();
        let inline_dags = lift_inline_dags(parsed.ast(), &dag_id, stem, |path| {
            let Some(ModuleResolution::Resolved(resolved)) = parsed.resolution(path) else {
                return None;
            };
            let source_file = if resolved.file == *file {
                Some(dag_id.clone())
            } else {
                self.built.get(&resolved.file).cloned()
            }?;
            Some(resolved.target_from(&source_file))
        });

        self.loading.pop();
        self.built.insert(file.clone(), dag_id.clone());
        let ParsedFile {
            source,
            named_source,
            ast,
        } = parsed.into_file();
        Ok(LoadedFile {
            path: file.path().to_path_buf(),
            dag_id,
            source,
            ast,
            named_source,
            resolved_imports,
            inline_dags,
        })
    }

    /// Load the files named by file-root imports/includes and record each
    /// path's owning file. Every resolution failure is an error here.
    fn load_file_root_dependencies<'p>(
        &mut self,
        file: &K,
        parsed: &'p ParsedSource<K>,
    ) -> Result<HashMap<ModulePathKey, (ImportOwner, &'p ResolvedFile<K>)>, CompileError> {
        let src = parsed.named_source();
        let mut imports = HashMap::new();
        for dependency in loading_dependency_paths(parsed.ast(), file_stem(file.path())) {
            let DependencySite::FileRoot(kind) = dependency.site else {
                continue;
            };
            let path = dependency.path;
            // An unrecorded path did not resolve to any file.
            let resolved = match parsed.resolution(path) {
                Some(ModuleResolution::Resolved(resolved)) => resolved,
                Some(ModuleResolution::OutsideProjectRoot) => {
                    return Err(CompileError::Eval(outside_root(path, src.clone())));
                }
                Some(ModuleResolution::Failed(failure)) => return Err(failure.to_error(path, src)),
                None => return Err(ResolveFailure::FileNotFound.to_error(path, src)),
            };
            let owner = if resolved.file == *file {
                if kind == FileRootDependencyKind::Import && resolved.inline_path.is_empty() {
                    return Err(file_root_self_import_error(path, src));
                }
                ImportOwner::ThisFile
            } else {
                ImportOwner::Dependency(self.dependency(&resolved.file)?)
            };
            imports.insert(ModulePathKey::from_path(path), (owner, resolved));
        }
        Ok(imports)
    }

    /// Load the files named by inline-DAG body imports/includes. Unresolved
    /// body paths stay in the AST for the module resolver to report with their
    /// spans; only a path outside the project root is an error here.
    fn load_dag_body_dependencies(
        &mut self,
        file: &K,
        parsed: &ParsedSource<K>,
    ) -> Result<(), CompileError> {
        for dependency in loading_dependency_paths(parsed.ast(), file_stem(file.path())) {
            if dependency.site != DependencySite::DagBody {
                continue;
            }
            match parsed.resolution(dependency.path) {
                Some(ModuleResolution::Resolved(resolved)) if resolved.file != *file => {
                    self.dependency(&resolved.file)?;
                }
                Some(ModuleResolution::Resolved(_) | ModuleResolution::Failed(_)) | None => {}
                Some(ModuleResolution::OutsideProjectRoot) => {
                    return Err(CompileError::Eval(outside_root(
                        dependency.path,
                        parsed.named_source().clone(),
                    )));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn file_root_self_import_error(
    path: &ModulePath,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    CompileError::Eval(GraphcalError::FileRootSelfImport {
        path: path.display_path(),
        src: src.clone(),
        span: path.span().into(),
    })
}

/// Reject `import <file stem>` at file root: a file cannot import itself as a
/// module, unless the stem also names one of its inline DAGs.
pub(super) fn reject_file_root_stem_imports(
    declarations: &[Declaration],
    file_stem: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    let dag_names: HashSet<String> = collect_inline_dag_names(declarations);
    declarations.iter().try_for_each(|declaration| {
        let Some((path, FileRootDependencyKind::Import)) = file_root_dependency(declaration) else {
            return Ok(());
        };
        let is_file_root_self_import = path.segments.len() == 1
            && path.segments[0].name.as_str() == file_stem
            && !dag_names.contains(path.segments[0].name.as_str());
        if is_file_root_self_import {
            Err(file_root_self_import_error(path, src))
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use graphcal_compiler::dag_id::DagPackageId;
    use graphcal_compiler::import_cycle::ImportChainFile;
    use graphcal_compiler::syntax::decl_name::DeclName;
    use graphcal_package::PackageInstanceId;

    use super::super::source_snapshot::{ModuleLocation, PackageFileKey, ResolvedFile};
    use super::super::{InlineBodyImportResolution, ResolvedModuleTarget};
    use super::*;

    const PACKAGE: &str = "pkg";

    fn key(name: &str) -> PathBuf {
        PathBuf::from(format!("/p/src/{name}.gcl"))
    }

    fn resolved(name: &str, inline: &[&str]) -> ModuleResolution<PathBuf> {
        ModuleResolution::Resolved(ResolvedFile {
            file: key(name),
            inline_path: inline
                .iter()
                .map(|segment| DeclName::try_new((*segment).to_string()).unwrap())
                .collect(),
        })
    }

    fn parse(file: &Path, text: &str) -> ParsedFile {
        let name = file.display().to_string();
        let source = Arc::new(text.to_string());
        let named_source = NamedSource::new(name.as_str(), Arc::clone(&source));
        let raw = graphcal_compiler::syntax::parser::Parser::new(&source)
            .parse_file()
            .unwrap();
        ParsedFile {
            source,
            named_source,
            ast: graphcal_compiler::desugar::desugared_ast::File::from(raw),
        }
    }

    /// A parsed file whose dependency paths resolve through `table`, keyed by
    /// dotted display path; unlisted paths are not found.
    fn fetched_at<K: SourceKey>(
        file: &K,
        location: ModuleLocation,
        text: &str,
        table: &[(&str, ModuleResolution<K>)],
    ) -> ParsedSource<K> {
        let parsed = ParsedSource::resolve(location, parse(file.path(), text), |path| {
            Ok::<_, std::convert::Infallible>(
                table
                    .iter()
                    .find(|(display, _)| *display == path.display_path())
                    .map_or(
                        ModuleResolution::Failed(ResolveFailure::FileNotFound),
                        |(_, resolution)| resolution.clone(),
                    ),
            )
        });
        match parsed {
            Ok(parsed) => parsed,
            Err(never) => match never {},
        }
    }

    fn fetched(
        name: &str,
        text: &str,
        table: &[(&str, ModuleResolution<PathBuf>)],
    ) -> (PathBuf, FetchedFile<PathBuf>) {
        let file = key(name);
        let location = ModuleLocation {
            package: DagPackageId::new(PACKAGE),
            relative_path: PathBuf::from(format!("src/{name}.gcl")),
        };
        let fetched = Ok(fetched_at(&file, location, text, table));
        (file, fetched)
    }

    fn snapshot<const N: usize>(
        root: &str,
        files: [(PathBuf, FetchedFile<PathBuf>); N],
    ) -> SourceSnapshot<PathBuf> {
        SourceSnapshot {
            root: key(root),
            files: files.into_iter().collect(),
        }
    }

    fn dag_id(name: &str) -> DagId {
        DagId::from_relative_path(PACKAGE, Path::new(&format!("src/{name}.gcl"))).unwrap()
    }

    fn build_error<K: SourceKey>(snapshot: SourceSnapshot<K>) -> GraphcalError {
        match build_loaded_files(snapshot) {
            Err(CompileError::Eval(error)) => error,
            Err(other) => panic!("unexpected error kind: {other:?}"),
            Ok(_) => panic!("expected the build to fail"),
        }
    }

    fn order(files: &DependencyOrdered<LoadedFile>) -> Vec<DagId> {
        files.iter().map(|file| file.dag_id.clone()).collect()
    }

    fn import_targets(file: &LoadedFile) -> Vec<ResolvedModuleTarget> {
        file.imports_with_targets()
            .map(|(_, _, target)| target.clone())
            .collect()
    }

    #[test]
    fn dependencies_precede_dependents_in_post_order() {
        let files = build_loaded_files(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "import pkg.b::{y};\nimport pkg.c::{z};",
                    &[("pkg.b", resolved("b", &[])), ("pkg.c", resolved("c", &[]))],
                ),
                fetched("b", "import pkg.c::{z};", &[("pkg.c", resolved("c", &[]))]),
                fetched("c", "param z: Dimensionless = 1.0;", &[]),
            ],
        ))
        .unwrap();

        assert_eq!(order(&files), [dag_id("c"), dag_id("b"), dag_id("main")]);
        let root = files.root();
        assert_eq!(root.path, key("main"));
        assert_eq!(
            root.source.as_str(),
            "import pkg.b::{y};\nimport pkg.c::{z};"
        );
        assert_eq!(root.named_source.name(), "/p/src/main.gcl");
        assert_eq!(
            import_targets(root),
            [
                ResolvedModuleTarget::file_root(dag_id("b")),
                ResolvedModuleTarget::file_root(dag_id("c")),
            ]
        );
    }

    #[test]
    fn diamond_dependency_is_built_once() {
        let files = build_loaded_files(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "import pkg.b::{y};\nimport pkg.c::{z};",
                    &[("pkg.b", resolved("b", &[])), ("pkg.c", resolved("c", &[]))],
                ),
                fetched("b", "import pkg.d::{w};", &[("pkg.d", resolved("d", &[]))]),
                fetched("c", "import pkg.d::{w};", &[("pkg.d", resolved("d", &[]))]),
                fetched("d", "param w: Dimensionless = 1.0;", &[]),
            ],
        ))
        .unwrap();

        assert_eq!(
            order(&files),
            [dag_id("d"), dag_id("b"), dag_id("c"), dag_id("main")]
        );
        assert_eq!(
            import_targets(files.iter().nth(2).unwrap()),
            [ResolvedModuleTarget::file_root(dag_id("d"))]
        );
    }

    #[test]
    fn inline_path_targets_nested_dag_in_owner_file() {
        let files = build_loaded_files(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "import pkg.lib.inner::{x};",
                    &[("pkg.lib.inner", resolved("lib", &["inner"]))],
                ),
                fetched("lib", "dag inner { param x: Dimensionless = 1.0; }", &[]),
            ],
        ))
        .unwrap();

        let [target] = import_targets(files.root()).try_into().unwrap();
        assert_eq!(target.source_file(), &dag_id("lib"));
        assert_eq!(target.target(), &dag_id("lib").child("inner"));
    }

    #[test]
    fn import_cycle_reports_typed_loading_chain() {
        let error = build_error(snapshot(
            "a",
            [
                fetched("a", "import pkg.b::{y};", &[("pkg.b", resolved("b", &[]))]),
                fetched("b", "import pkg.c::{z};", &[("pkg.c", resolved("c", &[]))]),
                fetched("c", "import pkg.b::{y};", &[("pkg.b", resolved("b", &[]))]),
            ],
        ));

        let GraphcalError::CircularImport { cycle } = &error else {
            panic!("expected a circular import, got {error:?}");
        };
        assert_eq!(
            cycle.loading(),
            [
                ImportChainFile::Path(key("a")),
                ImportChainFile::Path(key("b")),
                ImportChainFile::Path(key("c")),
            ]
        );
        assert_eq!(cycle.repeated(), &ImportChainFile::Path(key("b")));
        assert_eq!(
            error.to_string(),
            "circular import detected: /p/src/a.gcl -> /p/src/b.gcl -> /p/src/c.gcl -> /p/src/b.gcl"
        );
    }

    #[test]
    fn package_import_cycle_labels_files_with_their_package() {
        let package = PackageInstanceId::new("dep").unwrap();
        let root = PackageFileKey {
            package: package.clone(),
            path: PathBuf::from("/cache/dep/src/dep.gcl"),
        };
        let other = PackageFileKey {
            package,
            path: PathBuf::from("/cache/dep/src/dep/b.gcl"),
        };
        let importing = |from: &PackageFileKey, to: &PackageFileKey, relative: &str| {
            (
                from.clone(),
                Ok(fetched_at(
                    from,
                    ModuleLocation {
                        package: DagPackageId::new("dep"),
                        relative_path: PathBuf::from(relative),
                    },
                    "import dep.x::{y};",
                    &[(
                        "dep.x",
                        ModuleResolution::Resolved(ResolvedFile {
                            file: to.clone(),
                            inline_path: Vec::new(),
                        }),
                    )],
                )),
            )
        };
        let error = build_error(SourceSnapshot {
            root: root.clone(),
            files: [
                importing(&root, &other, "src/dep.gcl"),
                importing(&other, &root, "src/dep/b.gcl"),
            ]
            .into_iter()
            .collect(),
        });
        assert_eq!(
            error.to_string(),
            "circular import detected: dep:/cache/dep/src/dep.gcl -> dep:/cache/dep/src/dep/b.gcl -> dep:/cache/dep/src/dep.gcl"
        );
    }

    #[test]
    fn earlier_import_failure_wins_over_later_file_failure() {
        let error = build_error(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "import graphcal.core::{x};\nimport pkg.b::{y};",
                    &[
                        (
                            "graphcal.core",
                            ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented),
                        ),
                        ("pkg.b", resolved("b", &[])),
                    ],
                ),
                (key("b"), Err(io_not_found(&key("b")))),
            ],
        ));
        assert!(
            matches!(error, GraphcalError::StdlibNotImplemented { ref path, .. } if path == "graphcal.core"),
            "{error:?}"
        );
    }

    #[test]
    fn dependency_failure_is_reported_when_reached() {
        let error = build_error(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "import pkg.b::{y};\nimport graphcal.core::{x};",
                    &[
                        ("pkg.b", resolved("b", &[])),
                        (
                            "graphcal.core",
                            ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented),
                        ),
                    ],
                ),
                (key("b"), Err(io_not_found(&key("b")))),
            ],
        ));
        assert!(
            matches!(error, GraphcalError::FileNotFound { ref path } if path == "/p/src/b.gcl"),
            "{error:?}"
        );
    }

    #[test]
    fn file_missing_from_snapshot_is_not_found() {
        let error = build_error(snapshot(
            "main",
            [fetched(
                "main",
                "import pkg.b::{y};",
                &[("pkg.b", resolved("b", &[]))],
            )],
        ));
        assert!(
            matches!(error, GraphcalError::FileNotFound { ref path } if path == "/p/src/b.gcl"),
            "{error:?}"
        );
        let error = build_error(snapshot("main", []));
        assert!(
            matches!(error, GraphcalError::FileNotFound { ref path } if path == "/p/src/main.gcl"),
            "{error:?}"
        );
    }

    fn failing_import(failure: ResolveFailure) -> GraphcalError {
        build_error(snapshot(
            "main",
            [fetched(
                "main",
                "import pkg.b::{y};",
                &[("pkg.b", ModuleResolution::Failed(failure))],
            )],
        ))
    }

    #[test]
    fn resolution_failures_render_at_the_import_site() {
        let error = failing_import(ResolveFailure::PackageNameMismatch {
            package_name: "other".to_string(),
        });
        assert!(
            matches!(
                error,
                GraphcalError::PackageNameMismatch { ref path_first, ref package_name, .. }
                    if path_first == "pkg" && package_name == "other"
            ),
            "{error:?}"
        );
        let error = failing_import(ResolveFailure::FileNotFound);
        assert!(
            matches!(error, GraphcalError::ImportFileNotFound { ref path, .. } if path == "pkg.b"),
            "{error:?}"
        );
        let error = failing_import(ResolveFailure::CrossFileImportInVirtualPackage);
        assert!(
            matches!(error, GraphcalError::CrossFileImportInVirtualPackage { ref path, .. } if path == "pkg.b"),
            "{error:?}"
        );
        let error = failing_import(ResolveFailure::OutsidePackageRoot);
        assert!(
            matches!(error, GraphcalError::ImportOutsideRoot { ref path, .. } if path == "pkg.b"),
            "{error:?}"
        );
        let error = failing_import(ResolveFailure::NotLocked {
            message: "no dependency `b`".to_string(),
        });
        assert!(
            matches!(
                error,
                GraphcalError::EvalError { ref message, .. }
                    if message == "no dependency `b`; run `graphcal deps lock` after changing dependencies"
            ),
            "{error:?}"
        );
        let error = failing_import(ResolveFailure::Manifest {
            message: "lockfile package `b` is missing".to_string(),
        });
        assert!(
            matches!(
                error,
                GraphcalError::ManifestError { ref message } if message == "lockfile package `b` is missing"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn file_root_self_import_is_rejected() {
        let error = build_error(snapshot(
            "main",
            [fetched(
                "main",
                "import pkg.main::{x};",
                &[("pkg.main", resolved("main", &[]))],
            )],
        ));
        assert!(
            matches!(error, GraphcalError::FileRootSelfImport { ref path, .. } if path == "pkg.main"),
            "{error:?}"
        );

        let error = build_error(snapshot(
            "main",
            [fetched("main", "import main::{x};", &[])],
        ));
        assert!(
            matches!(error, GraphcalError::FileRootSelfImport { ref path, .. } if path == "main"),
            "{error:?}"
        );
    }

    #[test]
    fn self_references_through_inline_paths_and_includes_are_allowed() {
        let files = build_loaded_files(snapshot(
            "main",
            [fetched(
                "main",
                "import pkg.main.inner::{x};\ndag inner { param x: Dimensionless = 1.0; }",
                &[("pkg.main.inner", resolved("main", &["inner"]))],
            )],
        ))
        .unwrap();
        let [target] = import_targets(files.root()).try_into().unwrap();
        assert_eq!(target.source_file(), &dag_id("main"));
        assert_eq!(target.target(), &dag_id("main").child("inner"));

        let files = build_loaded_files(snapshot(
            "main",
            [fetched(
                "main",
                "include pkg.main()::{x};",
                &[("pkg.main", resolved("main", &[]))],
            )],
        ))
        .unwrap();
        let (_, _, target) = files.root().includes_with_targets().next().unwrap();
        assert_eq!(target, &ResolvedModuleTarget::file_root(dag_id("main")));
    }

    #[test]
    fn same_file_dag_references_are_not_dependencies() {
        let files = build_loaded_files(snapshot(
            "main",
            [fetched(
                "main",
                "dag inner { param x: Dimensionless = 1.0; }\ninclude inner()::{x};",
                &[],
            )],
        ))
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.root().resolved_imports.is_empty());
    }

    #[test]
    fn outside_project_root_is_rejected_at_file_root_and_in_dag_bodies() {
        for text in ["import pkg.b::{y};", "dag inner { import pkg.b::{y}; }"] {
            let error = build_error(snapshot(
                "main",
                [fetched(
                    "main",
                    text,
                    &[("pkg.b", ModuleResolution::OutsideProjectRoot)],
                )],
            ));
            assert!(
                matches!(error, GraphcalError::ImportOutsideRoot { ref path, .. } if path == "pkg.b"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn dag_body_imports_load_dependencies_and_keep_failures_unresolved() {
        let files = build_loaded_files(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "dag inner {\n  import pkg.b::{y};\n  import pkg.missing::{z};\n  import pkg.main::{w};\n}\nparam w: Dimensionless = 1.0;",
                    &[
                        ("pkg.b", resolved("b", &[])),
                        (
                            "pkg.missing",
                            ModuleResolution::Failed(ResolveFailure::FileNotFound),
                        ),
                        ("pkg.main", resolved("main", &[])),
                    ],
                ),
                fetched("b", "param y: Dimensionless = 1.0;", &[]),
            ],
        ))
        .unwrap();

        assert_eq!(order(&files), [dag_id("b"), dag_id("main")]);
        let inner = &files.root().inline_dags[0];
        assert_eq!(inner.dag_id, dag_id("main").child("inner"));
        let resolution = |segments: &[&str]| {
            inner
                .resolved_imports
                .iter()
                .find(|(key, _)| key.segments() == segments)
                .map(|(_, resolution)| resolution.clone())
                .unwrap()
        };
        assert_eq!(
            resolution(&["pkg", "b"]),
            InlineBodyImportResolution::Resolved(ResolvedModuleTarget::file_root(dag_id("b")))
        );
        assert_eq!(
            resolution(&["pkg", "missing"]),
            InlineBodyImportResolution::Unresolved
        );
        assert_eq!(
            resolution(&["pkg", "main"]),
            InlineBodyImportResolution::Resolved(ResolvedModuleTarget::file_root(dag_id("main")))
        );
    }

    #[test]
    fn dag_body_reference_to_an_unloaded_file_stays_unresolved() {
        // A single-segment body path naming a same-file DAG outside lexical
        // scope never loads the file it would resolve to.
        let files = build_loaded_files(snapshot(
            "main",
            [
                fetched(
                    "main",
                    "dag a { dag pkg { } }\ndag b { import pkg::{x}; }",
                    &[("pkg", resolved("other", &[]))],
                ),
                fetched("other", "param x: Dimensionless = 1.0;", &[]),
            ],
        ))
        .unwrap();
        assert_eq!(files.len(), 1);
        let b = files
            .root()
            .inline_dags
            .iter()
            .find(|dag| dag.dag_id.leaf().spelling() == Some("b"))
            .unwrap();
        assert!(
            b.resolved_imports
                .values()
                .all(|resolution| *resolution == InlineBodyImportResolution::Unresolved)
        );
    }

    #[test]
    fn invalid_module_location_is_an_error() {
        let file = key("main");
        let location = ModuleLocation {
            package: DagPackageId::new(PACKAGE),
            relative_path: PathBuf::from("src/main.txt"),
        };
        let fetched = fetched_at(&file, location, "param x: Dimensionless = 1.0;", &[]);
        let error = build_error(SourceSnapshot {
            root: file.clone(),
            files: std::iter::once((file, Ok(fetched))).collect(),
        });
        assert!(
            error
                .to_string()
                .contains("invalid module path `src/main.txt`"),
            "{error}"
        );
    }
}
