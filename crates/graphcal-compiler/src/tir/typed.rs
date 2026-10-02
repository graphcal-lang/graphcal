//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! For value declarations, the TIR layer consumes canonical HIR type references
//! and resolves them into concrete dimensions, struct types, generic dimension
//! parameters, or generic index parameters. It does not reinterpret source
//! paths from declaration signatures.

use crate::outcome::Outcome;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use crate::semantic_error::attribute::AttributeError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::graph::GraphError;
use crate::semantic_error::name::NameError;
use crate::semantic_error::plugin::ExternCallContext;
use crate::semantic_error::plugin::PluginError;
use crate::semantic_error::visibility::VisibilityError;
use crate::source_id::SourceId;
use crate::syntax::non_empty::NonEmpty;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::generic_param::GenericParamId;
pub use crate::ir::model::{LoweredPlotBody, LoweredPlotField};
pub use crate::nat::NatPolyForm;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::{Span, Spanned};

use crate::ir::imported_binding::{ImportedBinding, ImportedConstantTypes};
use crate::ir::model::HirDag;
use crate::ir::resolve::collected::ExternalDeclSurface;
use crate::resolve::ModuleResolver;
use crate::resolve::symbols::SymbolRef;
use crate::semantic_error::SemanticError;
use crate::syntax::module_name::ScopedName;

pub mod body_scope;
pub use body_scope::Scoped;
pub mod checked_instance;
pub use checked_instance::*;
pub mod checked;
pub mod checked_dag;
pub use checked_dag::*;
pub mod dag_position;
pub mod dag_store;
pub mod declaration_view;
pub mod declared_type_spelling;
pub use dag_store::*;
pub mod freeze;
pub use freeze::*;
pub(crate) mod checking_tir;
pub mod dag_slots;
pub mod program;
pub use program::*;
pub(crate) mod frame_mint;
pub use checked::*;
pub mod evaluation_unit;
pub use evaluation_unit::*;
pub mod model;
pub use model::*;
pub mod module_type_context;
pub use module_type_context::ModuleTypeContext;
pub mod override_dependencies;
pub use override_dependencies::CheckedOverrideDependencies;
pub mod resolved_nominal;
pub use resolved_nominal::*;
pub mod resolved_type;
pub use resolved_type::*;
pub mod scoped_node;

impl DagTIR {
    /// Populate the values that callers may project from this DAG.
    ///
    /// Explicitly exported nodes and annotation-free param input ports are both
    /// readable outputs. Projecting a param returns its effective value after
    /// applying the call binding or default.
    fn populate_projectable_outputs(&mut self, surface: &ExternalDeclSurface) {
        self.projectable_outputs.extend(
            self.decls
                .params()
                .map(crate::ir::entry::ParamEntry::name)
                .chain(self.decls.nodes().map(crate::ir::entry::NodeEntry::name))
                .filter(|name| surface.can_select_output(name))
                .cloned(),
        );
    }

    /// Look up the canonical identity recorded for a source-facing name, for
    /// tests that name a declaration by its spelling.
    ///
    /// Unknown names remain [`DiagnosticDeclProbe`] values rather than being
    /// assigned an authoritative-looking identity from this DAG's owner.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn lookup_decl_identity(&self, name: &ScopedName) -> DeclarationIdentityLookup {
        self.semantic.decl_bindings.get(name).map_or_else(
            || {
                DeclarationIdentityLookup::DiagnosticProbe(DiagnosticDeclProbe::new(
                    self.dag_id.clone(),
                    name.clone(),
                ))
            },
            |identity| DeclarationIdentityLookup::Bound(identity.clone()),
        )
    }

    /// Borrow the declaration identity bound to a source-facing name, for
    /// tests that name a declaration by its spelling. Production code
    /// resolves declarations by identity.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn bound_decl_identity(&self, name: &ScopedName) -> Option<&ResolvedDeclName> {
        self.semantic.decl_bindings.get(name)
    }
}

/// A HIR DAG whose complete declaration signature has been resolved once.
///
/// Owning the HIR body prevents a signature from being paired with another DAG
/// between the signature and body passes of project TIR construction.
#[derive(Debug)]
pub struct SignatureResolvedHirDag {
    hir: HirDag,
    decl_types: HashMap<ResolvedDeclName, CheckedDeclType>,
}

impl SignatureResolvedHirDag {
    /// Borrow the canonical DAG identity.
    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        self.hir.dag_id()
    }

    /// Borrow HIR while project import interfaces are assembled.
    #[must_use]
    pub const fn hir(&self) -> &HirDag {
        &self.hir
    }

    /// Borrow the checked value-declaration types produced by the signature
    /// pass, keyed by canonical identity.
    #[must_use]
    pub const fn decl_types(&self) -> &HashMap<ResolvedDeclName, CheckedDeclType> {
        &self.decl_types
    }
}

/// Resolve one HIR DAG's complete declaration signature before checking bodies.
///
/// # Errors
///
/// Returns a [`SemanticError`] when a declaration annotation cannot be
/// resolved to a concrete declared type.
pub fn resolve_hir_signature_with_modules_and_cancellation(
    hir: HirDag,
    src: SourceId,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<SignatureResolvedHirDag, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let ctx = ModuleTypeContext::new(hir.module(), module_resolver, project_types);
    let decl_types = resolve_declared_type_exprs(&hir, src, ctx, cancellation)?;
    Ok(SignatureResolvedHirDag { hir, decl_types })
}

/// Resolve a HIR DAG without imports into a draft whose only DAG is its root.
#[cfg(test)]
pub(crate) fn type_resolve_draft(
    dag: HirDag,
    src: SourceId,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
) -> Result<TirDraft, SemanticError> {
    crate::outcome::without_cancellation(|cancellation| {
        let signed = resolve_hir_signature_with_modules_and_cancellation(
            dag,
            src,
            module_resolver,
            &project_types,
            cancellation,
        )?;
        TirDraft::resolve_root(
            signed,
            &|_| None,
            src,
            module_resolver,
            project_types,
            cancellation,
        )
    })
}

