//! Pure transitive Static dependency analysis for external declaration surfaces.

use std::collections::{HashMap, HashSet};

use crate::desugar::desugared_ast::{DeclKind, Declaration, IndexExpr, TypeExpr, TypeExprKind};
use crate::ir::static_interface::{StaticInputKind, StaticRole, static_interface};
use crate::syntax::ast::{GenericArg, ImportItemNamespace};
use crate::syntax::names::{NameAtom, NamePath};

/// Namespace candidates carried by one unresolved Static reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticReferenceNamespaces {
    Exact(StaticInputKind),
    TypeOrDimension,
    IndexTypeOrDimension,
}

/// One structured Static name reference retained before module-aware resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticReference {
    path: NamePath,
    namespaces: StaticReferenceNamespaces,
}

impl StaticReference {
    #[must_use]
    pub const fn exact(name: NameAtom, kind: StaticInputKind) -> Self {
        Self {
            path: NamePath::local(name),
            namespaces: StaticReferenceNamespaces::Exact(kind),
        }
    }

    #[must_use]
    pub const fn exact_path(path: NamePath, kind: StaticInputKind) -> Self {
        Self {
            path,
            namespaces: StaticReferenceNamespaces::Exact(kind),
        }
    }

    #[must_use]
    pub const fn ambiguous(name: NameAtom, namespaces: StaticReferenceNamespaces) -> Self {
        Self {
            path: NamePath::local(name),
            namespaces,
        }
    }

    #[must_use]
    pub const fn ambiguous_path(path: NamePath, namespaces: StaticReferenceNamespaces) -> Self {
        Self { path, namespaces }
    }

    #[must_use]
    pub const fn path(&self) -> &NamePath {
        &self.path
    }

    #[must_use]
    pub const fn bare_name(&self) -> Option<&NameAtom> {
        self.path.as_bare()
    }

    #[must_use]
    pub const fn namespaces(&self) -> StaticReferenceNamespaces {
        self.namespaces
    }
}

fn collect_path(path: &NamePath, kind: StaticInputKind, references: &mut Vec<StaticReference>) {
    references.push(StaticReference::exact_path(path.clone(), kind));
}

fn collect_ambiguous_path(
    path: &NamePath,
    namespaces: StaticReferenceNamespaces,
    references: &mut Vec<StaticReference>,
) {
    references.push(StaticReference::ambiguous_path(path.clone(), namespaces));
}

