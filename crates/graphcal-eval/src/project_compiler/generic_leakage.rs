//! Generic type-system leakage analysis at include boundaries.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "leakage checks consume project compiler model types"
)]
use super::*;
use graphcal_compiler::desugar::desugared_ast::DeclKind;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::static_dependencies::{
    StaticReference, StaticReferenceNamespaces, declaration_static_references,
};
use graphcal_compiler::ir::static_interface::{StaticInputKind, StaticRole, static_interface};
use graphcal_compiler::registry::types::IndexBindingTarget;
use graphcal_compiler::syntax::import_category::ImportItemNamespace;
use graphcal_compiler::syntax::names::NameAtom;

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
        .filter_map(|declaration| {
            let interface = static_interface(&declaration.kind)?;
            if interface.role() != StaticRole::RequiredInput {
                return None;
            }
            let name = match &declaration.kind {
                DeclKind::Dimension(dimension) => dimension.name.value.atom().clone(),
                DeclKind::Type(type_decl) => type_decl.name.value.atom().clone(),
                DeclKind::Index(index) => index.name.value.atom().clone(),
                _ => return None,
            };
            let namespace = match interface.kind() {
                StaticInputKind::Type => ImportItemNamespace::Type,
                StaticInputKind::Dimension => ImportItemNamespace::Dimension,
                StaticInputKind::Index => ImportItemNamespace::Index,
            };
            Some((name, namespace))
        })
        .collect()
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
    reference: &StaticReference,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) -> Result<ReferenceSubstitution, &'static str> {
    let Some(name) = reference.bare_name() else {
        return Ok(ReferenceSubstitution::Unbound);
    };
    match reference.namespaces() {
        StaticReferenceNamespaces::Exact(StaticInputKind::Index) => Ok(index_bindings
            .get(&IndexName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, index_substitution)),
        StaticReferenceNamespaces::Exact(StaticInputKind::Type) => Ok(type_bindings
            .get(&StructTypeName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::ImporterLocal(target.atom().clone())
            })),
        StaticReferenceNamespaces::Exact(StaticInputKind::Dimension) => Ok(dim_bindings
            .get(&DimName::from_atom(name.clone()))
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::ImporterLocal(target.atom().clone())
            })),
        namespaces @ (StaticReferenceNamespaces::TypeOrDimension
        | StaticReferenceNamespaces::IndexTypeOrDimension) => {
            let index = if namespaces == StaticReferenceNamespaces::IndexTypeOrDimension {
                index_bindings
                    .get(&IndexName::from_atom(name.clone()))
                    .map(index_substitution)
            } else {
                None
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

        let refs = declaration_static_references(&decl.kind);

        // Only a concrete importer-side substitution can leak an importer
        // declaration. Unsubstituted names remain dependency-local or builtin;
        // required ports, however, must have a substitution by this phase.
        for reference in refs {
            let Some(reference_name) = reference.bare_name() else {
                continue;
            };
            let substitution =
                reference_substitution(&reference, index_bindings, type_bindings, dim_bindings)
                    .map_err(|detail| {
                        CompileError::Eval(GraphcalError::internal_error(
                            format!(
                                "generic-leakage substitution for `{reference_name}` is inconsistent: {detail}"
                            ),
                            importer_src,
                            DiagnosticAnchor::Source(include_span),
                        ))
                    })?;
            let substituted = match substitution {
                ReferenceSubstitution::StructuralIndex => continue,
                ReferenceSubstitution::Unbound => {
                    if let Some(namespace) = required_bindings.get(reference_name) {
                        return Err(CompileError::Eval(GraphcalError::internal_error(
                            format!(
                                "required {} binding `{reference_name}` is absent during generic-leakage analysis",
                                namespace_diagnostic_name(*namespace),
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
                && !importer_external_surface.is_static_explicit_export(&substituted)
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
