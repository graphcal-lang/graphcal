//! Import-surface classification for Graphcal files.
//!
//! This module is shared by the pure project compiler and inline-DAG self-import
//! preprocessing. Keeping it outside `project_compiler` avoids a circular module
//! dependency while preserving one source of truth for import namespace and
//! visibility rules.

use graphcal_compiler::desugar::desugared_ast::{
    DeclKind, Declaration, File, TypeDecl, TypeDeclBody,
};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::import_category::ImportItemCategoryMismatch;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;
use miette::NamedSource;
use std::sync::Arc;

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
    pub name: &'a NameAtom,
    pub kind: ProjectDeclKind,
}

pub fn decl_identity(decl: &Declaration) -> Option<ProjectDeclIdentity<'_>> {
    let (name, kind) = match &decl.kind {
        DeclKind::Param(p) => (p.name.value.atom(), ProjectDeclKind::Param),
        DeclKind::Node(n) => (n.name.value.atom(), ProjectDeclKind::Node),
        DeclKind::ConstNode(c) => (c.name.value.atom(), ProjectDeclKind::Const),
        DeclKind::Assert(a) => (a.name.value.atom(), ProjectDeclKind::Assert),
        DeclKind::BaseDimension(d) => (d.name.value.atom(), ProjectDeclKind::Dimension),
        DeclKind::Dimension(d) => (d.name.value.atom(), ProjectDeclKind::Dimension),
        DeclKind::Unit(u) => (u.name.value.atom(), ProjectDeclKind::Unit),
        DeclKind::Index(idx) => (idx.name.value.atom(), ProjectDeclKind::Index),
        DeclKind::Type(t) => (t.name.value.atom(), ProjectDeclKind::Type),
        DeclKind::Plot(p) => (p.name.value.atom(), ProjectDeclKind::Plot),
        DeclKind::Figure(f) => (f.name.value.atom(), ProjectDeclKind::Figure),
        DeclKind::Layer(l) => (l.name.value.atom(), ProjectDeclKind::Layer),
        DeclKind::Dag(d) => (d.name.value.atom(), ProjectDeclKind::Dag),
        DeclKind::Import(_) | DeclKind::PluginImport(_) | DeclKind::Include(_) => return None,
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    };
    Some(ProjectDeclIdentity { name, kind })
}