impl TirDraft {
    /// Resolve a signature-complete root HIR module's bodies into a draft.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] when an imported interface is inconsistent
    /// or body semantic resolution fails.
    pub fn resolve_root(
        signed: SignatureResolvedHirDag,
        imported_types: &ImportedConstantTypes<'_>,
        src: SourceId,
        module_resolver: &ModuleResolver,
        project_types: Arc<ProjectTypeStore>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<Self, Outcome<SemanticError>> {
        cancellation.checkpoint()?;
        let imported_bindings = checked_imported_bindings(signed.hir(), imported_types, src)?;
        let context_types = Arc::clone(&project_types);
        let ctx = ModuleTypeContext::new(signed.hir().module(), module_resolver, &context_types);
        type_resolve_impl(
            signed,
            imported_bindings,
            src,
            ctx,
            project_types,
            cancellation,
        )
    }

    /// Resolve a signature-complete same-file inline DAG's bodies and add it
    /// to this draft, merging the extern signatures its own `import plugin`
    /// blocks declare.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] when a declared extern signature conflicts,
    /// an imported interface is inconsistent, or body semantic resolution
    /// fails.
    pub fn add_inline_dag(
        &mut self,
        signed: SignatureResolvedHirDag,
        imported_types: &ImportedConstantTypes<'_>,
        src: SourceId,
        module_resolver: &ModuleResolver,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<(), Outcome<SemanticError>> {
        cancellation.checkpoint()?;
        // A nested DAG's `import plugin` signatures join the file's extern
        // map, exactly like the root body's, so calls inside it resolve.
        self.merge_declared_extern_functions(signed.hir(), src)?;
        let project_types = self.project_types();
        let dag = type_resolve_signed_single_with_imported_bindings_and_cancellation(
            signed,
            imported_types,
            src,
            module_resolver,
            &project_types,
            cancellation,
        )?;
        self.insert_dag(dag)
            .map_err(|error| {
                SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
            })
            .map_err(Outcome::Failed)
    }

    /// Complete assembly and materialize every semantic include edge as a
    /// concrete instance DAG.
    ///
    /// `overrides` refines the include override obligations of every local
    /// body (materialized instances included) with the canonical dependency
    /// summaries of already-checked source modules.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] when an instance cannot be specialized.
    pub fn instantiate(
        self,
        overrides: &CheckedOverrideDependencies,
        src: SourceId,
    ) -> Result<InstantiatedTir, SemanticError> {
        let mut tir = self.finish();
        specialization::instantiate_semantic_edges(&mut tir, src)?;
        augment_runtime_deps_for_dynamic_units(&mut tir);
        overrides.reconcile(&mut tir);
        Ok(InstantiatedTir { tir })
    }
}

/// Attach the checked declared type of every constant `ir` imports, keyed and
/// targeted exactly as HIR recorded them, so the bindings cover HIR's imports
/// by construction.
fn checked_imported_bindings(
    ir: &HirDag,
    imported_types: &ImportedConstantTypes<'_>,
    src: SourceId,
) -> Result<HashMap<ScopedName, ImportedBinding>, SemanticError> {
    ir.imported_bindings()
        .iter()
        .map(|(lexical, target)| {
            let declared_type = imported_types(target).ok_or_else(|| {
                SemanticError::internal_error(
                    format!(
                        "checked interface for HIR import `{lexical}` targeting `{target}` is unavailable"
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            Ok((
                lexical.clone(),
                ImportedBinding::new(target.clone(), declared_type),
            ))
        })
        .collect()
}

fn finalize_hir_dag(
    dag: &mut DagTIR,
    surface: &ExternalDeclSurface,
    module_ctx: ModuleTypeContext<'_>,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(), Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    dag.populate_projectable_outputs(surface);
    cancellation.checkpoint()?;
    validate_public_generic_defaults(dag, surface, module_ctx, src)?;
    cancellation.checkpoint()?;
    check_hir_body_policies(dag, surface, module_ctx, src).map_err(Outcome::Failed)
}

fn type_resolve_impl(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
    src: SourceId,
    module_ctx: ModuleTypeContext<'_>,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TirDraft, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let SignatureResolvedHirDag {
        hir: mut ir,
        decl_types,
    } = signed;
    let imported_bindings_for_hir = imported_bindings.clone();
    let (decls, domain_bounds) =
        attach_checked_types(std::mem::take(&mut ir.decls), decl_types, src)?;
    let mut root_dag = type_resolve_dag(
        decls,
        src,
        module_ctx.owner,
        module_ctx,
        &imported_bindings_for_hir,
        domain_bounds,
        cancellation,
    )?
    .with_body(
        HirBody {
            included_plots: ir.included_plots,
            static_ports: ir.static_ports,
            assumes_map: ir.assumes_map,
            expected_fail: ir.expected_fail,
            dynamic_unit_scales: ir.dynamic_unit_scales,
            semantic_instances: ir.semantic_instances,
        },
        imported_bindings,
        module_ctx,
        src,
    )?;
    finalize_hir_dag(
        &mut root_dag,
        &ir.external_surface,
        module_ctx,
        src,
        cancellation,
    )?;
    Ok(TirDraft::new(
        crate::display::formatting_registry::FormattingRegistry::new(
            project_types.base_dimensions().clone(),
            ir.display_dimensions,
        ),
        project_types,
        root_dag,
        ir.extern_functions,
    ))
}

/// Resolve type annotations for one DAG body without imports.
#[cfg(test)]
pub(crate) fn type_resolve_single_with_modules(
    dag: HirDag,
    src: SourceId,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
) -> Result<DagTIR, SemanticError> {
    crate::outcome::without_cancellation(|cancellation| {
        let signed = resolve_hir_signature_with_modules_and_cancellation(
            dag,
            src,
            module_resolver,
            project_types,
            cancellation,
        )?;
        type_resolve_signed_single_with_imported_bindings_and_cancellation(
            signed,
            &|_| None,
            src,
            module_resolver,
            project_types,
            cancellation,
        )
    })
}

/// Resolve a signature-complete non-root HIR module's bodies.
fn type_resolve_signed_single_with_imported_bindings_and_cancellation(
    signed: SignatureResolvedHirDag,
    imported_types: &ImportedConstantTypes<'_>,
    src: SourceId,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let imported_bindings = checked_imported_bindings(signed.hir(), imported_types, src)?;
    let ctx = ModuleTypeContext::new(signed.hir().module(), module_resolver, project_types);
    type_resolve_single_impl(signed, imported_bindings, src, ctx, cancellation)
}

fn type_resolve_single_impl(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
    src: SourceId,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let SignatureResolvedHirDag {
        hir: mut ir,
        decl_types,
    } = signed;
    let imported_bindings_for_hir = imported_bindings.clone();
    let (decls, domain_bounds) =
        attach_checked_types(std::mem::take(&mut ir.decls), decl_types, src)?;
    let mut dag = type_resolve_dag(
        decls,
        src,
        module_ctx.owner,
        module_ctx,
        &imported_bindings_for_hir,
        domain_bounds,
        cancellation,
    )?
    .with_body(
        HirBody {
            included_plots: ir.included_plots,
            static_ports: ir.static_ports,
            assumes_map: ir.assumes_map,
            expected_fail: ir.expected_fail,
            dynamic_unit_scales: ir.dynamic_unit_scales,
            semantic_instances: ir.semantic_instances,
        },
        imported_bindings,
        module_ctx,
        src,
    )?;
    finalize_hir_dag(
        &mut dag,
        &ir.external_surface,
        module_ctx,
        src,
        cancellation,
    )?;
    Ok(dag)
}

/// Resolve every const/param/node annotation of one HIR DAG.
fn resolve_declared_type_exprs(
    hir: &HirDag,
    src: SourceId,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<ResolvedDeclName, CheckedDeclType>, Outcome<SemanticError>> {
    let decls = hir.decls();
    resolve_declared_types(
        decls
            .consts()
            .map(|entry| (entry.identity(), &entry.type_ann.decl_type))
            .chain(
                decls
                    .params()
                    .map(|entry| (entry.identity(), &entry.type_ann.decl_type)),
            )
            .chain(
                decls
                    .nodes()
                    .map(|entry| (entry.identity(), &entry.type_ann.decl_type)),
            ),
        &hir.semantic_instances,
        module_ctx.types,
        src,
        cancellation,
    )
}

/// Resolve declaration annotations in the type view `types`.
///
/// A value projected from a semantic include instance keeps the annotation of
/// its template declaration, resolved in the template's scope (with the
/// defaulted dimension ports the include binds kept rigid). Its declared type
/// is that annotation specialized through the instance's Static substitution,
/// exactly as the instance output itself is specialized.
fn resolve_declared_types<'d>(
    declarations: impl Iterator<Item = (ResolvedDeclName, &'d crate::hir::types::DeclType)>,
    semantic_instances: &[crate::ir::instance::HirInstanceRecord],
    types: &ProjectTypeStore,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<ResolvedDeclName, CheckedDeclType>, Outcome<SemanticError>> {
    let projection_substitutions = semantic_instances
        .iter()
        .flat_map(|record| {
            record
                .output_projections
                .iter()
                .filter_map(|projection| {
                    crate::ir::instance::InstanceProjection::exposure(projection).selected()
                })
                .map(|exposed| (exposed, record.instance.substitution()))
        })
        .collect::<HashMap<_, _>>();
    let mut views = HashMap::new();
    for substitution in projection_substitutions.values() {
        views.insert(*substitution, instance_type_view(types, substitution, src)?);
    }
    let mut resolved = Vec::new();
    for (identity, decl_type) in declarations {
        cancellation.checkpoint()?;
        let ty = match projection_substitutions.get(&identity.to_unowned_def_name()) {
            Some(substitution) => {
                let view = views.get(substitution).map_or(types, AsRef::as_ref);
                resolve_instance_decl_type(decl_type, substitution, view, types, src)?
            }
            None => type_expr::resolve_hir_decl_type_with_project_types(decl_type, src, types)?,
        };
        resolved.push((identity, ty));
    }
    // Every annotation resolves before any is required to be concrete.
    resolved
        .into_iter()
        .map(|(identity, ty)| CheckedDeclType::new(ty, src).map(|checked| (identity, checked)))
        .collect::<Result<_, SemanticError>>()
        .map_err(Outcome::Failed)
}

/// The type view a template annotation is resolved in for an include with
/// `substitution`: the defaulted dimension ports it binds stay rigid, so the
/// substitution reaches every dimension defined over them.
fn instance_type_view<'s>(
    types: &'s ProjectTypeStore,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    src: SourceId,
) -> Result<std::borrow::Cow<'s, ProjectTypeStore>, SemanticError> {
    let ports = types.bound_defaulted_dimension_ports(substitution);
    if ports.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(types));
    }
    types
        .with_rigid_dimensions(&ports)
        .map(std::borrow::Cow::Owned)
        .map_err(|_| {
            SemanticError::located(src, src.whole_span(), DimensionError::DimensionOverflow)
        })
}

/// Resolve a template annotation in `view` (from [`instance_type_view`]) and
/// specialize it through the include's `substitution`.
fn resolve_instance_decl_type(
    decl_type: &crate::hir::types::DeclType,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    view: &ProjectTypeStore,
    types: &ProjectTypeStore,
    src: SourceId,
) -> Result<ResolvedDeclType, SemanticError> {
    let template_type = type_expr::resolve_hir_decl_type_with_project_types(decl_type, src, view)?;
    specialization::specialize_type(&template_type, substitution, types, src)
}

/// Declaration domain bounds keyed by the canonical declaration they bound.
type DomainBounds = HashMap<ResolvedDeclName, NonEmpty<ResolvedDomainBound>>;

/// Attach each value declaration's checked type to its record, moving its
/// domain bounds into a table keyed by canonical identity.
fn attach_checked_types(
    decls: crate::ir::decl_table::DeclTable<crate::ir::model::Lowered>,
    mut decl_types: HashMap<ResolvedDeclName, CheckedDeclType>,
    src: SourceId,
) -> Result<(crate::ir::decl_table::DeclTable<Typed>, DomainBounds), SemanticError> {
    use crate::ir::entry::{
        AssertEntry, ConstEntry, Decl, FigureEntry, LayerEntry, NodeEntry, ParamEntry, PlotEntry,
    };
    let mut domain_bounds = HashMap::new();
    let mut check = |identity: ResolvedDeclName,
                     annotation: crate::hir::type_annotation::TypeAnnotation| {
        let checked = decl_types.remove(&identity).ok_or_else(|| {
            SemanticError::internal_error(
                format!("value declaration `{identity}` has no resolved signature"),
                src,
                DiagnosticAnchor::Source(annotation.span),
            )
        })?;
        if let Ok(bounds) = NonEmpty::try_from_vec(annotation.domain_bounds) {
            domain_bounds.insert(
                identity,
                bounds.map(|bound| ResolvedDomainBound {
                    kind: bound.kind,
                    value: bound.value,
                    span: bound.span,
                    src,
                }),
            );
        }
        Ok::<_, SemanticError>(CheckedTypeAnnotation {
            decl_type: annotation.decl_type,
            span: annotation.span,
            checked,
        })
    };
    let decls = decls.try_map(
        |_| (),
        |decl| {
            let identity = decl.identity();
            Ok::<_, SemanticError>(match decl {
                Decl::Const(entry) => Decl::Const(ConstEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    identity: entry.identity,
                    expr: entry.expr,
                    span: entry.span,
                }),
                Decl::Param(entry) => Decl::Param(ParamEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    identity: entry.identity,
                    default: entry.default,
                    span: entry.span,
                    override_reconciliations: entry.override_reconciliations,
                }),
                Decl::Node(entry) => Decl::Node(NodeEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    identity: entry.identity,
                    definition: entry.definition,
                    span: entry.span,
                }),
                Decl::Assert(entry) => Decl::Assert(AssertEntry {
                    identity: entry.identity,
                    body: entry.body,
                    span: entry.span,
                }),
                Decl::Plot(entry) => Decl::Plot(PlotEntry {
                    identity: entry.identity,
                    mark_type: entry.mark_type,
                    body: entry.body,
                    visibility: entry.visibility,
                }),
                Decl::Figure(entry) => Decl::Figure(FigureEntry {
                    identity: entry.identity,
                    plot_names: entry.plot_names,
                    fields: entry.fields,
                }),
                Decl::Layer(entry) => Decl::Layer(LayerEntry {
                    identity: entry.identity,
                    plot_names: entry.plot_names,
                    fields: entry.fields,
                }),
            })
        },
    )?;
    Ok((decls, domain_bounds))
}

