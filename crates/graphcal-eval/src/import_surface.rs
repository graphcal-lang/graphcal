//! Import-surface classification for Graphcal files.
//!
//! This module is shared by the project pipeline and inline-DAG self-import
//! preprocessing. Keeping it outside `eval::project` avoids a circular module
//! dependency while preserving one source of truth for import namespace and
//! visibility rules.

use std::collections::HashSet;

use graphcal_compiler::desugar::desugared_ast::{
    DeclKind, Declaration, File, TypeDecl, TypeDeclBody,
};
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::decl_name::DeclName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectDeclKind {
    Param,
    Node,
    Const,
    Assert,
    Dimension,
    Unit,
    Index,
    Type,
    Plot,
    Figure,
    Layer,
    Dag,
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectDeclIdentity<'a> {
    pub name: &'a str,
    pub kind: ProjectDeclKind,
}

pub fn decl_identity(decl: &Declaration) -> Option<ProjectDeclIdentity<'_>> {
    let (name, kind) = match &decl.kind {
        DeclKind::Param(p) => (p.name.value.as_str(), ProjectDeclKind::Param),
        DeclKind::Node(n) => (n.name.value.as_str(), ProjectDeclKind::Node),
        DeclKind::ConstNode(c) => (c.name.value.as_str(), ProjectDeclKind::Const),
        DeclKind::Assert(a) => (a.name.value.as_str(), ProjectDeclKind::Assert),
        DeclKind::BaseDimension(d) => (d.name.value.as_str(), ProjectDeclKind::Dimension),
        DeclKind::Dimension(d) => (d.name.value.as_str(), ProjectDeclKind::Dimension),
        DeclKind::Unit(u) => (u.name.value.as_str(), ProjectDeclKind::Unit),
        DeclKind::Index(idx) => (idx.name.value.as_str(), ProjectDeclKind::Index),
        DeclKind::Type(t) => (t.name.value.as_str(), ProjectDeclKind::Type),
        DeclKind::Plot(p) => (p.name.value.as_str(), ProjectDeclKind::Plot),
        DeclKind::Figure(f) => (f.name.value.as_str(), ProjectDeclKind::Figure),
        DeclKind::Layer(l) => (l.name.value.as_str(), ProjectDeclKind::Layer),
        DeclKind::Dag(d) => (d.name.value.as_str(), ProjectDeclKind::Dag),
        DeclKind::Import(_) | DeclKind::PluginImport(_) | DeclKind::Include(_) => return None,
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    };
    Some(ProjectDeclIdentity { name, kind })
}

pub fn decl_is_public(decl: &Declaration) -> bool {
    match &decl.kind {
        DeclKind::Param(_) => true,
        DeclKind::Node(d) | DeclKind::ConstNode(d) => d.visibility.is_public(),
        DeclKind::BaseDimension(d) => d.visibility.is_public(),
        DeclKind::Dimension(d) => d.visibility.is_public(),
        DeclKind::Unit(d) => d.visibility.is_public(),
        DeclKind::Type(d) => d.visibility.is_public(),
        DeclKind::Index(d) => d.visibility.is_public(),
        DeclKind::Import(d) => d.visibility.is_public(),
        // Plugin imports carry no visibility; their functions are only
        // callable through the alias and are never re-exported.
        DeclKind::PluginImport(_) => false,
        DeclKind::Include(d) => d.visibility.is_public(),
        DeclKind::Dag(d) => d.visibility.is_public(),
        DeclKind::Assert(d) => d.visibility.is_public(),
        DeclKind::Plot(d) => d.visibility.is_public(),
        DeclKind::Figure(d) => d.visibility.is_public(),
        DeclKind::Layer(d) => d.visibility.is_public(),
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    }
}