pub fn decl_is_explicit_export(decl: &Declaration) -> bool {
    match &decl.kind {
        // Params carry the distinct input-port role. Import/include visibility
        // exists only on selected items; plugin functions are never re-exported.
        DeclKind::Param(_)
        | DeclKind::Import(_)
        | DeclKind::Include(_)
        | DeclKind::PluginImport(_) => false,
        DeclKind::Node(d) | DeclKind::ConstNode(d) => d.visibility.is_public(),
        DeclKind::BaseDimension(d) => d.visibility.is_public(),
        DeclKind::Dimension(d) => d.visibility.is_public(),
        DeclKind::Unit(d) => d.visibility.is_public(),
        DeclKind::Type(d) => d.visibility.is_public(),
        DeclKind::Index(d) => d.visibility.is_public(),
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
/// outputs. Namespaced imports/includes are private use-sites and never widen
/// this surface.
pub fn extract_external_decl_surface(file: &File) -> ExternalDeclSurface {
    extract_external_decl_surface_from_declarations(&file.declarations)
}

pub fn extract_external_decl_surface_from_declarations(
    declarations: &[Declaration],
) -> ExternalDeclSurface {
    let mut surface = ExternalDeclSurface::default();
    for decl in declarations {
        match &decl.kind {
            DeclKind::Param(param) => {
                surface.insert_input_port(param.name.value.clone());
            }
            DeclKind::Import(d) => {
                if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                    &d.kind
                {
                    for item in items {
                        if item.is_pub {
                            surface.insert_explicit_export(DeclName::from_atom(
                                item.local_name_atom().clone(),
                            ));
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
                            surface.insert_explicit_export(DeclName::from_atom(
                                item.local_name_atom().clone(),
                            ));
                        }
                    }
                }
            }
            _ if decl_is_explicit_export(decl) => {
                if let Some(identity) = decl_identity(decl) {
                    surface.insert_explicit_export(DeclName::from_atom(identity.name.clone()));
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

/// Compile-time policy for a term selected by a pure import.
///
/// Both cross-file imports and an inline DAG's `import <self>.{...}` consume
/// this classification. Keeping the permitted actions and rejection reasons
/// exhaustive prevents either path from silently accepting a new declaration
/// kind when the language grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureImportTermDisposition {
    /// Materialize a compile-time constant binding in the importer.
    BindConstant,
    /// Keep the name available only to compile-time/type-system resolution,
    /// such as an algebraic constructor or DAG blueprint.
    ResolverOnly,
    /// Reject a term that requires a concrete instance boundary.
    Reject(PureImportRejection),
}

/// Why a term cannot cross a pure-import boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureImportRejection {
    Runtime,
    Assertion,
    Visualization,
}

impl PureImportRejection {
    /// Build the diagnostic shared by cross-file and inline-self imports.
    pub fn diagnostic(
        self,
        name: &NameAtom,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> GraphcalError {
        let name = name.to_string();
        let src = src.clone();
        let span = span.into();
        match self {
            Self::Runtime => GraphcalError::ImportRuntimeItem { name, src, span },
            Self::Assertion => GraphcalError::ImportAssertionItem { name, src, span },
            Self::Visualization => GraphcalError::ImportPlotItem { name, src, span },
        }
    }
}

impl PureImportTermDisposition {
    const fn precedence(self) -> u8 {
        match self {
            Self::Reject(PureImportRejection::Visualization) => 4,
            Self::Reject(PureImportRejection::Runtime) => 3,
            Self::Reject(PureImportRejection::Assertion) => 2,
            Self::BindConstant => 1,
            Self::ResolverOnly => 0,
        }
    }

    const fn combine(self, other: Self) -> Self {
        if self.precedence() >= other.precedence() {
            self
        } else {
            other
        }
    }
}

/// Classify a term by the action a pure import must take.
///
/// `None` means no declaration occupies the term namespace. Visibility is an
/// independent boundary and must be checked before applying this policy.
pub fn pure_import_term_disposition(
    declarations: &[Declaration],
    name: &str,
) -> Option<PureImportTermDisposition> {
    declarations
        .iter()
        .filter_map(|decl| decl_pure_import_term_disposition(decl, name))
        .reduce(PureImportTermDisposition::combine)
}

fn decl_pure_import_term_disposition(
    decl: &Declaration,
    name: &str,
) -> Option<PureImportTermDisposition> {
    match &decl.kind {
        DeclKind::ConstNode(constant) if constant.name.value.as_str() == name => {
            Some(PureImportTermDisposition::BindConstant)
        }
        DeclKind::Param(param) if param.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Runtime),
        ),
        DeclKind::Node(node) if node.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Runtime),
        ),
        DeclKind::Assert(assertion) if assertion.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Assertion),
        ),
        DeclKind::Plot(plot) if plot.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Visualization),
        ),
        DeclKind::Figure(figure) if figure.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Visualization),
        ),
        DeclKind::Layer(layer) if layer.name.value.as_str() == name => Some(
            PureImportTermDisposition::Reject(PureImportRejection::Visualization),
        ),
        DeclKind::Dag(dag) if dag.name.value.as_str() == name => {
            Some(PureImportTermDisposition::ResolverOnly)
        }
        DeclKind::Type(type_decl)
            if type_decl_import_item_matches(type_decl, name, ImportItemNamespace::Term) =>
        {
            Some(PureImportTermDisposition::ResolverOnly)
        }
        DeclKind::Import(import)
            if selective_reexport_matches(&import.kind, name, ImportItemNamespace::Term) =>
        {
            Some(PureImportTermDisposition::ResolverOnly)
        }
        DeclKind::Include(include) if selective_include_reexport_matches(&include.kind, name) => {
            Some(PureImportTermDisposition::ResolverOnly)
        }
        DeclKind::Sugar(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
        DeclKind::Param(_)
        | DeclKind::Node(_)
        | DeclKind::ConstNode(_)
        | DeclKind::Assert(_)
        | DeclKind::BaseDimension(_)
        | DeclKind::Dimension(_)
        | DeclKind::Unit(_)
        | DeclKind::Index(_)
        | DeclKind::Type(_)
        | DeclKind::Plot(_)
        | DeclKind::Figure(_)
        | DeclKind::Layer(_)
        | DeclKind::Dag(_)
        | DeclKind::Import(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Include(_) => None,
    }
}