/// Internal helper: resolve type annotations for the const/param/node
/// declarations of a single DAG, returning a partially-built [`DagTIR`].
fn type_resolve_dag(
    decls: crate::ir::decl_table::DeclTable<Typed>,
    src: SourceId,
    dag_id: &crate::dag_id::DagId,
    module_ctx: ModuleTypeContext<'_>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    domain_bounds: DomainBounds,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIRSeed, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    // A type-resolved module or inline DAG is canonical: it runs the bodies
    // it defines.
    let frame =
        crate::ir::instance::frame::InstanceFrame::canonical(frame_mint::CanonicalFrameMint(()));
    let dependencies = collect_resolved_dag_dependencies(&decls, &frame, module_ctx, src)?;
    cancellation.checkpoint()?;
    let override_reconciliations = override_reconciliations(decls.params());
    cancellation.checkpoint()?;
    let type_defs = collect_resolved_type_defs(
        decls
            .consts()
            .map(|entry| &entry.type_ann)
            .chain(decls.params().map(|entry| &entry.type_ann))
            .chain(decls.nodes().map(|entry| &entry.type_ann)),
        imported_bindings,
        module_ctx,
    )?;
    let bindable_nominals = collect_bindable_nominals(module_ctx);

    let semantic = DagSemanticBody {
        domain_bounds,
        dynamic_unit_scales: HashMap::new(),
        dependencies,
        override_reconciliations,
        bindable_nominals,
        type_defs,
        decl_bindings: HashMap::new(),
    };

    Ok(DagTIRSeed {
        dag_id: dag_id.clone(),
        decls,
        semantic,
        frame,
    })
}

