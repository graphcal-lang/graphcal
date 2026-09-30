//! Generic type-system leakage analysis at include boundaries.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "leakage checks consume project compiler model types"
)]
use super::*;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::ir::static_dependencies::{
    StaticReference, StaticScope, declaration_static_references,
};
use graphcal_compiler::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use graphcal_compiler::resolved_name::ResolvedStaticName;
use graphcal_compiler::static_interface::{StaticRole, static_interface};
use graphcal_compiler::syntax::ast::IntroducedKind;
use graphcal_compiler::syntax::import_category::ImportItemNamespace;
use graphcal_compiler::syntax::names::NameAtom;

fn collect_required_binding_names(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
) -> HashMap<NameAtom, ImportItemNamespace> {
    declarations
        .iter()
        .filter(|declaration| {
            static_interface(&declaration.kind)
                .is_some_and(|interface| interface.role() == StaticRole::RequiredInput)
        })
        .filter_map(|declaration| declaration.kind.declared_name())
        .map(|introduced| (introduced.atom().clone(), introduced.namespace()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReferenceSubstitution {
    /// The reference names a symbol of another module; includes never
    /// substitute it.
    External,
    Unbound,
    StructuralIndex,
    /// The canonical importer-side definition the include binds.
    Importer(ResolvedStaticName),
}

/// The importer-side substitution for one resolved dependency reference.
///
/// Only the dependency's own Static ports are substituted by an include;
/// references into other modules are external. Prelude names do not resolve
/// to a module symbol and never reach this function.
fn reference_substitution(
    reference: &StaticReference,
    dependency: &DagId,
    substitution: &StaticSubstitution,
) -> ReferenceSubstitution {
    if reference.target().owner() != dependency {
        return ReferenceSubstitution::External;
    }
    match reference.target() {
        ResolvedStaticName::Index(name) => {
            substitution
                .indexes
                .get(name)
                .map_or(ReferenceSubstitution::Unbound, |target| match target {
                    InstanceIndexBindingTarget::Declared(target) => {
                        ReferenceSubstitution::Importer(ResolvedStaticName::Index(target.clone()))
                    }
                    InstanceIndexBindingTarget::Finite(_) => ReferenceSubstitution::StructuralIndex,
                })
        }
        ResolvedStaticName::Type(name) => substitution
            .types
            .get(name)
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::Importer(ResolvedStaticName::Type(target.clone()))
            }),
        ResolvedStaticName::Dimension(name) => substitution
            .dimensions
            .get(name)
            .map_or(ReferenceSubstitution::Unbound, |target| {
                ReferenceSubstitution::Importer(ResolvedStaticName::Dimension(target.clone()))
            }),
    }
}

const fn static_namespace(name: &ResolvedStaticName) -> ImportItemNamespace {
    match name {
        ResolvedStaticName::Dimension(_) => ImportItemNamespace::Dimension,
        ResolvedStaticName::Type(_) => ImportItemNamespace::Type,
        ResolvedStaticName::Index(_) => ImportItemNamespace::Index,
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

/// Diagnostic noun for a declaration an include brace item (`{ pub name }`)
/// can re-export with a checked signature, or `None` for declarations whose
/// signature is outside the V006 check.
const fn reexported_declaration_kind(kind: IntroducedKind) -> Option<&'static str> {
    match kind {
        IntroducedKind::Param => Some("param"),
        IntroducedKind::Node => Some("node"),
        IntroducedKind::ConstNode => Some("const node"),
        IntroducedKind::BaseDimension | IntroducedKind::Dimension => Some("dim"),
        IntroducedKind::Unit => Some("unit"),
        IntroducedKind::Index => Some("index"),
        IntroducedKind::Type => Some("type"),
        IntroducedKind::Assert
        | IntroducedKind::Plot
        | IntroducedKind::Figure
        | IntroducedKind::Layer
        | IntroducedKind::Dag
        | IntroducedKind::Constructor => None,
    }
}

/// A9 case 2 / V006 — re-exported decls must not name a private-at-importer
/// symbol in their effective signature.
///
/// For every dependency declaration that the importer explicitly re-exports
/// via `{ pub name }`, walk its signature, apply the include's canonical
/// substitution, and check each referenced type/dim/index. If it is bound to
/// a declaration of the importer itself that the importer does not explicitly
/// export, the re-export leaks a private symbol.
#[expect(
    clippy::too_many_arguments,
    reason = "the check needs the dep AST, the canonical substitution, and the importer's identity and interface"
)]
pub(super) fn check_generics_leakage(
    dep_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dep_scope: StaticScope<'_>,
    pub_reexport_items: &HashSet<NameAtom>,
    substitution: &StaticSubstitution,
    importer: &DagId,
    importer_interface: &ModuleInterface,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    if pub_reexport_items.is_empty() {
        return Ok(());
    }
    let required_bindings = collect_required_binding_names(dep_declarations);

    for decl in dep_declarations {
        // Is this decl part of the importer's re-exported surface?
        let Some(introduced) = decl.kind.declared_name() else {
            continue;
        };
        let Some(decl_kind_str) = reexported_declaration_kind(introduced.kind()) else {
            continue;
        };
        let decl_name = introduced.atom();
        if !pub_reexport_items.contains(decl_name) {
            continue;
        }

        let refs = declaration_static_references(&decl.kind, dep_scope);

        // Only a concrete importer-side substitution can leak an importer
        // declaration. Unsubstituted names remain dependency-local or builtin;
        // required ports, however, must have a substitution by this phase.
        for reference in refs {
            let reference_name = reference.target().atom();
            let substituted = match reference_substitution(
                &reference,
                dep_scope.owner(),
                substitution,
            ) {
                ReferenceSubstitution::External | ReferenceSubstitution::StructuralIndex => {
                    continue;
                }
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
                ReferenceSubstitution::Importer(name) => name,
            };

            let namespace = static_namespace(&substituted);
            if substituted.owner() == importer
                && importer_interface
                    .declared_kinds(substituted.atom(), namespace)
                    .next()
                    .is_some()
                && !importer_interface
                    .external_surface()
                    .is_static_explicit_export(substituted.atom())
            {
                return Err(CompileError::Eval(GraphcalError::GenericsLeakage {
                    reexport_kind: decl_kind_str.to_string(),
                    reexport_name: decl_name.to_string(),
                    leaked_kind: namespace_diagnostic_name(namespace).to_string(),
                    leaked_name: substituted.atom().to_string(),
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
        graphcal_compiler::desugar::desugared_ast::File::from(raw).declarations
    }

    fn source() -> NamedSource<Arc<String>> {
        NamedSource::new("main.gcl", Arc::new(String::new()))
    }

    fn dependency() -> DagId {
        DagId::from_virtual_relative_path(std::path::Path::new("dep.gcl")).unwrap()
    }

    fn importer() -> DagId {
        DagId::from_virtual_relative_path(std::path::Path::new("main.gcl")).unwrap()
    }

    fn resolver(
        declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    ) -> graphcal_compiler::resolve::ModuleResolver {
        let mut tables = graphcal_compiler::resolve::builder::SymbolTables::default();
        tables.add_file(dependency(), declarations).unwrap();
        tables
            .scopes(&graphcal_compiler::resolve::builder::NoModuleTargets)
            .unwrap()
            .freeze()
            .unwrap()
    }

    #[test]
    fn unsubstituted_dependency_name_is_not_probed_in_the_importer() {
        let declarations = parse_declarations(
            "type Inner { Inner(value: Int), } node output: Inner = Inner(value: 1);",
        );
        let reexports = HashSet::from([NameAtom::parse("output").unwrap()]);
        let importer_interface =
            ModuleInterface::new(&parse_declarations("type Inner { Inner(value: Int), }"));

        let resolver = resolver(&declarations);
        let owner = dependency();
        check_generics_leakage(
            &declarations,
            StaticScope::new(&owner, &resolver),
            &reexports,
            &StaticSubstitution::default(),
            &importer(),
            &importer_interface,
            &source(),
            Span::new(0, 0),
        )
        .unwrap();
    }

    #[test]
    fn missing_required_substitution_is_an_internal_error() {
        let declarations =
            parse_declarations("pub(bind) type Element; node output: Element = Missing;");
        let reexports = HashSet::from([NameAtom::parse("output").unwrap()]);

        let resolver = resolver(&declarations);
        let owner = dependency();
        let error = check_generics_leakage(
            &declarations,
            StaticScope::new(&owner, &resolver),
            &reexports,
            &StaticSubstitution::default(),
            &importer(),
            &ModuleInterface::default(),
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