pub fn file_import_item_presence(
    file: &File,
    name: &str,
    namespace: ImportItemNamespace,
) -> ImportItemPresence {
    declarations_import_item_presence(&file.declarations, name, namespace)
}

pub fn declarations_import_item_presence(
    declarations: &[Declaration],
    name: &str,
    namespace: ImportItemNamespace,
) -> ImportItemPresence {
    declarations
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
        DeclKind::Param(param) => (namespace == ImportItemNamespace::Term
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
        DeclKind::Include(d) => (namespace == ImportItemNamespace::Term
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
        || (namespace == ImportItemNamespace::Term
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

/// Import categories in which `name` exists, in canonical marker order.
#[cfg(test)]
pub fn file_import_item_namespaces(
    file: &File,
    name: &str,
) -> Option<graphcal_compiler::syntax::non_empty::NonEmpty<ImportItemNamespace>> {
    declarations_import_item_namespaces(&file.declarations, name)
}

pub fn declarations_import_item_namespaces(
    declarations: &[Declaration],
    name: &str,
) -> Option<graphcal_compiler::syntax::non_empty::NonEmpty<ImportItemNamespace>> {
    let namespaces = [
        ImportItemNamespace::Term,
        ImportItemNamespace::Type,
        ImportItemNamespace::Dimension,
        ImportItemNamespace::Unit,
        ImportItemNamespace::Index,
    ]
    .into_iter()
    .filter(|namespace| {
        declarations_import_item_presence(declarations, name, *namespace).is_present()
    })
    .collect();
    graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(namespaces).ok()
}

pub fn import_item_not_found_error(
    file: &File,
    name: &NameAtom,
    expected: ImportItemNamespace,
    file_path: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    import_item_not_found_error_from_declarations(
        &file.declarations,
        name,
        expected,
        file_path,
        src,
        span,
    )
}

pub fn import_item_not_found_error_from_declarations(
    declarations: &[Declaration],
    name: &NameAtom,
    expected: ImportItemNamespace,
    file_path: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    declarations_import_item_namespaces(declarations, name.as_str()).map_or_else(
        || GraphcalError::ImportNameNotFound {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src: src.clone(),
            span: span.into(),
        },
        |alternatives| GraphcalError::ImportCategoryMismatch {
            file_path: file_path.to_string(),
            mismatch: ImportItemCategoryMismatch::new(name.clone(), expected, alternatives),
            src: src.clone(),
            span: span.into(),
        },
    )
}

pub fn declarations_have_import_item(
    declarations: &[Declaration],
    name: &str,
    namespace: ImportItemNamespace,
) -> bool {
    declarations_import_item_presence(declarations, name, namespace).is_present()
}

pub fn declarations_expose_import_item(
    declarations: &[Declaration],
    name: &str,
    namespace: ImportItemNamespace,
) -> bool {
    declarations_import_item_presence(declarations, name, namespace).can_select_output()
}

const fn import_namespace_matches(kind: ProjectDeclKind, namespace: ImportItemNamespace) -> bool {
    match namespace {
        ImportItemNamespace::Term => matches!(
            kind,
            ProjectDeclKind::Param
                | ProjectDeclKind::Node
                | ProjectDeclKind::Const
                | ProjectDeclKind::Assert
                | ProjectDeclKind::Plot
                | ProjectDeclKind::Figure
                | ProjectDeclKind::Layer
                | ProjectDeclKind::Dag
        ),
        ImportItemNamespace::Type => matches!(kind, ProjectDeclKind::Type),
        ImportItemNamespace::Dimension => matches!(kind, ProjectDeclKind::Dimension),
        ImportItemNamespace::Unit => matches!(kind, ProjectDeclKind::Unit),
        ImportItemNamespace::Index => matches!(kind, ProjectDeclKind::Index),
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
    fn import_item_namespaces_preserve_legal_same_name_categories() {
        let file = parse(
            "pub const node JPY: Dimensionless = 1.0;\n\
             pub base unit JPY: Dimensionless;\n\
             pub type Student { Student }\n",
        );

        assert_eq!(
            file_import_item_namespaces(&file, "JPY")
                .expect("JPY categories")
                .as_slice(),
            &[ImportItemNamespace::Term, ImportItemNamespace::Unit]
        );
        assert_eq!(
            file_import_item_namespaces(&file, "Student")
                .expect("Student categories")
                .as_slice(),
            &[ImportItemNamespace::Term, ImportItemNamespace::Type]
        );
    }

    #[test]
    fn pure_term_policy_classifies_every_supported_declaration_role() {
        let file = parse(
            "pub const node constant: Dimensionless = 1.0;\n\
             param parameter: Dimensionless = 1.0;\n\
             pub node runtime_node: Dimensionless = @parameter;\n\
             pub assert assertion = true;\n\
             pub plot chart = { mark: point, encode: { x: 1.0 } };\n\
             pub type Choice { Pick }\n\
             pub dag blueprint { pub node output: Dimensionless = 1.0; }\n",
        );

        let cases = [
            ("constant", PureImportTermDisposition::BindConstant),
            (
                "parameter",
                PureImportTermDisposition::Reject(PureImportRejection::Runtime),
            ),
            (
                "runtime_node",
                PureImportTermDisposition::Reject(PureImportRejection::Runtime),
            ),
            (
                "assertion",
                PureImportTermDisposition::Reject(PureImportRejection::Assertion),
            ),
            (
                "chart",
                PureImportTermDisposition::Reject(PureImportRejection::Visualization),
            ),
            ("Pick", PureImportTermDisposition::ResolverOnly),
            ("blueprint", PureImportTermDisposition::ResolverOnly),
        ];

        for (name, expected) in cases {
            assert_eq!(
                pure_import_term_disposition(&file.declarations, name),
                Some(expected),
                "unexpected pure-import disposition for {name}"
            );
        }
    }

    #[test]
    fn namespaced_use_sites_never_widen_the_external_surface() {
        let file = parse(
            "import pkg.core;\n\
             include pkg.engine() as engine;\n\
             import pkg.core.{ pub dim Exposed, helper };\n\
             include pkg.engine().{ pub thrust, fuel_flow };\n",
        );
        let surface = extract_external_decl_surface(&file);

        for exported in ["Exposed", "thrust"] {
            assert!(
                surface.is_explicit_export(&DeclName::expect_valid(exported)),
                "{exported} should be explicitly exported"
            );
        }
        for private in ["core", "engine", "helper", "fuel_flow"] {
            assert!(
                !surface.is_externally_nameable(&DeclName::expect_valid(private)),
                "{private} must remain private"
            );
        }
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
            file_import_item_presence(&file, "input", ImportItemNamespace::Term),
            ImportItemPresence::InputPort
        );
        assert_eq!(
            file_import_item_presence(&file, "output", ImportItemNamespace::Term),
            ImportItemPresence::ExplicitExport
        );
        assert_eq!(
            file_import_item_presence(&file, "helper", ImportItemNamespace::Term),
            ImportItemPresence::Private
        );
    }
}