fn collect_bindable_nominals(ctx: ModuleTypeContext<'_>) -> HashSet<BindableNominalIdentity> {
    let symbols = ctx.symbols();
    symbols
        .indexes()
        .values()
        .filter(|symbol| symbol.visibility().is_bindable())
        .map(|symbol| BindableNominalIdentity::Index(symbol.resolved().clone()))
        .chain(
            symbols
                .struct_types()
                .values()
                .filter(|symbol| symbol.visibility().is_bindable())
                .map(|symbol| BindableNominalIdentity::Type(symbol.resolved().clone())),
        )
        .collect()
}

fn override_reconciliations<'a>(
    params: impl Iterator<Item = &'a TypedParamEntry>,
) -> HashMap<ResolvedDeclName, Vec<OverrideReconciliation>> {
    params
        .filter(|entry| !entry.override_reconciliations.is_empty())
        .map(|entry| (entry.identity(), entry.override_reconciliations.clone()))
        .collect()
}

fn collect_resolved_type_defs<'a>(
    annotations: impl Iterator<Item = &'a CheckedTypeAnnotation>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeDefs, SemanticError> {
    let mut defs = ResolvedTypeDefs::default();
    let mut collector = TypeDefCollector::new(ctx, &mut defs);
    for symbol in ctx.symbols().struct_types().values() {
        collector.record(symbol.resolved())?;
    }
    for annotation in annotations {
        collector.resolved_type(annotation.checked().resolved().element())?;
    }
    for binding in imported_bindings.values() {
        collector.declared_type(binding.declared_type())?;
    }
    Ok(defs)
}

#[derive(Debug, Clone)]
enum PublicSignatureDependency {
    Dimension(Spanned<ResolvedDimName>),
    Index(Spanned<ResolvedIndexName>),
    Type(Spanned<ResolvedStructTypeName>),
}

impl PublicSignatureDependency {
    const fn span(&self) -> Span {
        match self {
            Self::Dimension(name) => name.span,
            Self::Index(name) => name.span,
            Self::Type(name) => name.span,
        }
    }

    fn name(&self) -> crate::syntax::names::NameAtom {
        match self {
            Self::Dimension(name) => name.value.atom().clone(),
            Self::Index(name) => name.value.atom().clone(),
            Self::Type(name) => name.value.atom().clone(),
        }
    }

    const fn kind(&self) -> crate::declaration_kind::DeclarationKind {
        match self {
            Self::Dimension(_) => crate::declaration_kind::DeclarationKind::Dimension,
            Self::Index(_) => crate::declaration_kind::DeclarationKind::Index,
            Self::Type(_) => crate::declaration_kind::DeclarationKind::Type,
        }
    }

    fn is_public(&self, resolver: &ModuleResolver) -> Option<bool> {
        let visibility = match self {
            Self::Dimension(name) => resolver.symbol(&name.value).map(SymbolRef::visibility),
            Self::Index(name) => resolver.symbol(&name.value).map(SymbolRef::visibility),
            Self::Type(name) => resolver.symbol(&name.value).map(SymbolRef::visibility),
        };
        visibility.map(crate::syntax::ast::BindableVisibility::is_public)
    }

    const fn owner(&self) -> &crate::dag_id::DagId {
        match self {
            Self::Dimension(name) => name.value.owner(),
            Self::Index(name) => name.value.owner(),
            Self::Type(name) => name.value.owner(),
        }
    }
}

fn collect_public_signature_generic_arg_dependencies(
    arg: &crate::hir::types::GenericArg,
    defs: &ResolvedTypeDefs,
    visited_defaults: &mut std::collections::HashSet<GenericParamId>,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    match arg {
        crate::hir::types::GenericArg::Dim(arg) => {
            collect_public_signature_dim_arg_dependencies(arg, dependencies);
        }
        crate::hir::types::GenericArg::Index(index) => {
            collect_public_signature_index_dependencies(index, dependencies);
        }
        crate::hir::types::GenericArg::Nat(_) => {}
        crate::hir::types::GenericArg::Type(type_expr) => {
            collect_public_signature_type_dependencies(
                type_expr,
                defs,
                visited_defaults,
                dependencies,
            );
        }
    }
}

fn collect_public_signature_dim_arg_dependencies(
    arg: &crate::hir::types::DimArg,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    let crate::hir::types::DimArg::Expr(expr) = arg else {
        return;
    };
    dependencies.extend(
        expr.terms
            .iter()
            .filter_map(|item| match &item.term.target {
                crate::hir::types::DimTermTarget::Dimension(name) => {
                    Some(PublicSignatureDependency::Dimension(name.clone()))
                }
                crate::hir::types::DimTermTarget::GenericParam(_) => None,
            }),
    );
}

fn collect_public_signature_index_dependencies(
    index: &crate::hir::types::IndexRef,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    if let crate::hir::types::IndexRef::Concrete(name) = index {
        dependencies.push(PublicSignatureDependency::Index(name.clone()));
    }
}

fn collect_public_signature_type_dependencies(
    value_type: &crate::hir::types::ValueType,
    defs: &ResolvedTypeDefs,
    visited_defaults: &mut std::collections::HashSet<GenericParamId>,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    match &value_type.kind {
        crate::hir::types::ValueTypeKind::Builtin(_)
        | crate::hir::types::ValueTypeKind::GenericTypeParam(_) => {}
        crate::hir::types::ValueTypeKind::DimExpr(expr) => {
            dependencies.extend(
                expr.terms
                    .iter()
                    .filter_map(|item| match &item.term.target {
                        crate::hir::types::DimTermTarget::Dimension(name) => {
                            Some(PublicSignatureDependency::Dimension(name.clone()))
                        }
                        crate::hir::types::DimTermTarget::GenericParam(_) => None,
                    }),
            );
        }
        crate::hir::types::ValueTypeKind::Key(index) => {
            collect_public_signature_index_dependencies(index, dependencies);
        }
        crate::hir::types::ValueTypeKind::Struct(name) => {
            dependencies.push(PublicSignatureDependency::Type(name.clone()));
        }
        crate::hir::types::ValueTypeKind::Complex(arg) => {
            collect_public_signature_dim_arg_dependencies(arg, dependencies);
        }
        crate::hir::types::ValueTypeKind::TypeApplication { name, generic_args } => {
            dependencies.push(PublicSignatureDependency::Type(name.clone()));
            for arg in generic_args {
                collect_public_signature_generic_arg_dependencies(
                    arg,
                    defs,
                    visited_defaults,
                    dependencies,
                );
            }
            if let Some(type_def) = defs.nominal(&name.value).map(ResolvedNominal::definition) {
                for param in type_def.generic_params().iter().skip(generic_args.len()) {
                    let key = param.id().clone();
                    if !visited_defaults.insert(key) {
                        continue;
                    }
                    if let Some(default) = param.default() {
                        collect_public_signature_generic_arg_dependencies(
                            default,
                            defs,
                            visited_defaults,
                            dependencies,
                        );
                    }
                }
            }
        }
    }
}

