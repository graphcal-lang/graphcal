//! Per-DAG HIR construction from a desugared syntax tree.
//!
//! `lower()` combines declaration collection (`resolve`), canonical
//! evaluation of the module's dimensions, units, and indexes, and nominal
//! type collection into one [`HirDag`]. Reference resolution happens at
//! [`UnfrozenIR::freeze`], which lowers every assembled declaration body to
//! HIR — a frozen DAG carries no syntax-AST expression.

use std::collections::HashMap;

use crate::desugar::desugared_ast::{DeclKind, File};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::module_interface::ModuleInterface;
use crate::ir::resolve::{CollectedFile, ImportedValueNames, resolve_with_imported_values};
use crate::outcome::Outcome;
use crate::resolved_name::ResolvedDeclName;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::module_name::ScopedName;

#[cfg(test)]
use super::extern_fns::resolve_extern_struct_return;
use super::model::{HirDag, IncludedPlotEntry, ParsedExpectedFailMetadata, UnfrozenIR};

use super::static_definitions::StaticDefinitionEvaluator;

/// Lower an AST into a [`HirDag`].
///
/// This combines:
/// 1. Name resolution (`resolve`) — checks duplicates, extracts deps
/// 2. Registry construction — registers dimensions, units, indexes, structs from declarations
/// 3. Function registration — registers user-defined functions into the registry
///
/// # Errors
///
/// Returns a [`SemanticError`] if declaration collection or registry construction fails
/// (e.g., unknown dimension in a type annotation, duplicate names, etc.).
/// `name` is the virtual relative path that names the module.
pub fn lower(ast: &File, name: &str, src: SourceId) -> Result<HirDag, SemanticError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(name))
        .map_err(|error| {
            SemanticError::internal_error(
                format!("invalid source name `{name}`: {error}"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    // Declaration collection reports duplicate names before the resolver's
    // own tables are built.
    let interface = ModuleInterface::new(&ast.declarations);
    resolve_with_imported_values(
        ast,
        interface.declared_surface(),
        src,
        &ImportedValueNames::default(),
    )?;
    let resolver = single_module_resolver(ast, &dag_id, src)?;
    let mut definitions = definition_evaluator(
        &resolver,
        [(
            dag_id.clone(),
            super::static_definitions::DefinitionSource {
                declarations: &ast.declarations,
                src,
            },
        )],
        src,
    )?;
    let unresolved = lower_module_with_imported_bindings(
        ast,
        src,
        &ImportedValueNames::default(),
        HashMap::new(),
        &dag_id,
        &mut definitions,
    )?;
    unresolved.freeze(&dag_id, &mut definitions, src)
}

/// Create a definition evaluator over `sources`, reporting a broken prelude
/// at `src`.
///
/// # Errors
///
/// Returns an internal error only if the built-in prelude is inconsistent.
pub fn definition_evaluator<'a>(
    resolver: &'a crate::resolve::ModuleResolver,
    sources: impl IntoIterator<
        Item = (
            crate::dag_id::DagId,
            super::static_definitions::DefinitionSource<'a>,
        ),
    >,
    src: SourceId,
) -> Result<StaticDefinitionEvaluator<'a>, SemanticError> {
    StaticDefinitionEvaluator::new(resolver, sources, src).map_err(|error| {
        SemanticError::internal_error(
            format!("prelude failed to load: {error}"),
            src,
            DiagnosticAnchor::Builtin,
        )
    })
}

/// A file root and its inline DAG bodies lowered with one resolver, for
/// compiler-side tests without the project loader.
#[cfg(test)]
pub(crate) struct LoweredTestFile {
    pub(crate) root: HirDag,
    pub(crate) inline_dags: Vec<HirDag>,
    pub(crate) resolver: crate::resolve::ModuleResolver,
}

/// Lower a file root and each inline DAG body (without self-import
/// preprocessing) against one project-wide resolver.
#[cfg(test)]
pub(crate) fn lower_file_with_inline_dags_for_test(
    ast: &File,
    name: &str,
    src: SourceId,
) -> Result<LoweredTestFile, SemanticError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(name))
        .unwrap_or_else(|error| panic!("test source name `{name}` is invalid: {error}"));
    let interface = ModuleInterface::new(&ast.declarations);
    resolve_with_imported_values(
        ast,
        interface.declared_surface(),
        src,
        &ImportedValueNames::default(),
    )?;
    let dag_bodies = ast
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Dag(dag) => Some((
                dag_id.inline_dag_child(dag.name.value.clone()),
                File {
                    declarations: dag.body.clone(),
                },
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(dag_id.clone(), &ast.declarations);
    for (owner, body) in &dag_bodies {
        modules.add(owner.clone(), &body.declarations);
        for declaration in &body.declarations {
            if let DeclKind::Import(import) = &declaration.kind {
                modules.import(owner, import, &dag_id);
            }
        }
    }
    let resolver = modules
        .build()
        .unwrap_or_else(|error| panic!("test module resolver failed: {error}"));
    let (root, inline_dags) = {
        let mut definitions = definition_evaluator(
            &resolver,
            std::iter::once((
                dag_id.clone(),
                super::static_definitions::DefinitionSource {
                    declarations: &ast.declarations,
                    src,
                },
            ))
            .chain(dag_bodies.iter().map(|(owner, body)| {
                (
                    owner.clone(),
                    super::static_definitions::DefinitionSource {
                        declarations: &body.declarations,
                        src,
                    },
                )
            })),
            src,
        )?;
        let unresolved = lower_module_with_imported_bindings(
            ast,
            src,
            &ImportedValueNames::default(),
            HashMap::new(),
            &dag_id,
            &mut definitions,
        )?;
        let root = unresolved.freeze(&dag_id, &mut definitions, src)?;
        let inline_dags = dag_bodies
            .iter()
            .map(|(owner, body)| {
                let unresolved = lower_dag_module_with_imported_bindings(
                    body,
                    &ImportedValueNames::default(),
                    HashMap::new(),
                    src,
                    owner,
                    &mut definitions,
                )?;
                unresolved.freeze(owner, &mut definitions, src)
            })
            .collect::<Result<Vec<_>, SemanticError>>()?;
        (root, inline_dags)
    };
    Ok(LoweredTestFile {
        root,
        inline_dags,
        resolver,
    })
}

/// Build a resolver covering only this file's own module.
///
/// Single-file lowering has no project loader, so imported modules are not
/// resolvable; bodies that reference them fail at the freeze boundary just
/// as they previously failed during type resolution.
fn single_module_resolver(
    ast: &File,
    dag_id: &crate::dag_id::DagId,
    src: SourceId,
) -> Result<crate::resolve::ModuleResolver, SemanticError> {
    let mut tables = crate::resolve::builder::SymbolTables::default();
    tables
        .add_file(dag_id.clone(), &ast.declarations)
        .and_then(|()| {
            tables
                .scopes(&crate::resolve::builder::NoModuleTargets)?
                .freeze()
        })
        .map_err(|error| {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })
}

/// The Static ports `owner` declares, with the identities the resolver
/// declared for them.
fn collect_static_ports(
    ast: &File,
    owner: &crate::dag_id::DagId,
    resolver: &crate::resolve::ModuleResolver,
) -> Result<Vec<crate::hir::source_interface::StaticPort>, crate::resolve::error::ModuleResolveError>
{
    use crate::hir::source_interface::StaticPortIdentity;
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            let interface = crate::static_interface::static_interface(&declaration.kind)?;
            let identity = match &declaration.kind {
                DeclKind::Type(type_decl) => resolver
                    .declaration(owner, &type_decl.name.value)
                    .map(|symbol| StaticPortIdentity::Type(symbol.into_resolved())),
                DeclKind::BaseDimension(dimension) => resolver
                    .declaration(owner, &dimension.name.value)
                    .map(|symbol| StaticPortIdentity::Dimension(symbol.into_resolved())),
                DeclKind::Dimension(dimension) => resolver
                    .declaration(owner, &dimension.name.value)
                    .map(|symbol| StaticPortIdentity::Dimension(symbol.into_resolved())),
                DeclKind::Index(index) => resolver
                    .declaration(owner, &index.name.value)
                    .map(|symbol| StaticPortIdentity::Index(symbol.into_resolved())),
                _ => return None,
            };
            Some(
                identity.map(|identity| crate::hir::source_interface::StaticPort {
                    identity,
                    role: interface.role(),
                    span: declaration.span,
                }),
            )
        })
        .collect()
}

