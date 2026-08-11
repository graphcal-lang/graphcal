//! Generic type-system leakage analysis at include boundaries.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "leakage checks consume project compiler model types"
)]
use super::*;
use graphcal_compiler::desugar::desugared_ast::DeclKind;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::types::IndexBindingTarget;
use graphcal_compiler::syntax::import_category::ImportItemNamespace;
use graphcal_compiler::syntax::names::{NameAtom, NamePath};

/// Collect the set of type-system names declared locally in a file
/// (dims, units, indexes, types). Used to distinguish a private-local
/// symbol (V = private at the importer) from a builtin or cross-file
/// symbol for the V006 check.
pub(super) fn collect_local_type_names(
    file: &graphcal_compiler::desugar::desugared_ast::File,
) -> HashMap<NameAtom, ImportItemNamespace> {
    file.declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::BaseDimension(dimension) => Some((
                dimension.name.value.atom().clone(),
                ImportItemNamespace::Dimension,
            )),
            DeclKind::Dimension(dimension) => Some((
                dimension.name.value.atom().clone(),
                ImportItemNamespace::Dimension,
            )),
            DeclKind::Unit(unit) => {
                Some((unit.name.value.atom().clone(), ImportItemNamespace::Unit))
            }
            DeclKind::Index(index) => {
                Some((index.name.value.atom().clone(), ImportItemNamespace::Index))
            }
            DeclKind::Type(type_decl) => Some((
                type_decl.name.value.atom().clone(),
                ImportItemNamespace::Type,
            )),
            _ => None,
        })
        .collect()
}

