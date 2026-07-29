//! Import-surface classification for Graphcal files.
//!
//! This module is shared by the project pipeline and inline-DAG self-import
//! preprocessing. Keeping it outside `eval::project` avoids a circular module
//! dependency while preserving one source of truth for import namespace and
//! visibility rules.

use graphcal_compiler::desugar::desugared_ast::{
    DeclKind, Declaration, File, TypeDecl, TypeDeclBody,
};
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
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

pub fn decl_is_explicit_export(decl: &Declaration) -> bool {
    match &decl.kind {
        // Params carry the distinct input-port role. Plugin imports carry no
        // visibility and their functions are never re-exported.
        DeclKind::Param(_) | DeclKind::PluginImport(_) => false,
        DeclKind::Node(d) | DeclKind::ConstNode(d) => d.visibility.is_public(),
        DeclKind::BaseDimension(d) => d.visibility.is_public(),
        DeclKind::Dimension(d) => d.visibility.is_public(),
        DeclKind::Unit(d) => d.visibility.is_public(),
        DeclKind::Type(d) => d.visibility.is_public(),
        DeclKind::Index(d) => d.visibility.is_public(),
        DeclKind::Import(d) => d.visibility.is_public(),
        DeclKind::Include(d) => d.visibility.is_public(),
        DeclKind::Dag(d) => d.visibility.is_public(),
        DeclKind::Assert(d) => d.visibility.is_public(),
        DeclKind::Plot(d) => d.visibility.is_public(),
        DeclKind::Figure(d) => d.visibility.is_public(),
        DeclKind::Layer(d) => d.visibility.is_public(),
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    }
}

/// Whether the declaration is externally addressable as an explicit export or
/// param input port.
pub fn decl_has_external_role(decl: &Declaration) -> bool {
    matches!(&decl.kind, DeclKind::Param(_)) || decl_is_explicit_export(decl)
}

/// Classify the declarations addressable at a file's external boundary.
///
/// Explicitly `pub`/`pub(bind)` declarations and selective re-exports carry
/// the explicit-export role. Params carry the distinct annotation-free input-
/// port role, while their effective values remain externally selectable as
/// outputs. Whole-module re-exports are expanded transitively during processing,
/// because this pure AST walk does not have the dependency's external surface.
pub fn extract_external_decl_surface(file: &File) -> ExternalDeclSurface {
    let mut surface = ExternalDeclSurface::default();
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Param(param) => {
                surface.insert_input_port(param.name.value.clone());
            }
            DeclKind::Import(d) if !decl_is_explicit_export(decl) => {
                if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                    &d.kind
                {
                    for item in items {
                        if item.is_pub {
                            surface
                                .insert_explicit_export(DeclName::expect_valid(item.local_name()));
                        }
                    }
                }
            }
            DeclKind::Include(d) if !decl_is_explicit_export(decl) => {
                if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                    &d.kind
                {
                    for item in items {
                        if item.is_pub {
                            surface
                                .insert_explicit_export(DeclName::expect_valid(item.local_name()));
                        }
                    }
                }
            }
            _ if decl_is_explicit_export(decl) => {
                if let Some(identity) = decl_identity(decl) {
                    surface.insert_explicit_export(DeclName::expect_valid(identity.name));
                }
            }
            _ => {}
        }
    }
    surface
}

/// External-role-aware result of looking up an importable item in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportItemPresence {
    Missing,
    Private,
    ExplicitExport,
    InputPort,
}

impl ImportItemPresence {
    pub const fn is_present(self) -> bool {
        !matches!(self, Self::Missing)
    }

    pub const fn can_select_output(self) -> bool {
        matches!(self, Self::ExplicitExport | Self::InputPort)
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
                (ImportItemPresence::ExplicitExport, _)
                | (_, ImportItemPresence::ExplicitExport) => ImportItemPresence::ExplicitExport,
                (ImportItemPresence::InputPort, _) | (_, ImportItemPresence::InputPort) => {
                    ImportItemPresence::InputPort
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
                ImportItemPresence::ExplicitExport
            } else {
                ImportItemPresence::Private
            }
        }),
        DeclKind::Param(param) => (namespace == ImportItemNamespace::Default
            && param.name.value.as_str() == name)
            .then_some(ImportItemPresence::InputPort),
        DeclKind::Node(_)
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
                if decl_is_explicit_export(decl) {
                    ImportItemPresence::ExplicitExport
                } else {
                    ImportItemPresence::Private
                }
            }),
        DeclKind::Import(d) => selective_reexport_matches(&d.kind, name, namespace)
            .then_some(ImportItemPresence::ExplicitExport),
        DeclKind::Include(d) => (namespace == ImportItemNamespace::Default
            && selective_include_reexport_matches(&d.kind, name))
        .then_some(ImportItemPresence::ExplicitExport),
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

pub fn file_exposes_import_item(file: &File, name: &str, namespace: ImportItemNamespace) -> bool {
    file_import_item_presence(file, name, namespace).can_select_output()
}

const fn import_namespace_matches(kind: ProjectDeclKind, namespace: ImportItemNamespace) -> bool {
    match namespace {
        ImportItemNamespace::Type => matches!(kind, ProjectDeclKind::Type),
        ImportItemNamespace::Default => !matches!(kind, ProjectDeclKind::Type),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::syntax::parser::Parser;

    fn parse(source: &str) -> File {
        let parsed = Parser::new(source).parse_file().expect("source parses");
        graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(parsed)
    }

    #[test]
    fn external_surface_keeps_param_input_ports_separate_from_explicit_exports() {
        let file = parse(
            "param input: Dimensionless = 1.0;\n\
             pub node output: Dimensionless = @input;\n\
             node helper: Dimensionless = @input;\n",
        );
        let surface = extract_external_decl_surface(&file);
        let input = DeclName::expect_valid("input");
        let output = DeclName::expect_valid("output");
        let helper = DeclName::expect_valid("helper");

        assert!(surface.is_input_port(&input));
        assert!(surface.is_externally_nameable(&input));
        assert!(surface.can_select_output(&input));
        assert!(!surface.is_explicit_export(&input));
        assert!(surface.is_explicit_export(&output));
        assert!(surface.can_select_output(&output));
        assert!(!surface.is_input_port(&output));
        assert!(!surface.is_externally_nameable(&helper));

        assert_eq!(
            file_import_item_presence(&file, "input", ImportItemNamespace::Default),
            ImportItemPresence::InputPort
        );
        assert_eq!(
            file_import_item_presence(&file, "output", ImportItemNamespace::Default),
            ImportItemPresence::ExplicitExport
        );
        assert_eq!(
            file_import_item_presence(&file, "helper", ImportItemNamespace::Default),
            ImportItemPresence::Private
        );
    }
}