/// The runtime-interface declarations `owner` authors directly, in source
/// order, with the identities the resolver declared for them.
fn collect_source_declarations(
    ast: &File,
    owner: &crate::dag_id::DagId,
    resolver: &crate::resolve::ModuleResolver,
) -> Result<
    Vec<crate::hir::source_interface::SourceDeclaration>,
    crate::resolve::error::ModuleResolveError,
> {
    use crate::hir::source_interface::SourceDeclaration;
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            let span = declaration.span;
            Some(match &declaration.kind {
                DeclKind::Param(param) => {
                    resolver
                        .declaration(owner, &param.name.value)
                        .map(|symbol| SourceDeclaration::Parameter {
                            identity: symbol.into_resolved(),
                            span,
                        })
                }
                DeclKind::Node(node) => {
                    resolver.declaration(owner, &node.name.value).map(|symbol| {
                        SourceDeclaration::Node {
                            identity: symbol.into_resolved(),
                            span,
                        }
                    })
                }
                DeclKind::Index(index) => {
                    resolver
                        .declaration(owner, &index.name.value)
                        .map(|symbol| SourceDeclaration::Index {
                            identity: symbol.into_resolved(),
                            span,
                        })
                }
                _ => return None,
            })
        })
        .collect()
}

/// Lower an AST with imported value bindings into an [`UnfrozenIR`], which
/// include elaboration may still extend before freezing.
///
/// Imported lexical names are added to the resolution scope without injecting
/// parallel AST expressions. Canonical targets remain attached to those names
/// in `imported_bindings`.
///
/// # Errors
///
/// Returns a [`SemanticError`] if declaration collection or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_module_with_imported_bindings(
    ast: &File,
    src: SourceId,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
) -> Result<UnfrozenIR, SemanticError> {
    crate::outcome::without_cancellation(|cancellation| {
        lower_module_with_imported_bindings_and_cancellation(
            ModuleBody {
                ast,
                interface: &ModuleInterface::new(&ast.declarations),
            },
            src,
            imported_names,
            imported_bindings,
            dag_id,
            definitions,
            cancellation,
        )
    })
}