/// Collect every bare Static name referenced by a type expression.
pub fn collect_type_expr_static_references(
    type_expr: &TypeExpr,
    references: &mut Vec<StaticReference>,
) {
    match &type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => match dim_expr.terms.as_slice() {
            [item] if item.term.power.is_none() => collect_ambiguous_path(
                &item.term.name.value,
                StaticReferenceNamespaces::TypeOrDimension,
                references,
            ),
            terms => {
                for item in terms {
                    collect_path(
                        &item.term.name.value,
                        StaticInputKind::Dimension,
                        references,
                    );
                }
            }
        },
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_static_references(base, references);
            for index in indexes {
                if let IndexExpr::Name(path) = index {
                    collect_path(&path.value, StaticInputKind::Index, references);
                }
            }
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            collect_path(&name.value, StaticInputKind::Type, references);
            for argument in generic_args {
                collect_generic_arg_static_references(argument, references);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for argument in generic_args {
                collect_generic_arg_static_references(argument, references);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            for argument in type_args {
                collect_type_expr_static_references(argument, references);
            }
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

/// Collect every bare Static name referenced by a generic argument.
pub fn collect_generic_arg_static_references(
    argument: &GenericArg<crate::syntax::phase::Desugared>,
    references: &mut Vec<StaticReference>,
) {
    match argument {
        GenericArg::Type(type_expr) => collect_type_expr_static_references(type_expr, references),
        GenericArg::Index(IndexExpr::Name(path)) => {
            collect_path(&path.value, StaticInputKind::Index, references);
        }
        GenericArg::Index(IndexExpr::Finite { .. } | IndexExpr::BareNat(_))
        | GenericArg::Nat(_) => {}
        GenericArg::Ambiguous(ambiguous) => {
            collect_ambiguous_generic_static_references(ambiguous, references);
        }
    }
}

fn collect_ambiguous_generic_static_references(
    argument: &crate::desugar::desugared_ast::AmbiguousGenericArg,
    references: &mut Vec<StaticReference>,
) {
    match argument {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(identifier) => {
            references.push(StaticReference::ambiguous(
                identifier.name.clone(),
                StaticReferenceNamespaces::IndexTypeOrDimension,
            ));
        }
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_static_references(operand, references);
            }
        }
    }
}

/// Collect Static dependencies from one declaration's semantic signature.
#[must_use]
pub fn declaration_static_references(kind: &DeclKind) -> Vec<StaticReference> {
    let mut references = Vec::new();
    match kind {
        DeclKind::Param(param) => {
            collect_type_expr_static_references(&param.type_ann, &mut references);
        }
        DeclKind::Node(node) => {
            collect_type_expr_static_references(&node.type_ann, &mut references);
        }
        DeclKind::ConstNode(constant) => {
            collect_type_expr_static_references(&constant.type_ann, &mut references);
        }
        DeclKind::Unit(unit) => {
            for item in &unit.dim_type.terms {
                collect_path(
                    &item.term.name.value,
                    StaticInputKind::Dimension,
                    &mut references,
                );
            }
        }
        DeclKind::Dimension(dimension) => {
            if let Some(definition) = &dimension.definition {
                for item in &definition.terms {
                    collect_path(
                        &item.term.name.value,
                        StaticInputKind::Dimension,
                        &mut references,
                    );
                }
            }
        }
        DeclKind::Type(type_decl) => {
            if let crate::desugar::desugared_ast::TypeDeclBody::Constructors(members) =
                &type_decl.body
            {
                for member in members {
                    for field in member.payload.iter().flatten() {
                        collect_type_expr_static_references(&field.type_ann, &mut references);
                    }
                }
            }
            for default in type_decl
                .generic_params
                .iter()
                .filter_map(|parameter| parameter.default.as_ref())
            {
                collect_generic_arg_static_references(default, &mut references);
            }
            let generic_parameters = type_decl
                .generic_params
                .iter()
                .map(|parameter| parameter.name.value.atom().clone())
                .collect::<HashSet<_>>();
            references.retain(|reference| {
                reference
                    .bare_name()
                    .is_none_or(|name| !generic_parameters.contains(name))
            });
        }
        DeclKind::Index(index) => {
            if let crate::desugar::desugared_ast::IndexDeclKind::RequiredCoordinate { dimension } =
                &index.kind
            {
                for item in &dimension.terms {
                    collect_path(
                        &item.term.name.value,
                        StaticInputKind::Dimension,
                        &mut references,
                    );
                }
            }
        }
        DeclKind::Dag(dag) => {
            references.extend(
                dag.body
                    .iter()
                    .flat_map(|declaration| declaration_static_references(&declaration.kind)),
            );
        }
        DeclKind::BaseDimension(_)
        | DeclKind::Assert(_)
        | DeclKind::Plot(_)
        | DeclKind::Figure(_)
        | DeclKind::Layer(_)
        | DeclKind::Import(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Include(_) => {}
        DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
    }
    references
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StaticDeclarationKey {
    kind: StaticInputKind,
    name: NameAtom,
}

impl StaticDeclarationKey {
    const fn new(kind: StaticInputKind, name: NameAtom) -> Self {
        Self { kind, name }
    }
}

/// First unresolved required Static dependency reached from an import target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredStaticDependency {
    kind: StaticInputKind,
    name: NameAtom,
}

impl RequiredStaticDependency {
    #[must_use]
    pub const fn kind(&self) -> StaticInputKind {
        self.kind
    }

    #[must_use]
    pub const fn name(&self) -> &NameAtom {
        &self.name
    }
}

fn declaration_name_in_namespace(
    declaration: &Declaration,
    name: &NameAtom,
    namespace: ImportItemNamespace,
) -> bool {
    match (&declaration.kind, namespace) {
        (DeclKind::Type(type_decl), ImportItemNamespace::Type) => {
            type_decl.name.value.atom() == name
        }
        (DeclKind::Type(type_decl), ImportItemNamespace::Term) => match &type_decl.body {
            crate::desugar::desugared_ast::TypeDeclBody::Required => false,
            crate::desugar::desugared_ast::TypeDeclBody::Constructors(members) => members
                .iter()
                .any(|member| member.name.value.atom() == name),
        },
        (DeclKind::BaseDimension(dimension), ImportItemNamespace::Dimension) => {
            dimension.name.value.atom() == name
        }
        (DeclKind::Dimension(dimension), ImportItemNamespace::Dimension) => {
            dimension.name.value.atom() == name
        }
        (DeclKind::Index(index), ImportItemNamespace::Index) => index.name.value.atom() == name,
        (DeclKind::Unit(unit), ImportItemNamespace::Unit) => unit.name.value.atom() == name,
        (DeclKind::Param(param), ImportItemNamespace::Term) => param.name.value.atom() == name,
        (DeclKind::Node(node), ImportItemNamespace::Term) => node.name.value.atom() == name,
        (DeclKind::ConstNode(constant), ImportItemNamespace::Term) => {
            constant.name.value.atom() == name
        }
        (DeclKind::Dag(dag), ImportItemNamespace::Term) => dag.name.value.atom() == name,
        (DeclKind::Assert(assertion), ImportItemNamespace::Term) => {
            assertion.name.value.atom() == name
        }
        (DeclKind::Plot(plot), ImportItemNamespace::Term) => plot.name.value.atom() == name,
        (DeclKind::Figure(figure), ImportItemNamespace::Term) => figure.name.value.atom() == name,
        (DeclKind::Layer(layer), ImportItemNamespace::Term) => layer.name.value.atom() == name,
        _ => false,
    }
}

fn reference_candidates(
    reference: &StaticReference,
    interfaces: &HashMap<StaticDeclarationKey, StaticRole>,
) -> Vec<StaticDeclarationKey> {
    let kinds: &[StaticInputKind] = match reference.namespaces {
        StaticReferenceNamespaces::Exact(StaticInputKind::Type) => &[StaticInputKind::Type],
        StaticReferenceNamespaces::Exact(StaticInputKind::Dimension) => {
            &[StaticInputKind::Dimension]
        }
        StaticReferenceNamespaces::Exact(StaticInputKind::Index) => &[StaticInputKind::Index],
        StaticReferenceNamespaces::TypeOrDimension => {
            &[StaticInputKind::Type, StaticInputKind::Dimension]
        }
        StaticReferenceNamespaces::IndexTypeOrDimension => &[
            StaticInputKind::Index,
            StaticInputKind::Type,
            StaticInputKind::Dimension,
        ],
    };
    kinds
        .iter()
        .filter_map(|kind| {
            reference
                .bare_name()
                .map(|name| StaticDeclarationKey::new(*kind, name.clone()))
        })
        .filter(|candidate| interfaces.contains_key(candidate))
        .collect()
}

/// Typed reason a declaration cannot cross a blueprint-only import boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticImportRejection {
    RequiredInput {
        kind: StaticInputKind,
        name: NameAtom,
    },
    UnresolvedDependency(RequiredStaticDependency),
}

/// Classify Static-input reasons that reject one direct or qualified import.
#[must_use]
pub fn static_import_rejection(
    declarations: &[Declaration],
    selected_name: &NameAtom,
    selected_namespace: ImportItemNamespace,
) -> Option<StaticImportRejection> {
    if let Some(interface) = declarations.iter().find_map(|declaration| {
        if !declaration_name_in_namespace(declaration, selected_name, selected_namespace) {
            return None;
        }
        static_interface(&declaration.kind)
    }) && interface.role() == StaticRole::RequiredInput
    {
        return Some(StaticImportRejection::RequiredInput {
            kind: interface.kind(),
            name: selected_name.clone(),
        });
    }
    first_required_static_dependency(declarations, selected_name, selected_namespace)
        .map(StaticImportRejection::UnresolvedDependency)
}

fn visit_references(
    references: impl IntoIterator<Item = StaticReference>,
    interfaces: &HashMap<StaticDeclarationKey, StaticRole>,
    declarations: &HashMap<StaticDeclarationKey, &Declaration>,
    visited: &mut HashSet<StaticDeclarationKey>,
) -> Option<RequiredStaticDependency> {
    for reference in references {
        for candidate in reference_candidates(&reference, interfaces) {
            if !visited.insert(candidate.clone()) {
                continue;
            }
            let role = interfaces[&candidate];
            if role == StaticRole::RequiredInput {
                return Some(RequiredStaticDependency {
                    kind: candidate.kind,
                    name: candidate.name,
                });
            }
            if let Some(declaration) = declarations.get(&candidate)
                && let Some(required) = visit_references(
                    declaration_static_references(&declaration.kind),
                    interfaces,
                    declarations,
                    visited,
                )
            {
                return Some(required);
            }
        }
    }
    None
}

/// Find the first transitive required Static input in one selected declaration.
///
/// The traversal is fail-closed for syntactically ambiguous bare names: every
/// locally matching Static namespace is visited. Qualified references are
/// external semantic identities and are outside this module-local closure.
#[must_use]
pub fn first_required_static_dependency(
    declarations: &[Declaration],
    selected_name: &NameAtom,
    selected_namespace: ImportItemNamespace,
) -> Option<RequiredStaticDependency> {
    let interfaces = declarations
        .iter()
        .filter_map(|declaration| {
            let interface = static_interface(&declaration.kind)?;
            let name = match &declaration.kind {
                DeclKind::BaseDimension(dimension) => dimension.name.value.atom(),
                DeclKind::Dimension(dimension) => dimension.name.value.atom(),
                DeclKind::Type(type_decl) => type_decl.name.value.atom(),
                DeclKind::Index(index) => index.name.value.atom(),
                _ => return None,
            };
            Some((
                StaticDeclarationKey::new(interface.kind(), name.clone()),
                interface.role(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let declaration_by_key = declarations
        .iter()
        .filter_map(|declaration| {
            let interface = static_interface(&declaration.kind)?;
            let name = match &declaration.kind {
                DeclKind::BaseDimension(dimension) => dimension.name.value.atom(),
                DeclKind::Dimension(dimension) => dimension.name.value.atom(),
                DeclKind::Type(type_decl) => type_decl.name.value.atom(),
                DeclKind::Index(index) => index.name.value.atom(),
                _ => return None,
            };
            Some((
                StaticDeclarationKey::new(interface.kind(), name.clone()),
                declaration,
            ))
        })
        .collect::<HashMap<_, _>>();
    let selected = declarations.iter().find(|declaration| {
        declaration_name_in_namespace(declaration, selected_name, selected_namespace)
    })?;

    visit_references(
        declaration_static_references(&selected.kind),
        &interfaces,
        &declaration_by_key,
        &mut HashSet::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> crate::desugar::desugared_ast::File {
        let file = Parser::new(source).parse_file().expect("source parses");
        crate::syntax::desugar::desugar_multi_decls_in_file(file)
    }

    #[test]
    fn finds_required_inputs_through_transitive_static_signatures() {
        let file = parse(
            "pub(bind) type Element;\n\
             pub type Box { Box(value: Element) }\n\
             pub type Wrapper { Wrapper(value: Box) }",
        );
        let required = first_required_static_dependency(
            &file.declarations,
            &NameAtom::parse("Wrapper").unwrap(),
            ImportItemNamespace::Type,
        )
        .expect("required dependency");
        assert_eq!(required.kind(), StaticInputKind::Type);
        assert_eq!(required.name().as_str(), "Element");
    }

    #[test]
    fn finds_required_dimensions_and_indexes_by_exact_category() {
        let dimensions = parse(
            "pub(bind) dim Basis;\n\
             pub dim Derived = Basis / Time;",
        );
        let required_dimension = first_required_static_dependency(
            &dimensions.declarations,
            &NameAtom::parse("Derived").unwrap(),
            ImportItemNamespace::Dimension,
        )
        .expect("required dimension dependency");
        assert_eq!(required_dimension.kind(), StaticInputKind::Dimension);
        assert_eq!(required_dimension.name().as_str(), "Basis");

        let indexes = parse(
            "pub(bind) index Axis;\n\
             pub type Samples { Samples(value: Dimensionless[Axis]) }",
        );
        let required_index = first_required_static_dependency(
            &indexes.declarations,
            &NameAtom::parse("Samples").unwrap(),
            ImportItemNamespace::Type,
        )
        .expect("required index dependency");
        assert_eq!(required_index.kind(), StaticInputKind::Index);
        assert_eq!(required_index.name().as_str(), "Axis");
    }

    #[test]
    fn optional_inputs_are_closed_through_their_defaults() {
        let file = parse(
            "pub(bind) type Element { Element }\n\
             pub type Box { Box(value: Element) }",
        );
        assert!(
            first_required_static_dependency(
                &file.declarations,
                &NameAtom::parse("Box").unwrap(),
                ImportItemNamespace::Type,
            )
            .is_none()
        );
    }
}
