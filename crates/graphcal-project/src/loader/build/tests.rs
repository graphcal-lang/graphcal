use crate::load_error::LoadError;
use graphcal_compiler::graphcal_error::RenderedGraphcalError;
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use std::path::{Path, PathBuf};

use graphcal_compiler::dag_id::DagPackageId;
use graphcal_compiler::import_cycle::ImportChainFile;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_package::PackageInstanceId;

use super::super::module_path::{InlineBodyImportResolution, ResolvedModuleTarget};
use super::super::source_snapshot::{ModuleLocation, PackageFileKey, ParsedFile, ResolvedFile};
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
    ParsedFile::parse(
        &mut graphcal_compiler::source_registry::SourceRegistry::new(),
        &file.display().to_string(),
        Arc::new(text.to_string()),
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap()
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
        sources: graphcal_compiler::source_registry::SourceRegistry::new(),
    }
}

fn dag_id(name: &str) -> DagId {
    DagId::from_relative_path(PACKAGE, Path::new(&format!("src/{name}.gcl"))).unwrap()
}

fn build_error<K: SourceKey>(snapshot: SourceSnapshot<K>) -> CompileError {
    match build_files(snapshot) {
        Err(error) => error,
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
    let files = build_files(snapshot(
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
    let files = build_files(snapshot(
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
    let files = build_files(snapshot(
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
    assert_eq!(
        target.target(),
        &dag_id("lib").inline_dag_child(
            graphcal_compiler::syntax::decl_name::DeclName::expect_valid("inner")
        )
    );
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

    let CompileError::Load(LoadError::CircularImport { cycle }) = &error else {
        panic!("expected a circular import, got {error:?}");
    };
    assert_eq!(cycle.lead_in(), [ImportChainFile::Path(key("a"))]);
    assert_eq!(
        cycle.cycle().path().cloned().collect::<Vec<_>>(),
        [
            ImportChainFile::Path(key("b")),
            ImportChainFile::Path(key("c")),
        ]
    );
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
        sources: graphcal_compiler::source_registry::SourceRegistry::new(),
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
        matches!(error, CompileError::Load(LoadError::StdlibNotImplemented { ref path, .. }) if path == "graphcal.core"),
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
        matches!(error, CompileError::Load(LoadError::FileNotFound { ref path }) if path == "/p/src/b.gcl"),
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
        matches!(error, CompileError::Load(LoadError::FileNotFound { ref path }) if path == "/p/src/b.gcl"),
        "{error:?}"
    );
    let error = build_error(snapshot("main", []));
    assert!(
        matches!(error, CompileError::Load(LoadError::FileNotFound { ref path }) if path == "/p/src/main.gcl"),
        "{error:?}"
    );
}

fn failing_import(failure: ResolveFailure) -> CompileError {
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
            CompileError::Load(LoadError::PackageNameMismatch { ref path_first, ref package_name, .. })
                if path_first == "pkg" && package_name == "other"
        ),
        "{error:?}"
    );
    let error = failing_import(ResolveFailure::FileNotFound);
    assert!(
        matches!(error, CompileError::Load(LoadError::ImportFileNotFound { ref path, .. }) if path == "pkg.b"),
        "{error:?}"
    );
    let error = failing_import(ResolveFailure::CrossFileImportInVirtualPackage);
    assert!(
        matches!(error, CompileError::Load(LoadError::CrossFileImportInVirtualPackage { ref path, .. }) if path == "pkg.b"),
        "{error:?}"
    );
    let error = failing_import(ResolveFailure::NotLocked {
        message: "no dependency `b`".to_string(),
    });
    assert!(
        matches!(
            &error,
            CompileError::Eval(RenderedGraphcalError { error: GraphcalError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Evaluation(EvaluationError::Failed { message, .. }), .. }), .. })
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
            CompileError::Load(LoadError::ManifestError { ref message }) if message == "lockfile package `b` is missing"
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
        matches!(error, CompileError::Load(LoadError::FileRootSelfImport { ref path, .. }) if path == "pkg.main"),
        "{error:?}"
    );

    let error = build_error(snapshot(
        "main",
        [fetched("main", "import main::{x};", &[])],
    ));
    assert!(
        matches!(error, CompileError::Load(LoadError::FileRootSelfImport { ref path, .. }) if path == "main"),
        "{error:?}"
    );
}

#[test]
fn self_references_through_inline_paths_and_includes_are_allowed() {
    let files = build_files(snapshot(
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
    assert_eq!(
        target.target(),
        &dag_id("main").inline_dag_child(
            graphcal_compiler::syntax::decl_name::DeclName::expect_valid("inner")
        )
    );

    let files = build_files(snapshot(
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
    let files = build_files(snapshot(
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
fn outside_root_is_rejected_at_file_root_and_in_dag_bodies() {
    for text in ["import pkg.b::{y};", "dag inner { import pkg.b::{y}; }"] {
        let error = build_error(snapshot(
            "main",
            [fetched(
                "main",
                text,
                &[("pkg.b", ModuleResolution::OutsideRoot)],
            )],
        ));
        assert!(
            matches!(error, CompileError::Load(LoadError::ImportOutsideRoot { ref path, .. }) if path == "pkg.b"),
            "{error:?}"
        );
    }
}

#[test]
fn dag_body_imports_load_dependencies_and_keep_failures_unresolved() {
    let files = build_files(snapshot(
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
    assert_eq!(
        inner.dag_id,
        dag_id("main").inline_dag_child(
            graphcal_compiler::syntax::decl_name::DeclName::expect_valid("inner")
        )
    );
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
    let files = build_files(snapshot(
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
        .find(|dag| {
            dag.dag_id
                .leaf()
                .inline_dag()
                .is_some_and(|name| name.as_str() == "b")
        })
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
        sources: graphcal_compiler::source_registry::SourceRegistry::new(),
    });
    assert!(
        error
            .to_string()
            .contains("invalid module path `src/main.txt`"),
        "{error}"
    );
}

/// Build the loaded files of `snapshot`, without its source registry.
fn build_files<K: SourceKey>(
    snapshot: SourceSnapshot<K>,
) -> Result<DependencyOrdered<LoadedFile>, CompileError> {
    build_loaded_files(snapshot).map(|(files, _)| files)
}