/// Lower an AST with imported bindings and cooperative cancellation.
///
/// # Errors
///
/// Returns a [`SemanticError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_module_with_imported_bindings_and_cancellation(
    module: ModuleBody<'_>,
    src: SourceId,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let ModuleBody { ast, interface } = module;
    let resolved =
        resolve_with_imported_values(ast, interface.declared_surface(), src, imported_names)?;
    let mut unfrozen = build_ir_from_resolved(
        ast,
        src,
        resolved,
        imported_bindings,
        dag_id,
        definitions,
        cancellation,
    )?;

    // Plot aliases from include brace lists become known to this DAG so
    // figures/layers can reference them (#847).
    unfrozen.included_plots = imported_names
        .plot_names
        .iter()
        .map(|(name, _span)| IncludedPlotEntry { name: name.clone() })
        .collect();

    Ok(unfrozen)
}

/// Lower a `dag { ... }` body as if it were a standalone file.
///
/// The dag body is a virtual [`File`] with its own lexical scope. Cross-scope
/// values must be passed through params or explicit imports; every imported
/// lexical name maps to its canonical [`ResolvedDeclName`] target.
///
/// # Errors
///
/// Returns a [`SemanticError`] if declaration collection or type-system construction
/// fails for the dag body.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_dag_module_with_imported_bindings(
    dag_body: &File,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: SourceId,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
) -> Result<UnfrozenIR, SemanticError> {
    crate::outcome::without_cancellation(|cancellation| {
        lower_dag_module_with_imported_bindings_and_cancellation(
            ModuleBody {
                ast: dag_body,
                interface: &ModuleInterface::new(&dag_body.declarations),
            },
            imported_names,
            imported_bindings,
            src,
            dag_id,
            definitions,
            cancellation,
        )
    })
}