/// Validate that defaults in every public generic type are closed over public
/// canonical type-system declarations.
///
/// HIR is the validation boundary: aliases have already resolved to their exact
/// owner and lexical generic parameters are distinct from module declarations.
/// This avoids both spelling collisions and false positives from a generic
/// parameter shadowing a local private declaration.
fn validate_public_generic_defaults(
    dag: &DagTIR,
    external_surface: &ExternalDeclSurface,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
) -> Result<(), SemanticError> {
    for symbol in ctx.symbols().struct_types().values() {
        let type_name = symbol.resolved();
        if type_name.owner() != ctx.owner
            || !external_surface.is_static_explicit_export(type_name.atom())
        {
            continue;
        }
        // A type the project store does not define has no recorded nominal.
        let Some(nominal) = dag.semantic.type_defs.nominal(type_name) else {
            continue;
        };
        let type_def = nominal.definition();
        let pub_span = symbol.span();
        for param in type_def.generic_params() {
            let Some(default) = param.default() else {
                continue;
            };
            let key = param.id().clone();
            let mut dependencies = Vec::new();
            let mut visited_defaults = std::collections::HashSet::from([key]);
            collect_public_signature_generic_arg_dependencies(
                default,
                &dag.semantic.type_defs,
                &mut visited_defaults,
                &mut dependencies,
            );
            for dependency in dependencies {
                if dependency.owner() == &crate::semantic::prelude::prelude_dag_id() {
                    continue;
                }
                match dependency.is_public(ctx.resolver) {
                    Some(true) => {}
                    Some(false) => {
                        return Err(SemanticError::located(
                            src,
                            dependency.span(),
                            VisibilityError::PrivateInPublic {
                                pub_kind: crate::declaration_kind::DeclarationKind::Type,
                                pub_name: type_name.atom().clone(),
                                ref_kind: dependency.kind(),
                                ref_name: dependency.name(),
                                pub_span,
                            },
                        ));
                    }
                    None => {
                        return Err(SemanticError::internal_error(
                            format!(
                                "canonical {} `{}` is missing visibility metadata",
                                dependency.kind(),
                                dependency.name()
                            ),
                            src,
                            crate::diagnostic_anchor::DiagnosticAnchor::Source(dependency.span()),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Records nominal types into one DAG's type definitions, together with every
/// type their generic defaults and fields name.
///
/// Model-schema expansion needs this closure, not only the types named
/// directly by entry declarations: a prepared boundary value can contain
/// nested records whose names never appear in the consumer module.
struct TypeDefCollector<'d, 'c> {
    ctx: ModuleTypeContext<'c>,
    defs: &'d mut ResolvedTypeDefs,
    /// Types whose recording has started, so recursive nominal types
    /// terminate.
    visiting: HashSet<ResolvedStructTypeName>,
}

impl<'d, 'c> TypeDefCollector<'d, 'c> {
    fn new(ctx: ModuleTypeContext<'c>, defs: &'d mut ResolvedTypeDefs) -> Self {
        Self {
            ctx,
            defs,
            visiting: HashSet::new(),
        }
    }

    fn declared_type(
        &mut self,
        declared: &crate::semantic::checked_type::CheckedType,
    ) -> Result<(), SemanticError> {
        match declared {
            crate::semantic::checked_type::CheckedType::Struct(name, generic_args) => {
                self.record(name.resolved())?;
                for arg in generic_args {
                    if let crate::semantic::checked_type::CheckedGenericArg::Type(type_expr) = arg {
                        self.declared_type(type_expr)?;
                    }
                }
            }
            crate::semantic::checked_type::CheckedType::Indexed { element, .. } => {
                self.declared_type(element)?;
            }
            crate::semantic::checked_type::CheckedType::Quantity(_)
            | crate::semantic::checked_type::CheckedType::Complex(_)
            | crate::semantic::checked_type::CheckedType::Bool
            | crate::semantic::checked_type::CheckedType::Int
            | crate::semantic::checked_type::CheckedType::Datetime(_)
            | crate::semantic::checked_type::CheckedType::Key(_) => {}
        }
        Ok(())
    }

    fn resolved_type(&mut self, resolved: &ResolvedValueType) -> Result<(), SemanticError> {
        match resolved {
            ResolvedValueType::Struct {
                name, generic_args, ..
            } => {
                self.record(name)?;
                for arg in generic_args {
                    if let crate::tir::typed::ResolvedGenericArg::Type(type_expr) = arg {
                        self.resolved_type(type_expr)?;
                    }
                }
            }
            ResolvedValueType::Complex { .. }
            | ResolvedValueType::Key { .. }
            | ResolvedValueType::Bool
            | ResolvedValueType::Int
            | ResolvedValueType::Datetime(_)
            | ResolvedValueType::Quantity(_)
            | ResolvedValueType::GenericTypeParam(_, _) => {}
        }
        Ok(())
    }

    fn record(&mut self, name: &ResolvedStructTypeName) -> Result<(), SemanticError> {
        if self.defs.contains(name) || !self.visiting.insert(name.clone()) {
            return Ok(());
        }
        let ctx = self.ctx;
        let Some(type_def) = ctx.types.get_struct_type_handle(name).cloned() else {
            return Ok(());
        };
        let definition_src = type_def.source();

        for param in type_def.generic_params() {
            if let Some(default) = param.default() {
                let resolved = resolve_hir_generic_arg(param, default, definition_src, ctx)?;
                if let ResolvedGenericArg::Type(type_expr) = &resolved {
                    self.resolved_type(type_expr)?;
                }
                self.defs
                    .generic_defaults
                    .insert(param.id().clone(), ResolvedGenericDefault { resolved });
            }
        }

        let instance_view = type_def
            .instance_substitution()
            .map(|substitution| {
                instance_type_view(ctx.types, substitution, definition_src)
                    .map(|view| (substitution, view))
            })
            .transpose()?;
        let nominal = ResolvedNominal::try_resolve(Arc::clone(&type_def), |_, field| {
            let annotation = field.type_annotation();
            let resolved = match &instance_view {
                Some((substitution, view)) => resolve_instance_decl_type(
                    &annotation.decl_type,
                    substitution,
                    view,
                    ctx.types,
                    definition_src,
                )?,
                None => resolve_hir_decl_type(&annotation.decl_type, definition_src, ctx)?,
            };
            let bounds = annotation
                .domain_bounds
                .iter()
                .map(|bound| ResolvedDomainBound {
                    kind: bound.kind,
                    value: bound.value.clone(),
                    span: bound.span,
                    src: definition_src,
                })
                .collect();
            self.resolved_type(resolved.element())?;
            Ok(ResolvedStructFieldSemantics::new(resolved, bounds))
        })?;
        self.defs.insert(nominal);
        Ok(())
    }
}

/// HIR-level body policies that replaced the retired syntax-AST scope checks.
///
/// Walks every lowered body of one DAG and enforces:
/// - const bodies and every domain bound must not `@`-reference runtime
///   declarations (E020-style [`NameError::GraphRefInConst`](NameError::GraphRefInConst)) or use runtime
///   units in literals / conversion targets;
/// - no body may `@`-reference an assert declaration
///   ([`AttributeError::GraphRefToAssert`](AttributeError::GraphRefToAssert));
/// - A10(c) / V004: bodies of non-bindable kinds owned by this module must
///   not mention variant literals of the module's own `pub(bind)` indexes
///   ([`VisibilityError::PubIndexVariantLiteral`](VisibilityError::PubIndexVariantLiteral)). Params are exempt (A10(a));
///   sink kinds (assert/plot/figure/layer) are checked only when `pub`.
fn check_hir_body_policies(
    dag: &DagTIR,
    external_surface: &ExternalDeclSurface,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
) -> Result<(), SemanticError> {
    let semantic = &dag.semantic;
    let local = |key: &ResolvedDeclName| key.owner() == ctx.owner;

    for entry in dag.consts() {
        let key = entry.identity();
        HirPolicyChecker {
            ctx,
            src,
            frame: dag.frame(),
        }
        .check_expr(&entry.expr, BodyPhase::CompileTime, local(&key))?;
    }
    check_domain_bound_policies(semantic, dag.frame(), ctx)?;
    check_dynamic_unit_policies(semantic, dag.frame(), ctx)?;
    for entry in dag.nodes() {
        let key = entry.identity();
        entry.definition.formula().map_or(Ok(()), |expression| {
            HirPolicyChecker {
                ctx,
                src,
                frame: dag.frame(),
            }
            .check_expr(expression, BodyPhase::Runtime, local(&key))
        })?;
    }
    for entry in dag.params() {
        let Some(default) = &entry.default else {
            continue;
        };
        // Params are exempt from A10 (a rebinding importer is forced to
        // rebind the param too — V005 at the include site).
        HirPolicyChecker {
            ctx,
            src,
            frame: dag.frame(),
        }
        .check_expr(default, BodyPhase::Runtime, false)?;
    }
    check_sink_body_policies(dag, external_surface, ctx, src)
}

fn check_domain_bound_policies(
    semantic: &DagSemanticBody,
    frame: &crate::ir::instance::frame::InstanceFrame,
    ctx: ModuleTypeContext<'_>,
) -> Result<(), SemanticError> {
    let check_bounds = |bounds: &[ResolvedDomainBound],
                        check_pub_bind_literals: bool|
     -> Result<(), SemanticError> {
        for bound in bounds {
            HirPolicyChecker {
                ctx,
                src: bound.src,
                frame,
            }
            .check_expr(
                &bound.value,
                BodyPhase::CompileTime,
                check_pub_bind_literals,
            )?;
            // Domain bounds are evaluated without a host function registry.
            if let Some((external, span)) = crate::hir::expr::find_extern_call(&bound.value) {
                return Err(SemanticError::located(
                    bound.src,
                    span,
                    PluginError::ExternCallNotAllowed {
                        name: external.clone(),
                        context: ExternCallContext::DomainBound,
                    },
                ));
            }
        }
        Ok(())
    };
    for (key, bounds) in &semantic.domain_bounds {
        check_bounds(bounds.as_slice(), key.owner() == ctx.owner)?;
    }
    for field in semantic.type_defs.constrained_fields() {
        check_bounds(field.bounds().as_slice(), false)?;
    }
    Ok(())
}

fn check_dynamic_unit_policies(
    semantic: &DagSemanticBody,
    frame: &crate::ir::instance::frame::InstanceFrame,
    ctx: ModuleTypeContext<'_>,
) -> Result<(), SemanticError> {
    // Dynamic unit scales may read runtime params/nodes, but otherwise obey
    // ordinary runtime-body policy (notably, assertions cannot be read).
    // They also resolve in contexts with no host-function registry.
    for entry in semantic.dynamic_unit_scales.values() {
        HirPolicyChecker {
            ctx,
            src: entry.src,
            frame,
        }
        .check_expr(&entry.expr, BodyPhase::Runtime, false)?;
        if let Some((external, span)) = crate::hir::expr::find_extern_call(&entry.expr) {
            return Err(SemanticError::located(
                entry.src,
                span,
                PluginError::ExternCallNotAllowed {
                    name: external.clone(),
                    context: ExternCallContext::UnitScaleExpression,
                },
            ));
        }
    }
    Ok(())
}

fn check_sink_body_policies(
    dag: &DagTIR,
    external_surface: &ExternalDeclSurface,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
) -> Result<(), SemanticError> {
    let is_explicit_export = |name: &DeclName| external_surface.is_explicit_export(name);
    for entry in dag.asserts() {
        let check_literals =
            entry.identity.owner() == ctx.owner && is_explicit_export(entry.name());
        let checker = HirPolicyChecker {
            ctx,
            src,
            frame: dag.frame(),
        };
        match &*entry.body {
            crate::hir::expr::AssertBody::Expr(expr) => {
                checker.check_expr(expr, BodyPhase::Runtime, check_literals)?;
            }
            crate::hir::expr::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                checker.check_expr(actual, BodyPhase::Runtime, check_literals)?;
                checker.check_expr(expected, BodyPhase::Runtime, check_literals)?;
                checker.check_expr(tolerance, BodyPhase::Runtime, check_literals)?;
            }
        }
    }
    for entry in dag.plots() {
        let body = &entry.body;
        let check_literals = is_explicit_export(entry.name());
        let checker = HirPolicyChecker {
            ctx,
            src,
            frame: dag.frame(),
        };
        for (_, expr) in &body.encodings {
            checker.check_expr(expr, BodyPhase::Runtime, check_literals)?;
        }
        for value in body.property_values() {
            checker.check_expr(value, BodyPhase::Runtime, check_literals)?;
        }
    }
    for (name, fields) in dag
        .figures()
        .map(|entry| (entry.name(), &entry.fields))
        .chain(dag.layers().map(|entry| (entry.name(), &entry.fields)))
    {
        let check_literals = is_explicit_export(name);
        let checker = HirPolicyChecker {
            ctx,
            src,
            frame: dag.frame(),
        };
        for field in fields {
            checker.check_expr(&field.value, BodyPhase::Runtime, check_literals)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyPhase {
    CompileTime,
    Runtime,
}

impl BodyPhase {
    const fn is_compile_time(self) -> bool {
        matches!(self, Self::CompileTime)
    }
}

struct HirPolicyChecker<'a> {
    ctx: ModuleTypeContext<'a>,
    src: SourceId,
    /// The frame of the DAG whose bodies are checked.
    frame: &'a crate::ir::instance::frame::InstanceFrame,
}

impl HirPolicyChecker<'_> {
    fn check_expr(
        &self,
        expr: &crate::hir::expr::Expr,
        phase: BodyPhase,
        check_pub_bind_literals: bool,
    ) -> Result<(), SemanticError> {
        // Recursion choke point: recurses once per tree level.
        crate::stack::with_stack_growth(|| {
            self.check_expr_inner(expr, phase, check_pub_bind_literals)
        })
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind policy walk")]
    fn check_expr_inner(
        &self,
        expr: &crate::hir::expr::Expr,
        phase: BodyPhase,
        check_pub_bind_literals: bool,
    ) -> Result<(), SemanticError> {
        let recurse =
            |inner: &crate::hir::expr::Expr| self.check_expr(inner, phase, check_pub_bind_literals);
        match expr.kind() {
            crate::hir::expr::ExprKind::Error(no_error) => no_error.absurd(),
            crate::hir::expr::ExprKind::Number(_)
            | crate::hir::expr::ExprKind::Integer(_)
            | crate::hir::expr::ExprKind::Bool(_)
            | crate::hir::expr::ExprKind::StringLiteral(_)
            | crate::hir::expr::ExprKind::OffsetDateTimeLiteral(_)
            | crate::hir::expr::ExprKind::CivilDateTimeLiteral(_)
            | crate::hir::expr::ExprKind::EpochLiteral(_)
            | crate::hir::expr::ExprKind::ZonedDateTimeLiteral(_)
            | crate::hir::expr::ExprKind::IanaTimeZoneLiteral(_)
            | crate::hir::expr::ExprKind::TypeSystemRef(_)
            | crate::hir::expr::ExprKind::ConstRef(_)
            | crate::hir::expr::ExprKind::LocalRef(_) => Ok(()),
            crate::hir::expr::ExprKind::QuantityLiteral { unit, .. } => {
                self.check_unit_expr(unit, phase)
            }
            crate::hir::expr::ExprKind::GraphRef(target) => {
                // Use the whole `@name` span (the reference Spanned covers
                // only the name) so the label includes the sigil.
                self.check_graph_ref(target, expr.span, phase)
            }
            crate::hir::expr::ExprKind::VariantLiteral(variant) => {
                self.check_variant_literal(variant, check_pub_bind_literals)
            }
            crate::hir::expr::ExprKind::BinOp { lhs, rhs, .. } => {
                recurse(lhs)?;
                recurse(rhs)
            }
            crate::hir::expr::ExprKind::UnaryOp { operand, .. }
            | crate::hir::expr::ExprKind::DisplayTimezone { expr: operand, .. }
            | crate::hir::expr::ExprKind::FieldAccess { expr: operand, .. } => recurse(operand),
            crate::hir::expr::ExprKind::Convert {
                expr: operand,
                target,
            } => {
                self.check_unit_expr(target, phase)?;
                recurse(operand)
            }
            crate::hir::expr::ExprKind::FnCall { callee, args, .. } => {
                // Extern functions are runtime-provided; const expressions
                // evaluate at compile time without a host function registry.
                if phase.is_compile_time()
                    && let crate::hir::expr::FunctionRef::External(ext) = &callee.value
                {
                    return Err(SemanticError::located(
                        self.src,
                        callee.span,
                        PluginError::ExternCallNotAllowed {
                            name: ext.clone(),
                            context: ExternCallContext::ConstExpression,
                        },
                    ));
                }
                args.iter().try_for_each(recurse)
            }
            crate::hir::expr::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                recurse(condition)?;
                recurse(then_branch)?;
                recurse(else_branch)
            }
            crate::hir::expr::ExprKind::ConstructorCall { fields, .. } => {
                fields.iter().try_for_each(|field| recurse(&field.value))
            }
            crate::hir::expr::ExprKind::MapLiteral { entries } => {
                for entry in entries {
                    for key in &entry.keys {
                        if let crate::hir::expr::MapEntryKey::IndexVariant(variant) = key {
                            self.check_variant_literal(variant, check_pub_bind_literals)?;
                        }
                    }
                    recurse(&entry.value)?;
                }
                Ok(())
            }
            crate::hir::expr::ExprKind::ForComp { body, .. } => recurse(body),
            crate::hir::expr::ExprKind::IndexAccess { expr: inner, args } => {
                recurse(inner)?;
                for arg in args {
                    match arg {
                        crate::hir::expr::IndexArg::Variant(variant) => {
                            self.check_variant_literal(variant, check_pub_bind_literals)?;
                        }
                        crate::hir::expr::IndexArg::Expr(arg_expr) => recurse(arg_expr)?,
                        crate::hir::expr::IndexArg::Var(_) => {}
                    }
                }
                Ok(())
            }
            crate::hir::expr::ExprKind::Scan {
                source, init, body, ..
            } => {
                recurse(source)?;
                recurse(init)?;
                recurse(body)
            }
            crate::hir::expr::ExprKind::Unfold { init, body, .. } => {
                recurse(init)?;
                recurse(body)
            }
            crate::hir::expr::ExprKind::KeyForm { arg, .. } => recurse(arg),
            crate::hir::expr::ExprKind::Match { scrutinee, arms } => {
                recurse(scrutinee)?;
                for arm in arms {
                    if let crate::hir::expr::MatchPattern::IndexLabel { variant, .. } = &arm.pattern
                    {
                        self.check_variant_literal(variant, check_pub_bind_literals)?;
                    }
                    recurse(&arm.body)?;
                }
                Ok(())
            }
            crate::hir::expr::ExprKind::DagCall { target, args, .. } => {
                if phase.is_compile_time() {
                    return Err(SemanticError::located(
                        self.src,
                        expr.span,
                        GraphError::DagCallInCompileTime {
                            name: target.value.clone(),
                        },
                    ));
                }
                args.iter().try_for_each(|arg| recurse(&arg.value))
            }
        }
    }

    fn check_unit_expr(
        &self,
        unit: &crate::hir::expr::ResolvedUnitExpr,
        phase: BodyPhase,
    ) -> Result<(), SemanticError> {
        if !phase.is_compile_time() {
            return Ok(());
        }
        for term in &unit.terms {
            let Some(info) = self.ctx.types.get_unit(term.name.value.static_definition()) else {
                // Missing semantic unit definitions get their own diagnostics
                // from dimension checking.
                continue;
            };
            if !info.scale.constness().is_const() {
                return Err(SemanticError::located(
                    self.src,
                    term.name.span,
                    DimensionError::NonConstUnitInConst {
                        name: term.name.value.spelling().clone(),
                    },
                ));
            }
        }
        Ok(())
    }

    fn check_graph_ref(
        &self,
        reference: &Spanned<crate::hir::expr::LocalDecl>,
        ref_span: Span,
        phase: BodyPhase,
    ) -> Result<(), SemanticError> {
        let target = self.frame.resolve(&reference.value);
        let Some(kind) = self
            .ctx
            .resolver
            .symbol(&target)
            .map(|symbol| *symbol.kind())
        else {
            // Unknown targets get their own diagnostic from dependency
            // collection; the policy walk only classifies known ones.
            return Ok(());
        };
        if matches!(kind, crate::resolve::category::DeclSymbolKind::Assert) {
            return Err(SemanticError::located(
                self.src,
                ref_span,
                AttributeError::GraphRefToAssert {
                    name: target.to_unowned_def_name(),
                },
            ));
        }
        if phase.is_compile_time() && !kind.is_const() {
            return Err(SemanticError::located(
                self.src,
                ref_span,
                NameError::GraphRefInConst {
                    name: ScopedName::local(target.to_unowned_def_name()),
                },
            ));
        }
        Ok(())
    }

    fn check_variant_literal(
        &self,
        variant: &crate::hir::expr::IndexVariantRef,
        check_pub_bind_literals: bool,
    ) -> Result<(), SemanticError> {
        if !check_pub_bind_literals {
            return Ok(());
        }
        let index = variant.variant.index();
        if index.owner() != self.ctx.owner {
            return Ok(());
        }
        let is_pub_bind = self
            .ctx
            .symbols()
            .indexes()
            .get(&index.to_unowned_def_name())
            .is_some_and(|symbol| {
                // A bindable index with declared variants.
                symbol.visibility().is_bindable() && !symbol.data().is_empty()
            });
        if is_pub_bind {
            return Err(SemanticError::located(
                self.src,
                variant.path_span(),
                VisibilityError::PubIndexVariantLiteral {
                    index: index.to_unowned_def_name(),
                    variant: variant.variant.variant().clone(),
                },
            ));
        }
        Ok(())
    }
}

/// Augment runtime deps with transitive dependencies through dynamic units.
///
/// When a param/node expression references a dynamic unit (via a unit
/// literal or conversion), the `@`-references in that unit's scale
mod collect;
use collect::{
    augment_runtime_deps_for_dynamic_units, collect_constructed_types_from_expr,
    collect_resolved_dag_dependencies,
};

/// The HIR DAG fields beyond the resolved value declarations.
struct HirBody {
    included_plots: Vec<crate::ir::model::IncludedPlotEntry>,
    static_ports: Vec<crate::hir::source_interface::StaticPort>,
    assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    expected_fail: HashMap<ResolvedDeclName, crate::ir::model::ResolvedExpectedFailMetadata>,
    dynamic_unit_scales: Vec<crate::ir::model::DynamicUnitScaleEntry>,
    semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
}

/// Partially-built [`DagTIR`] returned by [`type_resolve_dag`]; finalized
/// by [`DagTIRSeed::with_body`] which fills in the rest of the per-DAG
/// fields.
struct DagTIRSeed {
    dag_id: crate::dag_id::DagId,
    decls: crate::ir::decl_table::DeclTable<Typed>,
    semantic: DagSemanticBody,
    frame: crate::ir::instance::frame::InstanceFrame,
}

impl DagTIRSeed {
    fn with_body(
        self,
        body: HirBody,
        imported_bindings: HashMap<ScopedName, ImportedBinding>,
        module_ctx: ModuleTypeContext<'_>,
        src: SourceId,
    ) -> Result<DagTIR, SemanticError> {
        let HirBody {
            included_plots,
            static_ports,
            assumes_map,
            expected_fail,
            dynamic_unit_scales,
            semantic_instances,
        } = body;
        // The HIR declaration table already binds every local spelling to its
        // canonical identity; imported values add their lexical targets.
        let mut decl_bindings = self
            .decls
            .spelling()
            .iter()
            .map(|(name, identity)| (ScopedName::local(name.clone()), identity.clone()))
            .collect::<HashMap<_, _>>();
        decl_bindings.extend(
            imported_bindings
                .iter()
                .map(|(name, binding)| (name.clone(), binding.target().clone())),
        );

        let mut semantic = self.semantic;
        semantic.decl_bindings = decl_bindings;
        for entry in dynamic_unit_scales {
            let unit = entry.unit.clone();
            let span = entry.span;
            if semantic
                .dynamic_unit_scales
                .insert(unit.clone(), entry)
                .is_some()
            {
                return Err(SemanticError::internal_error(
                    format!("duplicate dynamic unit semantic entry `{unit}`"),
                    src,
                    DiagnosticAnchor::Source(span),
                ));
            }
        }

        let mut dag = DagTIR {
            dag_id: self.dag_id,
            decls: self.decls,
            included_plots,
            semantic,
            static_ports,
            assumes_map,
            expected_fail,
            imported_bindings,
            semantic_instances,
            frame: self.frame,
            projectable_outputs: std::collections::HashSet::new(),
        };
        // The complete owned-root inventory includes nominal bounds, even when
        // a constructor occurs nowhere in a declaration's ordinary value body.
        let mut constructed_types = HashSet::new();
        for root in dag.owned_expression_roots() {
            collect_constructed_types_from_expr(root, module_ctx, src, &mut constructed_types)?;
        }
        let mut collector = TypeDefCollector::new(module_ctx, &mut dag.semantic.type_defs);
        for owning_type in &constructed_types {
            collector.record(owning_type)?;
        }
        Ok(dag)
    }
}

/// Build a temporary TIR view in which the optional dimension `ports` of
/// the DAG at `position` are rigid. The DAG keeps its position in the view.
///
/// The ordinary checked TIR retains default-resolved signatures for parameter
/// default reconciliation. This view re-resolves source-authored declaration
/// signatures against opaque base identities (with every dimension defined
/// over a rigid port recomputed) so Option A can verify executable bodies,
/// and an include binding the ports can specialize them, without mutating the
/// authoritative result.
pub(crate) fn rigid_dimension_view(
    tir: &UncheckedTir,
    position: dag_position::DagPosition,
    ports: &[ResolvedDimName],
    src: SourceId,
) -> Result<UncheckedTir, SemanticError> {
    let rigid_types = tir
        .project_type_store()
        .with_rigid_dimensions(ports)
        .map_err(|_| {
            SemanticError::located(src, src.whole_span(), DimensionError::DimensionOverflow)
        })?;
    let mut rigid = tir.clone();
    for port in ports {
        rigid
            .registry_mut()
            .dimensions
            .register_rigid_dimension(port);
    }
    let rigid_dag = rigid.dags.localized_mut(position);
    let resolved = crate::outcome::without_cancellation(|cancellation| {
        resolve_declared_types(
            rigid_dag
                .value_decl_types()
                .map(|(identity, annotation)| (identity, &annotation.decl_type)),
            &rigid_dag.semantic_instances,
            &rigid_types,
            src,
            cancellation,
        )
    })?;
    rigid_dag.replace_value_decl_types(resolved);
    rigid.replace_project_types(rigid_types);
    Ok(rigid)
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
pub(crate) mod specialization;
mod substitution;
mod type_expr;
pub use substitution::{Substitution, SubstitutionError};
pub use type_expr::resolve_hir_decl_type;
use type_expr::resolve_hir_generic_arg;

#[cfg(test)]
mod tests;
