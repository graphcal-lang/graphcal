//! Which dependency paths of one file name a module of that same file.
//!
//! A one-segment `import` / `include` path is classified from the place it is
//! written, before any package resolution:
//!
//! - At file root it names a top-level `dag` of the file.
//! - In a DAG body it names a child `dag` of that DAG, or a sibling (a child
//!   of the DAG's lexical parent).
//! - In a standalone (virtual-package) file, the file's own stem names the
//!   file root unless one of the DAGs above takes precedence.
//!
//! Every other path names a module through package resolution. In a package
//! file the stem is an ordinary first path segment (the package name or a
//! dependency alias); a DAG body that spells its own file's stem there is
//! rejected rather than resolved (M037), because standalone files and earlier
//! releases read that spelling as the file itself.

use std::collections::HashSet;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::Declaration;
use graphcal_compiler::syntax::ast::{DeclKind, ModulePath};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_path_key::ModulePathKey;
use graphcal_compiler::syntax::names::NameAtom;

use super::module_path::ModuleTarget;

/// How a file refers to its own root by a one-segment path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StemReference {
    /// A standalone file: its stem names its own root.
    SelfReference(NameAtom),
    /// A package file: its stem is not a self-reference, and a DAG body path
    /// spelled as the stem is rejected.
    RejectedInDagBody {
        stem: NameAtom,
        /// The module path that does name the file, when it has one.
        module_path: Option<ModulePathKey>,
    },
    /// The stem is not a name, so no one-segment path can spell it.
    Unspellable,
}

/// The place an import/include path is written within its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LexicalSite<'a> {
    /// A file-root declaration.
    FileRoot,
    /// The body of the inline DAG at this path from the file root (non-empty).
    DagBody(&'a [DeclName]),
}

/// A module of the declaring file named by a same-file reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SameFileModule {
    /// The file root (a standalone file's stem).
    FileRoot,
    /// The inline DAG at this path from the file root (non-empty).
    InlineDag(Vec<DeclName>),
}

impl SameFileModule {
    /// The exact module target once the file has the identity `file`.
    pub(super) fn target_in(&self, file: &DagId) -> ModuleTarget {
        match self {
            Self::FileRoot => ModuleTarget::file_root(file.clone()),
            Self::InlineDag(path) => ModuleTarget::in_file(
                file.clone(),
                path.iter().fold(file.clone(), |owner, name| {
                    owner.inline_dag_child(name.clone())
                }),
            ),
        }
    }
}

/// What a dependency path names, decided from where it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PathScope {
    /// A module of the declaring file; never resolved or loaded.
    SameFile(SameFileModule),
    /// A module found by package resolution (possibly in this same file, when
    /// the path spells the file's full module path).
    Package,
    /// A DAG-body path of a package file spelled as the file's own stem
    /// ([`StemReference::RejectedInDagBody`]), with the module path that does
    /// name the file, when it has one.
    RejectedStem { module_path: Option<ModulePathKey> },
}

/// The same-file names one file's dependency paths can spell.
#[derive(Debug, Clone)]
pub(super) struct SameFileScope {
    /// Path from the file root of every inline DAG the file declares.
    dags: HashSet<Vec<DeclName>>,
    stem: StemReference,
}

impl SameFileScope {
    /// The scope of the file whose top-level declarations are `declarations`.
    pub(super) fn new(declarations: &[Declaration], stem: StemReference) -> Self {
        let mut dags = HashSet::new();
        collect_dag_paths(declarations, &mut Vec::new(), &mut dags);
        Self { dags, stem }
    }

    /// Classify `path`, written at `site` of this file.
    pub(super) fn classify(&self, path: &ModulePath, site: LexicalSite<'_>) -> PathScope {
        let [segment] = path.segments() else {
            return PathScope::Package;
        };
        let name = DeclName::classify(segment.name.atom().clone());
        if let Some(dag) = self.visible_dag(&name, site) {
            return PathScope::SameFile(SameFileModule::InlineDag(dag));
        }
        match (&self.stem, site) {
            (StemReference::SelfReference(stem), _) if stem == segment.name.atom() => {
                PathScope::SameFile(SameFileModule::FileRoot)
            }
            (StemReference::RejectedInDagBody { stem, module_path }, LexicalSite::DagBody(_))
                if stem == segment.name.atom() =>
            {
                PathScope::RejectedStem {
                    module_path: module_path.clone(),
                }
            }
            _ => PathScope::Package,
        }
    }

    /// The path of the DAG `name` visible from `site`: a top-level DAG from
    /// the file root; a child, then a sibling, from a DAG body.
    fn visible_dag(&self, name: &DeclName, site: LexicalSite<'_>) -> Option<Vec<DeclName>> {
        let candidate = |owner: &[DeclName]| {
            let mut path = owner.to_vec();
            path.push(name.clone());
            self.dags.contains(&path).then_some(path)
        };
        match site {
            LexicalSite::FileRoot => candidate(&[]),
            LexicalSite::DagBody(owner) => candidate(owner)
                .or_else(|| owner.split_last().and_then(|(_, parent)| candidate(parent))),
        }
    }
}

fn collect_dag_paths(
    declarations: &[Declaration],
    owner: &mut Vec<DeclName>,
    out: &mut HashSet<Vec<DeclName>>,
) {
    for declaration in declarations {
        if let DeclKind::Dag(dag) = &declaration.kind {
            owner.push(dag.name.value.clone());
            out.insert(owner.clone());
            collect_dag_paths(&dag.body, owner, out);
            owner.pop();
        }
    }
}

#[cfg(test)]
mod tests;