/// Lower an inline DAG module with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`SemanticError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_dag_module_with_imported_bindings_and_cancellation(
    module: ModuleBody<'_>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: SourceId,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let ModuleBody {
        ast: dag_body,
        interface,
    } = module;
    let resolved =
        resolve_with_imported_values(dag_body, interface.declared_surface(), src, imported_names)?;

    build_ir_from_resolved(
        dag_body,
        src,
        resolved,
        imported_bindings,
        dag_id,
        definitions,
        cancellation,
    )
}

/// One module body to lower together with the declared interface of the
/// module it belongs to.
///
/// The interface's declared surface (explicit exports and `param` input ports
/// of the module's own declarations) is the single classification of the
/// module's external boundary; lowering does not re-derive it. An inline DAG's
/// `ast` may have its self-imports stripped, which never changes the declared
/// surface.
#[derive(Debug, Clone, Copy)]
pub struct ModuleBody<'a> {
    pub ast: &'a File,
    pub interface: &'a ModuleInterface,
}

/// Result of `preprocess_dag_body_self_imports`: imported names, canonical
/// bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub bindings: HashMap<ScopedName, ResolvedDeclName>,
    pub stripped_body: Vec<crate::desugar::desugared_ast::Declaration>,
}

/// Shared implementation for local and imported-binding lowering.
///
/// Evaluates the module's canonical Static definitions and constructs the
/// `UnfrozenIR` from the collected declaration entries.
fn build_ir_from_resolved(
    ast: &File,
    src: SourceId,
    resolved: CollectedFile,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    // Dimensions, units, and indexes are evaluated canonically through the
    // module resolver; nothing is registered under a source spelling.
    let module_statics = definitions.module_definitions(dag_id)?;
    cancellation.checkpoint()?;
    // The resolver was built from these same declarations, so it declared
    // every identity the entries and Static ports need.
    let module_resolver = definitions.resolver();
    let (decls, static_ports, source_declarations) = super::resolve::declaration_entries(
        ast,
        &resolved.plot_visibilities,
        module_resolver,
        dag_id,
    )
    .and_then(|decls| {
        Ok((
            decls,
            collect_static_ports(ast, dag_id, module_resolver)?,
            collect_source_declarations(ast, dag_id, module_resolver)?,
        ))
    })
    .map_err(|error| {
        SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;

    let unfrozen = UnfrozenIR {
        decls,
        included_plots: Vec::new(),
        source_declarations,
        static_ports,
        assumes_map: resolved
            .assumes_map
            .into_iter()
            .map(|(k, v)| {
                (
                    ScopedName::local(k),
                    v.into_iter().map(ScopedName::local).collect(),
                )
            })
            .collect(),
        expected_fail: resolved
            .expected_fail
            .into_iter()
            .map(|(name, collected)| {
                (
                    ScopedName::local(name),
                    ParsedExpectedFailMetadata {
                        expected: collected.expected,
                        resolution_owner: dag_id.clone(),
                        attribute_span: collected.attribute_span,
                    },
                )
            })
            .collect(),
        dynamic_unit_scales: module_statics.dynamic_unit_scales,
        static_definitions: module_statics.definitions,
        unit_bindings: HashMap::new(),
        imported_bindings,
        external_surface: resolved.external_surface,
        plugin_imports: ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                DeclKind::PluginImport(plugin) => Some(plugin.clone()),
                _ => None,
            })
            .collect(),
        semantic_instances: Vec::new(),
    };

    Ok(unfrozen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_error::SemanticErrorKind;
    use crate::semantic_error::dimension::DimensionError;
    use crate::semantic_error::name::NameError;
    use crate::semantic_error::plugin::PluginError;
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::parser::Parser;

    fn make_src(source: &str) -> SourceId {
        crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<HirDag, SemanticError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::desugar::desugared_ast::File::from(raw_file);
        let file = desugared;
        lower(&file, "test.gcl", make_src(source))
    }

    #[test]
    fn lower_rocket() {
        let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.decls().consts().count(), 1); // G0
        assert_eq!(ir.decls().params().count(), 3); // dry_mass, fuel_mass, isp
        assert_eq!(ir.decls().nodes().count(), 3); // v_exhaust, mass_ratio, delta_v
        // Prelude names are resolved canonically; the module owns no copy.
        assert_eq!(ir.definitions().statics().dimensions().count(), 0);
        assert!(ir.display_dimensions.iter().any(|(name, _)| name
            == &crate::syntax::dimension::DimRef::local(
                crate::syntax::dimension::DimName::expect_valid("Length")
            )));
    }

    #[test]
    fn lower_constants() {
        let source = include_str!("../../../../tests/fixtures/valid/constants.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.decls().consts().count(), 4);
        assert_eq!(ir.decls().params().count(), 1);
        assert_eq!(ir.decls().nodes().count(), 2);
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../../tests/fixtures/valid/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(
            ir.definitions()
                .statics()
                .indexes()
                .any(|(identity, _)| identity.as_str() == "Maneuver")
        );
    }

    #[test]
    fn hir_retains_the_direct_runtime_interface_in_source_order() {
        let hir = parse_and_lower(
            "const node ignored: Dimensionless = 1.0;\n\
             pub(bind) index Phase;\n\
             param input: Dimensionless = 1.0;\n\
             node private: Dimensionless = @input;\n\
             pub node output: Dimensionless = @private;\n",
        )
        .unwrap();

        assert!(matches!(
            hir.source_declarations(),
            [
                crate::hir::source_interface::SourceDeclaration::Index { identity: index, .. },
                crate::hir::source_interface::SourceDeclaration::Parameter { identity: input, .. },
                crate::hir::source_interface::SourceDeclaration::Node { identity: private, .. },
                crate::hir::source_interface::SourceDeclaration::Node { identity: output, .. },
            ] if index.as_str() == "Phase"
                && input.as_str() == "input"
                && private.as_str() == "private"
                && output.as_str() == "output"
                && [input, private, output].iter().all(|identity| identity.owner() == hir.dag_id())
        ));
    }

    #[test]
    fn nominal_signatures_are_canonical_hir() {
        let source = "type Marker { Marker }\n\
             type Box<T: Type = Marker> { Box(value: T) }\n";
        let hir_source = make_src(source);
        let file =
            crate::desugar::desugared_ast::File::from(Parser::new(source).parse_file().unwrap());
        let hir = lower(&file, "test.gcl", hir_source).unwrap();
        let identity = crate::resolved_name::ResolvedStructTypeName::for_test(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Box"),
        );
        let marker = crate::resolved_name::ResolvedStructTypeName::for_test(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Marker"),
        );
        assert!(
            hir.nominal_types().get(&identity).is_some(),
            "canonical nominal definitions must cross into HIR"
        );
        let definition = hir.nominal_types().get(&identity).unwrap();
        let [parameter] = definition.generic_params() else {
            panic!("Box should retain exactly one generic parameter");
        };
        let Some(crate::hir::types::GenericArg::Type(default)) = parameter.default() else {
            panic!("Box#T should have a HIR type default");
        };
        assert!(matches!(
            &default.kind,
            crate::hir::types::ValueTypeKind::Struct(name) if name.value == marker
        ));
        let [constructor] = definition.union_members().unwrap() else {
            panic!("Box should retain exactly one constructor");
        };
        let [field] = constructor.fields() else {
            panic!("Box should retain exactly one field");
        };
        assert!(matches!(
            &field.type_annotation().decl_type,
            crate::hir::types::DeclType::Value(crate::hir::types::ValueType {
                kind: crate::hir::types::ValueTypeKind::GenericTypeParam(field_param),
                ..
            }) if &field_param.value == parameter.id()
        ));
        assert_eq!(definition.source(), hir_source);
    }

    #[test]
    fn lower_hohmann() {
        // hohmann.gcl uses DAG+include. The full project pipeline accepts
        // it (see the CLI tests), but single-file IR lowering rejects it at
        // the freeze boundary: include expansion is a higher-phase concern,
        // so `@transfer` (the include's projected node) cannot resolve.
        let source = include_str!("../../../../tests/fixtures/valid/hohmann.gcl");
        let err = parse_and_lower(source).unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::UnknownGraphRef { .. }),
                ..
            })
        ));
    }

    #[test]
    fn lower_duplicate_name_error() {
        let err = parse_and_lower("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::DuplicateName { .. }),
                ..
            })
        ));
    }

    #[test]
    fn duplicate_constructor_field_declarations_are_rejected() {
        let err =
            parse_and_lower("pub type Pair { Pair(value: Length, other: Bool, value: Time) }")
                .unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::DuplicateConstructorField { type_name, constructor, field, .. }), .. }) if type_name.as_str() == "Pair"
                && constructor.as_str() == "Pair"
                && field.as_str() == "value"
        ));
    }

    #[test]
    fn same_field_name_in_distinct_constructors_is_valid() {
        parse_and_lower("pub type Choice { Left(value: Length), Right(value: Time) }").unwrap();
    }

    #[test]
    fn unknown_unit_dimension_reports_referenced_dimension() {
        let err = parse_and_lower("unit foo: Blah = 1.0 m;").unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name.to_string() == "Blah"
        ));
    }

    #[test]
    fn unknown_derived_dimension_term_reports_referenced_dimension() {
        let err = parse_and_lower("dim Foo = Bar * Baz;").unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name.to_string() == "Bar"
        ));
    }

    #[test]
    fn extern_struct_results_merge_only_for_the_same_record_type() {
        let declarations = |second_result: &str| {
            format!(
                "type Bounds {{ Bounds(lo: Length, hi: Length), }}\n\
                 type Range {{ Range(lo: Length, hi: Length), }}\n\
                 import plugin \"graphcal:demo\" as a {{ fn bounds(x: Length) -> Bounds; }}\n\
                 import plugin \"graphcal:demo\" as b {{ fn bounds(y: Length) -> {second_result}; }}\n"
            )
        };

        let merged = parse_and_lower(&declarations("Bounds")).unwrap();
        assert_eq!(merged.extern_functions().len(), 1);

        let err = parse_and_lower(&declarations("Range")).unwrap_err();
        assert!(
            matches!(
                &err,
                SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Plugin(PluginError::InvalidExternSignature { message, .. }), .. })
                    if message.contains("different result type")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn unknown_qualified_extern_dimension_preserves_its_path() {
        let path = NamePath::qualified(
            crate::syntax::non_empty::NonEmpty::singleton(NameAtom::parse("missing").unwrap()),
            NameAtom::parse("Dimension").unwrap(),
        );
        let owner =
            crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let resolver = crate::resolve::ModuleResolver::default();
        let source = make_src("missing.Dimension");
        let mut definitions = definition_evaluator(&resolver, [], source).unwrap();
        let nominal_types = crate::hir::nominal::NominalTypeRegistry::default();

        let error = resolve_extern_struct_return(
            &path,
            source.whole_span(),
            &mut super::super::extern_fns::ExternSignatureScope {
                owner: &owner,
                nominal_types: &nominal_types,
                definitions: &mut definitions,
            },
            source,
        )
        .unwrap_err();

        assert!(
            matches!(
                &error,
                SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. })
                    if name.qualifier().iter().map(NameAtom::as_str).eq(["missing"])
                        && name.leaf().as_str() == "Dimension"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn static_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower("const unit wrong: Length = 1.0 h;").unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnitDefinitionDimensionMismatch { name, declared, definition, .. }), .. }) if name.as_str() == "wrong"
                && declared == "Length"
                && definition == "Time"
        ));
    }

    #[test]
    fn dynamic_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower(
            "param factor: Dimensionless = 2.0;\nunit wrong: Length = (@factor) h;",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnitDefinitionDimensionMismatch { name, .. }), .. })
                if name.as_str() == "wrong"
        ));
    }

    #[test]
    fn compound_unit_definition_with_matching_dimension_is_accepted() {
        parse_and_lower("const unit cruise: Velocity = 0.5 km/h;").unwrap();
    }

    #[test]
    fn failed_unit_definition_is_not_memoized() {
        let source = "const unit wrong: Length = 1.0 h;";
        let src = make_src(source);
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = crate::desugar::desugared_ast::File::from(raw_file);
        let owner = crate::dag_id::DagId::root_in_package("test", "main");
        let resolver = single_module_resolver(&file, &owner, src).unwrap();
        let mut definitions = definition_evaluator(
            &resolver,
            [(
                owner.clone(),
                super::super::static_definitions::DefinitionSource {
                    declarations: &file.declarations,
                    src,
                },
            )],
            src,
        )
        .unwrap();
        let wrong = resolver
            .resolve_unit_path(&owner, &NamePath::expect_local("wrong"))
            .unwrap()
            .into_resolved();

        for _ in 0..2 {
            assert!(matches!(
                definitions.unit(&wrong),
                Err(SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Dimension(
                        DimensionError::UnitDefinitionDimensionMismatch { .. }
                    ),
                    ..
                }))
            ));
        }
        assert!(matches!(
            definitions.module_definitions(&owner),
            Err(SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(
                    DimensionError::UnitDefinitionDimensionMismatch { .. }
                ),
                ..
            }))
        ));
    }

    #[test]
    fn freeze_resolves_attribute_targets_to_identities() {
        let hir = parse_and_lower(
            "#[expected_fail]\nassert ok = true;\n\
             #[assumes(ok)]\nparam p: Dimensionless = 1.0;\n\
             #[assumes(ok)]\nnode n: Dimensionless = @p;",
        )
        .unwrap();
        let identity = |spelling: &str| {
            hir.decls()
                .lookup(&DeclName::expect_valid(spelling))
                .unwrap()
                .clone()
        };
        let ok = identity("ok");
        assert_eq!(ok.owner(), hir.dag_id());
        let mut assumers = hir.assumes_map.get(&ok).unwrap().clone();
        assumers.sort_by_key(ToString::to_string);
        assert_eq!(assumers, [identity("n"), identity("p")]);
        assert!(matches!(
            hir.expected_fail
                .get(&ok)
                .map(|metadata| &metadata.expected),
            Some(crate::assertion_expectation::ExpectedFail::All)
        ));
        assert_eq!(hir.expected_fail.len(), 1);
    }

    #[test]
    fn lower_source_order_preserved() {
        let ir = parse_and_lower(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;",
        )
        .unwrap();
        let names: Vec<String> = ir.decls().iter().map(|d| d.name().to_string()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }
}