/// Extract the set of names visible to importers of a file.
///
/// Explicitly `pub`/`pub(bind)` declarations contribute. Params are
/// implicitly visible under the A5 rule ("params are always visible
/// and bindable") and therefore always contribute regardless of
/// annotation.
///
/// Selective `import "X" { pub name }` re-exports `name` at this file
/// per issue #452 — those names also contribute. Whole-module
/// `pub import "X";` / `pub include "X";` re-exports every `pub` item
/// from X; that form is resolved transitively during import processing
/// (the enumeration requires X's own `pub_names`, which this pure AST
/// walk does not have), so it is not expanded here.
pub fn extract_pub_names(file: &File) -> HashSet<DeclName> {
    let mut pub_names = HashSet::new();
    for decl in &file.declarations {
        if !decl_is_public(decl) {
            match &decl.kind {
                DeclKind::Import(d) => {
                    if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                        &d.kind
                    {
                        for item in items {
                            if item.is_pub {
                                pub_names.insert(DeclName::expect_valid(item.local_name()));
                            }
                        }
                    }
                }
                DeclKind::Include(d) => {
                    if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                        &d.kind
                    {
                        for item in items {
                            if item.is_pub {
                                pub_names.insert(DeclName::expect_valid(item.local_name()));
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some(identity) = decl_identity(decl) {
            pub_names.insert(DeclName::expect_valid(identity.name));
        }
    }
    pub_names
}

/// Visibility-aware result of looking up an importable item in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportItemPresence {
    Missing,
    Private,
    Public,
}

impl ImportItemPresence {
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Private | Self::Public)
    }

    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

pub fn file_import_item_presence(
    file: &File,
    name: &str,
    namespace: ImportItemNamespace,
) -> ImportItemPresence {
    file.declarations
        .iter()
        .filter_map(|decl| decl_import_item_presence(decl, name, namespace))
        .fold(ImportItemPresence::Missing, |acc, presence| {
            match (acc, presence) {
                (ImportItemPresence::Public, _) | (_, ImportItemPresence::Public) => {
                    ImportItemPresence::Public
                }
                (ImportItemPresence::Private, _) | (_, ImportItemPresence::Private) => {
                    ImportItemPresence::Private
                }
                (ImportItemPresence::Missing, ImportItemPresence::Missing) => {
                    ImportItemPresence::Missing
                }
            }
        })
}

fn decl_import_item_presence(
    decl: &Declaration,
    name: &str,
    namespace: ImportItemNamespace,
) -> Option<ImportItemPresence> {
    match &decl.kind {
        DeclKind::Type(t) => type_decl_import_item_matches(t, name, namespace).then(|| {
            if t.visibility.is_public() {
                ImportItemPresence::Public
            } else {
                ImportItemPresence::Private
            }
        }),
        DeclKind::Param(_)
        | DeclKind::Node(_)
        | DeclKind::ConstNode(_)
        | DeclKind::Assert(_)
        | DeclKind::BaseDimension(_)
        | DeclKind::Dimension(_)
        | DeclKind::Unit(_)
        | DeclKind::Index(_)
        | DeclKind::Plot(_)
        | DeclKind::Figure(_)
        | DeclKind::Layer(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Dag(_) => decl_identity(decl)
            .is_some_and(|identity| {
                import_namespace_matches(identity.kind, namespace) && identity.name == name
            })
            .then(|| {
                if decl_is_public(decl) {
                    ImportItemPresence::Public
                } else {
                    ImportItemPresence::Private
                }
            }),
        DeclKind::Import(d) => selective_reexport_matches(&d.kind, name, namespace)
            .then_some(ImportItemPresence::Public),
        DeclKind::Include(d) => (namespace == ImportItemNamespace::Default
            && selective_include_reexport_matches(&d.kind, name))
        .then_some(ImportItemPresence::Public),
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    }
}

fn type_decl_import_item_matches(
    type_decl: &TypeDecl,
    name: &str,
    namespace: ImportItemNamespace,
) -> bool {
    (namespace == ImportItemNamespace::Type && type_decl.name.value.as_str() == name)
        || (namespace == ImportItemNamespace::Default
            && match &type_decl.body {
                TypeDeclBody::Required => false,
                TypeDeclBody::Constructors(members) => members
                    .iter()
                    .any(|member| member.name.value.as_str() == name),
            })
}

fn selective_reexport_matches(
    kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    name: &str,
    namespace: ImportItemNamespace,
) -> bool {
    matches!(
        kind,
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items)
            if items.iter().any(|it| {
                it.is_pub && it.namespace == namespace && it.local_name() == name
            })
    )
}

fn selective_include_reexport_matches(
    kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    name: &str,
) -> bool {
    matches!(
        kind,
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items)
            if items.iter().any(|it| it.is_pub && it.local_name() == name)
    )
}

pub fn file_has_import_item(file: &File, name: &str, namespace: ImportItemNamespace) -> bool {
    file_import_item_presence(file, name, namespace).is_present()
}

pub fn file_exports_import_item(file: &File, name: &str, namespace: ImportItemNamespace) -> bool {
    file_import_item_presence(file, name, namespace).is_public()
}

const fn import_namespace_matches(kind: ProjectDeclKind, namespace: ImportItemNamespace) -> bool {
    match namespace {
        ImportItemNamespace::Type => matches!(kind, ProjectDeclKind::Type),
        ImportItemNamespace::Default => !matches!(kind, ProjectDeclKind::Type),
    }
}