fn collect_required_binding_names(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
) -> HashMap<NameAtom, ImportItemNamespace> {
    declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Dimension(dimension) if dimension.definition.is_none() => Some((
                dimension.name.value.atom().clone(),
                ImportItemNamespace::Dimension,
            )),
            DeclKind::Index(index) if index.kind.is_required() => {
                Some((index.name.value.atom().clone(), ImportItemNamespace::Index))
            }
            DeclKind::Type(type_decl)
                if matches!(
                    type_decl.body,
                    graphcal_compiler::desugar::desugared_ast::TypeDeclBody::Required
                ) =>
            {
                Some((
                    type_decl.name.value.atom().clone(),
                    ImportItemNamespace::Type,
                ))
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AmbiguousReferenceNamespace {
    TypeOrDimension,
    IndexTypeOrDimension,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeSystemReference {
    Known {
        name: NameAtom,
        namespace: ImportItemNamespace,
    },
    Ambiguous {
        name: NameAtom,
        namespaces: AmbiguousReferenceNamespace,
    },
}

impl TypeSystemReference {
    const fn known(name: NameAtom, namespace: ImportItemNamespace) -> Self {
        Self::Known { name, namespace }
    }

    const fn ambiguous(name: NameAtom, namespaces: AmbiguousReferenceNamespace) -> Self {
        Self::Ambiguous { name, namespaces }
    }

    const fn name(&self) -> &NameAtom {
        match self {
            Self::Known { name, .. } | Self::Ambiguous { name, .. } => name,
        }
    }
}

fn collect_bare_path(
    path: &NamePath,
    namespace: ImportItemNamespace,
    refs: &mut Vec<TypeSystemReference>,
) {
    if let Some(name) = path.as_bare() {
        refs.push(TypeSystemReference::known(name.clone(), namespace));
    }
}

fn collect_ambiguous_bare_path(
    path: &NamePath,
    namespaces: AmbiguousReferenceNamespace,
    refs: &mut Vec<TypeSystemReference>,
) {
    if let Some(name) = path.as_bare() {
        refs.push(TypeSystemReference::ambiguous(name.clone(), namespaces));
    }
}

/// Walk a `TypeExpr` collecting every bare type-system name reference.
/// Qualified references name dependency/external modules and cannot refer to
/// importer-local substitution ports.
fn collect_type_expr_names(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    refs: &mut Vec<TypeSystemReference>,
) {
    use graphcal_compiler::desugar::desugared_ast::{IndexExpr, TypeExprKind};
    match &type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => match dim_expr.terms.as_slice() {
            // The parser intentionally retains a bare nominal type as a
            // singleton dimension expression until semantic resolution.
            [item] if item.term.power.is_none() => collect_ambiguous_bare_path(
                &item.term.name.value,
                AmbiguousReferenceNamespace::TypeOrDimension,
                refs,
            ),
            terms => {
                for item in terms {
                    collect_bare_path(&item.term.name.value, ImportItemNamespace::Dimension, refs);
                }
            }
        },
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_names(base, refs);
            for index in indexes {
                if let IndexExpr::Name(path) = index {
                    collect_bare_path(&path.value, ImportItemNamespace::Index, refs);
                }
            }
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            collect_bare_path(&name.value, ImportItemNamespace::Type, refs);
            for arg in generic_args {
                collect_generic_arg_names(arg, refs);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            // `Complex` and `Key` are built in; collect only names in their
            // generic arguments.
            for arg in generic_args {
                collect_generic_arg_names(arg, refs);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in — no top-level name to push.
            for arg in type_args {
                collect_type_expr_names(arg, refs);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

fn collect_generic_arg_names(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    refs: &mut Vec<TypeSystemReference>,
) {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::syntax::ast::GenericArg;
    match arg {
        GenericArg::Type(type_expr) => collect_type_expr_names(type_expr, refs),
        GenericArg::Index(IndexExpr::Name(path)) => {
            collect_bare_path(&path.value, ImportItemNamespace::Index, refs);
        }
        GenericArg::Index(IndexExpr::Finite { .. } | IndexExpr::BareNat(_))
        | GenericArg::Nat(_) => {}
        GenericArg::Ambiguous(ambiguous) => collect_ambiguous_generic_names(ambiguous, refs),
    }
}

fn collect_ambiguous_generic_names(
    arg: &graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg,
    refs: &mut Vec<TypeSystemReference>,
) {
    match arg {
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            refs.push(TypeSystemReference::ambiguous(
                ident.name.clone(),
                AmbiguousReferenceNamespace::IndexTypeOrDimension,
            ));
        }
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_names(operand, refs);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReferenceSubstitution {
    Unbound,
    StructuralIndex,
    ImporterLocal(NameAtom),
}

fn index_substitution(target: &IndexBindingTarget) -> ReferenceSubstitution {
    match target {
        IndexBindingTarget::Declared(name) => {
            ReferenceSubstitution::ImporterLocal(name.atom().clone())
        }
        IndexBindingTarget::Finite(_) => ReferenceSubstitution::StructuralIndex,
    }
}

fn reference_substitution(
    reference: &TypeSystemReference,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) -> Result<ReferenceSubstitution, &'static str> {
    let name = reference.name();
    match reference {
        TypeSystemReference::Known {
            namespace: ImportItemNamespace::Index,
            ..
        } => Ok(index_bindings
            .get(&IndexName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, index_substitution)),
        TypeSystemReference::Known {
            namespace: ImportItemNamespace::Type,
            ..
        } => Ok(type_bindings
            .get(&StructTypeName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::ImporterLocal(target.atom().clone())
            })),
        TypeSystemReference::Known {
            namespace: ImportItemNamespace::Dimension,
            ..
        } => Ok(dim_bindings
            .get(&DimName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::ImporterLocal(target.atom().clone())
            })),
        TypeSystemReference::Known { .. } => Ok(ReferenceSubstitution::Unbound),
        TypeSystemReference::Ambiguous { namespaces, .. } => {
            let index = match namespaces {
                AmbiguousReferenceNamespace::IndexTypeOrDimension => index_bindings
                    .get(&IndexName::from_atom(name.clone()))
                    .map(index_substitution),
                AmbiguousReferenceNamespace::TypeOrDimension => None,
            };
            let type_name = type_bindings
                .get(&StructTypeName::from_atom(name.clone()))
                .map(|target| ReferenceSubstitution::ImporterLocal(target.atom().clone()));
            let dimension = dim_bindings
                .get(&DimName::from_atom(name.clone()))
                .map(|target| ReferenceSubstitution::ImporterLocal(target.atom().clone()));
            match (index, type_name, dimension) {
                (None, None, None) => Ok(ReferenceSubstitution::Unbound),
                (Some(substitution), None, None)
                | (None, Some(substitution), None)
                | (None, None, Some(substitution)) => Ok(substitution),
                _ => Err("name occurs in more than one include substitution namespace"),
            }
        }
    }
}

const fn namespace_diagnostic_name(namespace: ImportItemNamespace) -> &'static str {
    match namespace {
        ImportItemNamespace::Term => "term",
        ImportItemNamespace::Type => "type",
        ImportItemNamespace::Dimension => "dim",
        ImportItemNamespace::Unit => "unit",
        ImportItemNamespace::Index => "index",
    }
}

fn reexported_declaration_identity(kind: &DeclKind) -> Option<(DeclName, &'static str)> {
    match kind {
        DeclKind::Param(param) => Some((param.name.value.clone(), "param")),
        DeclKind::Node(node) => Some((node.name.value.clone(), "node")),
        DeclKind::ConstNode(constant) => Some((constant.name.value.clone(), "const node")),
        DeclKind::BaseDimension(dimension) => Some((
            DeclName::from_atom(dimension.name.value.atom().clone()),
            "dim",
        )),
        DeclKind::Dimension(dimension) => Some((
            DeclName::from_atom(dimension.name.value.atom().clone()),
            "dim",
        )),
        DeclKind::Unit(unit) => Some((DeclName::from_atom(unit.name.value.atom().clone()), "unit")),
        DeclKind::Index(index) => Some((
            DeclName::from_atom(index.name.value.atom().clone()),
            "index",
        )),
        DeclKind::Type(type_decl) => Some((
            DeclName::from_atom(type_decl.name.value.atom().clone()),
            "type",
        )),
        _ => None,
    }
}

fn declaration_signature_references(kind: &DeclKind) -> Vec<TypeSystemReference> {
    let mut refs = Vec::new();
    match kind {
        DeclKind::Param(param) => collect_type_expr_names(&param.type_ann, &mut refs),
        DeclKind::Node(node) => collect_type_expr_names(&node.type_ann, &mut refs),
        DeclKind::ConstNode(constant) => {
            collect_type_expr_names(&constant.type_ann, &mut refs);
        }
        DeclKind::Unit(unit) => {
            for item in &unit.dim_type.terms {
                collect_bare_path(
                    &item.term.name.value,
                    ImportItemNamespace::Dimension,
                    &mut refs,
                );
            }
        }
        DeclKind::Dimension(dimension) => {
            if let Some(definition) = &dimension.definition {
                for item in &definition.terms {
                    collect_bare_path(
                        &item.term.name.value,
                        ImportItemNamespace::Dimension,
                        &mut refs,
                    );
                }
            }
        }
        DeclKind::Type(type_decl) => {
            if let graphcal_compiler::desugar::desugared_ast::TypeDeclBody::Constructors(members) =
                &type_decl.body
            {
                for member in members {
                    if let Some(fields) = &member.payload {
                        for field in fields {
                            collect_type_expr_names(&field.type_ann, &mut refs);
                        }
                    }
                }
            }
            for default in type_decl
                .generic_params
                .iter()
                .filter_map(|param| param.default.as_ref())
            {
                collect_generic_arg_names(default, &mut refs);
            }
            // Bare references to lexical generic parameters are not module API
            // dependencies, even when a local declaration has the same leaf.
            let generic_params = type_decl
                .generic_params
                .iter()
                .map(|param| param.name.value.atom().clone())
                .collect::<HashSet<_>>();
            refs.retain(|reference| !generic_params.contains(reference.name()));
        }
        _ => {}
    }
    refs
}

/// A9 case 2 / V006 — re-exported decls must not name a private-at-importer
/// symbol in their effective signature.
///
/// For every dependency declaration that the importer explicitly re-exports
/// via `{ pub name }`, walk its signature, apply the include's substitution
/// map, and check each referenced type/dim/index name. If a name resolves to a
/// declaration that exists locally in the importer but is not explicitly
/// exported, the re-export leaks a private symbol.
#[expect(
    clippy::too_many_arguments,
    reason = "the check needs the dep AST, the three substitution maps, and the importer's visibility tables"
)]
pub(super) fn check_generics_leakage(
    dep_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    pub_reexport_items: &HashSet<DeclName>,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    importer_external_surface: &ExternalDeclSurface,
    importer_local_type_names: &HashMap<NameAtom, ImportItemNamespace>,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    if pub_reexport_items.is_empty() {
        return Ok(());
    }
    let required_bindings = collect_required_binding_names(dep_declarations);

    for decl in dep_declarations {
        // Is this decl part of the importer's re-exported surface?
        let Some((decl_name, decl_kind_str)) = reexported_declaration_identity(&decl.kind) else {
            continue;
        };
        if !pub_reexport_items.contains(&decl_name) {
            continue;
        }

        let refs = declaration_signature_references(&decl.kind);

        // Only a concrete importer-side substitution can leak an importer
        // declaration. Unsubstituted names remain dependency-local or builtin;
        // required ports, however, must have a substitution by this phase.
        for reference in refs {
            let substitution =
                reference_substitution(&reference, index_bindings, type_bindings, dim_bindings)
                    .map_err(|detail| {
                        CompileError::Eval(GraphcalError::internal_error(
                            format!(
                                "generic-leakage substitution for `{}` is inconsistent: {detail}",
                                reference.name()
                            ),
                            importer_src,
                            DiagnosticAnchor::Source(include_span),
                        ))
                    })?;
            let substituted = match substitution {
                ReferenceSubstitution::StructuralIndex => continue,
                ReferenceSubstitution::Unbound => {
                    if let Some(namespace) = required_bindings.get(reference.name()) {
                        return Err(CompileError::Eval(GraphcalError::internal_error(
                            format!(
                                "required {} binding `{}` is absent during generic-leakage analysis",
                                namespace_diagnostic_name(*namespace),
                                reference.name()
                            ),
                            importer_src,
                            DiagnosticAnchor::Source(include_span),
                        )));
                    }
                    continue;
                }
                ReferenceSubstitution::ImporterLocal(name) => name,
            };

            if let Some(namespace) = importer_local_type_names.get(&substituted)
                && !importer_external_surface
                    .is_explicit_export(&DeclName::from_atom(substituted.clone()))
            {
                return Err(CompileError::Eval(GraphcalError::GenericsLeakage {
                    reexport_kind: decl_kind_str.to_string(),
                    reexport_name: decl_name.to_string(),
                    leaked_kind: namespace_diagnostic_name(*namespace).to_string(),
                    leaked_name: substituted.to_string(),
                    src: importer_src.clone(),
                    span: include_span.into(),
                }));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_declarations(
        source: &str,
    ) -> Vec<graphcal_compiler::desugar::desugared_ast::Declaration> {
        let raw = graphcal_compiler::syntax::parser::Parser::new(source)
            .parse_file()
            .unwrap();
        graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw).declarations
    }

    fn source() -> NamedSource<Arc<String>> {
        NamedSource::new("main.gcl", Arc::new(String::new()))
    }

    #[test]
    fn unsubstituted_dependency_name_is_not_probed_in_the_importer() {
        let declarations = parse_declarations(
            "type Inner { Inner(value: Int), } node output: Inner = Inner(value: 1);",
        );
        let reexports = HashSet::from([DeclName::expect_valid("output")]);
        let importer_names =
            HashMap::from([(NameAtom::parse("Inner").unwrap(), ImportItemNamespace::Type)]);

        check_generics_leakage(
            &declarations,
            &reexports,
            &IndexBindings::new(),
            &HashMap::new(),
            &HashMap::new(),
            &ExternalDeclSurface::default(),
            &importer_names,
            &source(),
            Span::new(0, 0),
        )
        .unwrap();
    }

    #[test]
    fn missing_required_substitution_is_an_internal_error() {
        let declarations =
            parse_declarations("pub(bind) type Element; node output: Element = Missing;");
        let reexports = HashSet::from([DeclName::expect_valid("output")]);

        let error = check_generics_leakage(
            &declarations,
            &reexports,
            &IndexBindings::new(),
            &HashMap::new(),
            &HashMap::new(),
            &ExternalDeclSurface::default(),
            &HashMap::new(),
            &source(),
            Span::new(0, 0),
        )
        .unwrap_err();

        match error {
            CompileError::Eval(GraphcalError::InternalError { message, .. }) => assert!(
                message.contains(
                    "required type binding `Element` is absent during generic-leakage analysis"
                ),
                "{message}"
            ),
            other => panic!("expected internal error, got {other:?}"),
        }
    }
}
