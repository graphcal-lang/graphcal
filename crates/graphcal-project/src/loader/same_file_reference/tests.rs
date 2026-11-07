use super::*;

use graphcal_compiler::desugar::desugared_ast::File;
use graphcal_compiler::syntax::parser::Parser;

const SOURCE: &str = "\
dag engine {
    import a::{x};
    dag rocket {
        import b::{y};
    }
    dag nozzle {
        import c::{z};
    }
}
dag pump {
    import d::{w};
}
";

fn declarations() -> Vec<Declaration> {
    File::from(Parser::new(SOURCE).parse_file().unwrap()).declarations
}

fn atom(name: &str) -> NameAtom {
    NameAtom::parse(name).unwrap()
}

fn dag_path(names: &[&str]) -> Vec<DeclName> {
    names
        .iter()
        .map(|name| DeclName::expect_valid(*name))
        .collect()
}

/// The `ModulePath` of `import <spelling>::{v};`.
fn module_path(spelling: &str) -> ModulePath {
    let file = File::from(
        Parser::new(&format!("import {spelling}::{{v}};"))
            .parse_file()
            .unwrap(),
    );
    match &file.declarations[0].kind {
        DeclKind::Import(import) => import.path().clone(),
        other => panic!("expected an import, got {other:?}"),
    }
}

fn scope(stem: StemReference) -> SameFileScope {
    SameFileScope::new(&declarations(), stem)
}

#[test]
fn file_root_names_only_top_level_dags() {
    let scope = scope(StemReference::Unspellable);
    assert_eq!(
        scope.classify(&module_path("engine"), LexicalSite::FileRoot),
        PathScope::SameFile(SameFileModule::InlineDag(dag_path(&["engine"])))
    );
    // A nested DAG is not visible from the file root: the path is resolved
    // as a package path (#2242).
    for nested in ["rocket", "nozzle"] {
        assert_eq!(
            scope.classify(&module_path(nested), LexicalSite::FileRoot),
            PathScope::Package,
            "{nested}"
        );
    }
}

#[test]
fn dag_body_names_children_then_siblings() {
    let scope = scope(StemReference::Unspellable);
    let engine = dag_path(&["engine"]);
    let rocket = dag_path(&["engine", "rocket"]);
    assert_eq!(
        scope.classify(&module_path("rocket"), LexicalSite::DagBody(&engine)),
        PathScope::SameFile(SameFileModule::InlineDag(rocket.clone()))
    );
    assert_eq!(
        scope.classify(&module_path("nozzle"), LexicalSite::DagBody(&rocket)),
        PathScope::SameFile(SameFileModule::InlineDag(dag_path(&["engine", "nozzle"])))
    );
    // `pump` is neither a child nor a sibling of `engine.rocket`.
    assert_eq!(
        scope.classify(&module_path("pump"), LexicalSite::DagBody(&rocket)),
        PathScope::Package
    );
    assert_eq!(
        scope.classify(&module_path("pump"), LexicalSite::DagBody(&engine)),
        PathScope::SameFile(SameFileModule::InlineDag(dag_path(&["pump"])))
    );
}

#[test]
fn multi_segment_paths_are_package_paths() {
    let scope = scope(StemReference::SelfReference(atom("engine")));
    assert_eq!(
        scope.classify(&module_path("engine.rocket"), LexicalSite::FileRoot),
        PathScope::Package
    );
}

#[test]
fn stem_self_reference_names_the_file_root_after_dags() {
    let scope = scope(StemReference::SelfReference(atom("main")));
    let engine = dag_path(&["engine"]);
    for site in [LexicalSite::FileRoot, LexicalSite::DagBody(&engine)] {
        assert_eq!(
            scope.classify(&module_path("main"), site),
            PathScope::SameFile(SameFileModule::FileRoot)
        );
    }
    // A same-named DAG takes precedence over the stem.
    let shadowed = SameFileScope::new(
        &declarations(),
        StemReference::SelfReference(atom("engine")),
    );
    assert_eq!(
        shadowed.classify(&module_path("engine"), LexicalSite::FileRoot),
        PathScope::SameFile(SameFileModule::InlineDag(engine))
    );
}

#[test]
fn package_stem_is_a_package_path_at_file_root_and_rejected_in_dag_bodies() {
    let own_path = Some(ModulePathKey::new(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(vec![
            atom("nasa"),
            atom("nasa"),
        ])
        .unwrap(),
    ));
    let scope = scope(StemReference::RejectedInDagBody {
        stem: atom("nasa"),
        module_path: own_path.clone(),
    });
    let engine = dag_path(&["engine"]);
    assert_eq!(
        scope.classify(&module_path("nasa"), LexicalSite::FileRoot),
        PathScope::Package
    );
    assert_eq!(
        scope.classify(&module_path("nasa"), LexicalSite::DagBody(&engine)),
        PathScope::RejectedStem {
            module_path: own_path
        }
    );
    // A visible DAG of the same name still takes precedence.
    let shadowed = SameFileScope::new(
        &declarations(),
        StemReference::RejectedInDagBody {
            stem: atom("rocket"),
            module_path: None,
        },
    );
    assert_eq!(
        shadowed.classify(&module_path("rocket"), LexicalSite::DagBody(&engine)),
        PathScope::SameFile(SameFileModule::InlineDag(dag_path(&["engine", "rocket"])))
    );
}
