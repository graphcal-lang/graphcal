//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! For value declarations, the TIR layer consumes canonical HIR type references
//! and resolves them into concrete dimensions, struct types, generic dimension
//! parameters, or generic index parameters. It does not reinterpret source
//! paths from declaration signatures.

use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::generic_param::GenericParamId;
use crate::hir;
pub use crate::ir::lower::{LoweredPlotBody, LoweredPlotField};
pub use crate::nat::NatPolyForm;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::{Span, Spanned};
use miette::NamedSource;

use crate::ir::lower::HirDag;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::resolve::ModuleResolver;
use crate::resolve::symbols::SymbolRef;
use crate::syntax::module_name::ScopedName;

pub mod model;
pub use model::*;
pub mod resolved_type;
pub use resolved_type::*;

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
                .map(|entry| &entry.name)
                .chain(self.decls.nodes().map(|entry| &entry.name))
                .filter(|name| surface.can_select_output(name))
                .cloned(),
        );
    }

    /// Look up the canonical identity recorded for a source-facing name.
    ///
    /// Unknown names remain [`DiagnosticDeclProbe`] values rather than being
    /// assigned an authoritative-looking identity from this DAG's owner.
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

    /// Borrow an authoritative declaration identity, if one is bound.
    #[must_use]
    pub fn bound_decl_identity(&self, name: &ScopedName) -> Option<&ResolvedDeclName> {
        self.semantic.decl_bindings.get(name)
    }

    /// Require an authoritative declaration identity for an invariant-backed
    /// compiler or evaluator operation.
    ///
    /// # Errors
    ///
    /// Returns an internal error instead of converting an unknown diagnostic
    /// probe into a semantic identity.
    pub fn require_bound_decl_identity(
        &self,
        name: &ScopedName,
        src: &NamedSource<Arc<String>>,
        anchor: DiagnosticAnchor,
    ) -> Result<ResolvedDeclName, GraphcalError> {
        self.lookup_decl_identity(name)
            .into_bound()
            .map_err(|probe| GraphcalError::internal_error(probe.to_string(), src, anchor))
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
/// Returns a [`GraphcalError`] when a declaration annotation cannot be
/// resolved to a concrete declared type.
pub fn resolve_hir_signature_with_modules_and_cancellation(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<SignatureResolvedHirDag, GraphcalError> {
    cancellation.checkpoint()?;
    let ctx = ModuleTypeContext::new(hir.dag_id(), module_resolver, project_types);
    let decl_types = resolve_declared_type_exprs(&hir, src, ctx, cancellation)?;
    Ok(SignatureResolvedHirDag { hir, decl_types })
}

/// Resolve all canonical HIR type annotations in an `HirDag` against the
/// authoritative project type store.
///
/// Syntax paths were eliminated while freezing HIR. This conversion therefore
/// reads owner-qualified references directly and never performs source-path
/// lookup or AST-to-HIR lowering.
pub fn type_resolve_with_modules(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
) -> Result<TIR, GraphcalError> {
    type_resolve_with_modules_and_cancellation(
        hir,
        src,
        module_resolver,
        project_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Resolve a HIR DAG to TIR while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid types or cancellation.
pub fn type_resolve_with_modules_and_cancellation(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TIR, GraphcalError> {
    type_resolve_builder_with_modules_and_cancellation(
        hir,
        src,
        module_resolver,
        project_types,
        cancellation,
    )
    .map(TirBuilder::finish)
}

/// Resolve a root DAG into mutable project-assembly state.
///
/// Callers add every file-defined or imported DAG through
/// [`TirBuilder::insert_dag`] and consume the builder with
/// [`TirBuilder::finish`] before checking or evaluation.
pub fn type_resolve_builder_with_modules_and_cancellation(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TirBuilder, GraphcalError> {
    type_resolve_builder_with_imported_bindings_and_cancellation(
        hir,
        HashMap::new(),
        src,
        module_resolver,
        project_types,
        cancellation,
    )
}

/// Resolve a root HIR module after attaching checked imported interfaces.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when an imported lexical target is missing or
/// does not match the canonical target recorded by HIR, or when type
/// resolution otherwise fails.
pub fn type_resolve_builder_with_imported_bindings_and_cancellation<S>(
    hir: HirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding, S>,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TirBuilder, GraphcalError>
where
    S: std::hash::BuildHasher,
{
    let signed = resolve_hir_signature_with_modules_and_cancellation(
        hir,
        src,
        module_resolver,
        &project_types,
        cancellation,
    )?;
    type_resolve_signed_builder_with_imported_bindings_and_cancellation(
        signed,
        imported_bindings,
        src,
        module_resolver,
        project_types,
        cancellation,
    )
}

/// Resolve a signature-complete root HIR module's bodies and assemble TIR.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when an imported interface is inconsistent or
/// body semantic resolution fails.
pub fn type_resolve_signed_builder_with_imported_bindings_and_cancellation<S>(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding, S>,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TirBuilder, GraphcalError>
where
    S: std::hash::BuildHasher,
{
    cancellation.checkpoint()?;
    validate_checked_imported_bindings(signed.hir(), &imported_bindings, src)?;
    let imported_bindings = imported_bindings.into_iter().collect();
    let dag_id = signed.dag_id().clone();
    let context_types = Arc::clone(&project_types);
    let ctx = ModuleTypeContext::new(&dag_id, module_resolver, &context_types);
    type_resolve_impl(
        signed,
        imported_bindings,
        src,
        ctx,
        project_types,
        cancellation,
    )
}

fn validate_checked_imported_bindings<S>(
    ir: &HirDag,
    checked: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding, S>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError>
where
    S: std::hash::BuildHasher,
{
    if ir.imported_bindings().len() != checked.len() {
        return Err(GraphcalError::internal_error(
            format!(
                "HIR declares {} imported bindings but checking supplied {}",
                ir.imported_bindings().len(),
                checked.len()
            ),
            src,
            DiagnosticAnchor::WholeFile,
        ));
    }
    for (lexical, hir_target) in ir.imported_bindings() {
        let Some(checked_binding) = checked.get(lexical) else {
            return Err(GraphcalError::internal_error(
                format!("checked interface for imported binding `{lexical}` is missing"),
                src,
                DiagnosticAnchor::WholeFile,
            ));
        };
        if checked_binding.target() != hir_target {
            return Err(GraphcalError::internal_error(
                format!(
                    "checked interface for `{lexical}` targets `{}` instead of HIR target `{}`",
                    checked_binding.target(),
                    hir_target
                ),
                src,
                DiagnosticAnchor::WholeFile,
            ));
        }
    }
    Ok(())
}

fn finalize_hir_dag(
    dag: &mut DagTIR,
    surface: &ExternalDeclSurface,
    module_ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(), GraphcalError> {
    cancellation.checkpoint()?;
    augment_runtime_deps_for_dynamic_units(dag);
    dag.populate_projectable_outputs(surface);
    cancellation.checkpoint()?;
    validate_public_generic_defaults(dag, surface, module_ctx, src)?;
    cancellation.checkpoint()?;
    check_hir_body_policies(dag, surface, module_ctx, src)
}

fn type_resolve_impl(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
    project_types: Arc<ProjectTypeStore>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TirBuilder, GraphcalError> {
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
    Ok(TirBuilder::new(
        crate::registry::types::FormattingRegistry::new(
            project_types.base_dimensions().clone(),
            ir.display_dimensions,
        ),
        project_types,
        root_dag,
        ir.extern_functions,
    ))
}

/// Resolve type annotations for one DAG body with module-aware type-system
/// path lookup.
pub fn type_resolve_single_with_modules(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
) -> Result<DagTIR, GraphcalError> {
    type_resolve_single_with_modules_and_cancellation(
        hir,
        src,
        module_resolver,
        project_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Resolve one HIR DAG while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid types or cancellation.
pub fn type_resolve_single_with_modules_and_cancellation(
    hir: HirDag,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError> {
    type_resolve_single_with_imported_bindings_and_cancellation(
        hir,
        HashMap::new(),
        src,
        module_resolver,
        project_types,
        cancellation,
    )
}

/// Resolve one HIR DAG after attaching checked imported interfaces.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when an imported lexical target is missing or
/// mismatched, or when type resolution otherwise fails.
pub fn type_resolve_single_with_imported_bindings_and_cancellation<S>(
    hir: HirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding, S>,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError>
where
    S: std::hash::BuildHasher,
{
    let signed = resolve_hir_signature_with_modules_and_cancellation(
        hir,
        src,
        module_resolver,
        project_types,
        cancellation,
    )?;
    type_resolve_signed_single_with_imported_bindings_and_cancellation(
        signed,
        imported_bindings,
        src,
        module_resolver,
        project_types,
        cancellation,
    )
}

/// Resolve a signature-complete non-root HIR module's bodies.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when an imported interface is inconsistent or
/// body semantic resolution fails.
pub fn type_resolve_signed_single_with_imported_bindings_and_cancellation<S>(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding, S>,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    project_types: &ProjectTypeStore,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError>
where
    S: std::hash::BuildHasher,
{
    cancellation.checkpoint()?;
    validate_checked_imported_bindings(signed.hir(), &imported_bindings, src)?;
    let imported_bindings = imported_bindings.into_iter().collect();
    let dag_id = signed.dag_id().clone();
    let ctx = ModuleTypeContext::new(&dag_id, module_resolver, project_types);
    type_resolve_single_impl(signed, imported_bindings, src, ctx, cancellation)
}

fn type_resolve_single_impl(
    signed: SignatureResolvedHirDag,
    imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError> {
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
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<ResolvedDeclName, CheckedDeclType>, GraphcalError> {
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
    declarations: impl Iterator<Item = (ResolvedDeclName, &'d hir::DeclType)>,
    semantic_instances: &[crate::ir::instance::HirInstanceRecord],
    types: &ProjectTypeStore,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<ResolvedDeclName, CheckedDeclType>, GraphcalError> {
    let projection_substitutions = semantic_instances
        .iter()
        .flat_map(|record| {
            record
                .output_projections
                .iter()
                .filter_map(|projection| projection.exposed_name.as_bare())
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
        .collect()
}

/// The type view a template annotation is resolved in for an include with
/// `substitution`: the defaulted dimension ports it binds stay rigid, so the
/// substitution reaches every dimension defined over them.
fn instance_type_view<'s>(
    types: &'s ProjectTypeStore,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    src: &NamedSource<Arc<String>>,
) -> Result<std::borrow::Cow<'s, ProjectTypeStore>, GraphcalError> {
    let ports = types.bound_defaulted_dimension_ports(substitution);
    if ports.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(types));
    }
    types
        .with_rigid_dimensions(&ports)
        .map(std::borrow::Cow::Owned)
        .map_err(|_| GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: Span::new(0, src.inner().len()).into(),
        })
}

/// Resolve a template annotation in `view` (from [`instance_type_view`]) and
/// specialize it through the include's `substitution`.
fn resolve_instance_decl_type(
    decl_type: &hir::DeclType,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    view: &ProjectTypeStore,
    types: &ProjectTypeStore,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDeclType, GraphcalError> {
    let template_type = type_expr::resolve_hir_decl_type_with_project_types(decl_type, src, view)?;
    specialization::specialize_type(&template_type, substitution, types, src)
}

/// Declaration domain bounds keyed by the canonical declaration they bound.
type DomainBounds = HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>;

/// Attach each value declaration's checked type to its record, moving its
/// domain bounds into a table keyed by canonical identity.
fn attach_checked_types(
    decls: crate::ir::decl_table::DeclTable<crate::ir::lower::Lowered>,
    mut decl_types: HashMap<ResolvedDeclName, CheckedDeclType>,
    src: &NamedSource<Arc<String>>,
) -> Result<(crate::ir::decl_table::DeclTable<Typed>, DomainBounds), GraphcalError> {
    use crate::ir::entry::{
        AssertEntry, ConstEntry, Decl, FigureEntry, LayerEntry, NodeEntry, ParamEntry, PlotEntry,
    };
    let mut domain_bounds = HashMap::new();
    let mut check = |identity: ResolvedDeclName, annotation: hir::TypeAnnotation| {
        let checked = decl_types.remove(&identity).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("value declaration `{identity}` has no resolved signature"),
                src,
                DiagnosticAnchor::Source(annotation.span),
            )
        })?;
        if !annotation.domain_bounds.is_empty() {
            let bounds = annotation
                .domain_bounds
                .into_iter()
                .map(|bound| ResolvedDomainBound {
                    kind: bound.kind,
                    value: bound.value,
                    span: bound.span,
                    src: src.clone(),
                })
                .collect();
            domain_bounds.insert(identity, bounds);
        }
        Ok::<_, GraphcalError>(CheckedTypeAnnotation {
            decl_type: annotation.decl_type,
            span: annotation.span,
            checked,
        })
    };
    let decls = decls.try_map(
        |_| (),
        |decl| {
            let identity = decl.identity();
            Ok::<_, GraphcalError>(match decl {
                Decl::Const(entry) => Decl::Const(ConstEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    expr: entry.expr,
                    span: entry.span,
                }),
                Decl::Param(entry) => Decl::Param(ParamEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    default: entry.default,
                    span: entry.span,
                    override_reconciliations: entry.override_reconciliations,
                }),
                Decl::Node(entry) => Decl::Node(NodeEntry {
                    type_ann: check(identity, entry.type_ann)?,
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    definition: entry.definition,
                    span: entry.span,
                }),
                Decl::Assert(entry) => Decl::Assert(AssertEntry {
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    body: entry.body,
                    span: entry.span,
                }),
                Decl::Plot(entry) => Decl::Plot(PlotEntry {
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    mark_type: entry.mark_type,
                    body: entry.body,
                    visibility: entry.visibility,
                }),
                Decl::Figure(entry) => Decl::Figure(FigureEntry {
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
                    plot_names: entry.plot_names,
                    fields: entry.fields,
                }),
                Decl::Layer(entry) => Decl::Layer(LayerEntry {
                    name: entry.name,
                    declaration_owner: entry.declaration_owner,
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
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    module_ctx: ModuleTypeContext<'_>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    domain_bounds: DomainBounds,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIRSeed, GraphcalError> {
    cancellation.checkpoint()?;
    let dependencies = collect_resolved_dag_dependencies(&decls, module_ctx, src)?;
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
    let bindable_nominals = collect_bindable_nominals(module_ctx, src)?;

    let semantic = DagSemanticBody {
        domain_bounds,
        dynamic_unit_scales: HashMap::new(),
        dependencies,
        override_reconciliations,
        bindable_nominals,
        type_defs,
        decl_bindings: HashMap::new(),
        expression_facts: None,
        presentation: crate::tir::presentation::DagPresentationFacts::default(),
    };

    Ok(DagTIRSeed {
        dag_id: dag_id.clone(),
        decls,
        semantic,
    })
}

fn collect_bindable_nominals(
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<BindableNominalIdentity>, GraphcalError> {
    let symbols = ctx.resolver.symbols(ctx.owner).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("module symbol table missing for DAG `{}`", ctx.owner),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    Ok(symbols
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
        .collect())
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
) -> Result<ResolvedTypeDefs, GraphcalError> {
    let mut defs = ResolvedTypeDefs::default();
    if let Some(symbols) = ctx.resolver.symbols(ctx.owner) {
        for symbol in symbols.struct_types().values() {
            record_resolved_struct_type_def(symbol.resolved(), ctx, &mut defs)?;
        }
    }
    for annotation in annotations {
        collect_struct_type_defs_from_resolved_type(
            annotation.checked().resolved().element(),
            ctx,
            &mut defs,
        )?;
    }
    for binding in imported_bindings.values() {
        collect_struct_type_defs_from_declared_type(binding.declared_type(), ctx, &mut defs)?;
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

    const fn kind(&self) -> crate::registry::resolve_types::DeclarationKind {
        match self {
            Self::Dimension(_) => crate::registry::resolve_types::DeclarationKind::Dimension,
            Self::Index(_) => crate::registry::resolve_types::DeclarationKind::Index,
            Self::Type(_) => crate::registry::resolve_types::DeclarationKind::Type,
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
    arg: &hir::GenericArg,
    defs: &ResolvedTypeDefs,
    visited_defaults: &mut std::collections::HashSet<GenericParamId>,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    match arg {
        hir::GenericArg::Dim(arg) => {
            collect_public_signature_dim_arg_dependencies(arg, dependencies);
        }
        hir::GenericArg::Index(index) => {
            collect_public_signature_index_dependencies(index, dependencies);
        }
        hir::GenericArg::Nat(_) => {}
        hir::GenericArg::Type(type_expr) => collect_public_signature_type_dependencies(
            type_expr,
            defs,
            visited_defaults,
            dependencies,
        ),
    }
}

fn collect_public_signature_dim_arg_dependencies(
    arg: &hir::DimArg,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    let hir::DimArg::Expr(expr) = arg else {
        return;
    };
    dependencies.extend(
        expr.terms
            .iter()
            .filter_map(|item| match &item.term.target {
                hir::DimTermTarget::Dimension(name) => {
                    Some(PublicSignatureDependency::Dimension(name.clone()))
                }
                hir::DimTermTarget::GenericParam(_) => None,
            }),
    );
}

fn collect_public_signature_index_dependencies(
    index: &hir::IndexRef,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    if let hir::IndexRef::Concrete(name) = index {
        dependencies.push(PublicSignatureDependency::Index(name.clone()));
    }
}

fn collect_public_signature_type_dependencies(
    value_type: &hir::ValueType,
    defs: &ResolvedTypeDefs,
    visited_defaults: &mut std::collections::HashSet<GenericParamId>,
    dependencies: &mut Vec<PublicSignatureDependency>,
) {
    match &value_type.kind {
        hir::ValueTypeKind::Builtin(_) | hir::ValueTypeKind::GenericTypeParam(_) => {}
        hir::ValueTypeKind::DimExpr(expr) => {
            dependencies.extend(
                expr.terms
                    .iter()
                    .filter_map(|item| match &item.term.target {
                        hir::DimTermTarget::Dimension(name) => {
                            Some(PublicSignatureDependency::Dimension(name.clone()))
                        }
                        hir::DimTermTarget::GenericParam(_) => None,
                    }),
            );
        }
        hir::ValueTypeKind::Key(index) => {
            collect_public_signature_index_dependencies(index, dependencies);
        }
        hir::ValueTypeKind::Struct(name) => {
            dependencies.push(PublicSignatureDependency::Type(name.clone()));
        }
        hir::ValueTypeKind::Complex(arg) => {
            collect_public_signature_dim_arg_dependencies(arg, dependencies);
        }
        hir::ValueTypeKind::TypeApplication { name, generic_args } => {
            dependencies.push(PublicSignatureDependency::Type(name.clone()));
            for arg in generic_args {
                collect_public_signature_generic_arg_dependencies(
                    arg,
                    defs,
                    visited_defaults,
                    dependencies,
                );
            }
            if let Some(type_def) = defs.struct_types.get(&name.value) {
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
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (type_name, type_def) in &dag.semantic.type_defs.struct_types {
        if type_name.owner() != ctx.owner
            || !external_surface.is_static_explicit_export(type_name.atom())
        {
            continue;
        }
        let pub_span = ctx
            .resolver
            .symbol(type_name)
            .map(SymbolRef::span)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("module resolver lost source span for public type `{type_name}`"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
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
                if dependency.owner() == &crate::registry::prelude::prelude_dag_id() {
                    continue;
                }
                match dependency.is_public(ctx.resolver) {
                    Some(true) => {}
                    Some(false) => {
                        return Err(GraphcalError::PrivateInPublic {
                            pub_kind: crate::registry::resolve_types::DeclarationKind::Type,
                            pub_name: type_name.atom().clone(),
                            ref_kind: dependency.kind(),
                            ref_name: dependency.name(),
                            src: src.clone(),
                            ref_span: dependency.span().into(),
                            pub_span: pub_span.into(),
                        });
                    }
                    None => {
                        return Err(GraphcalError::InternalError {
                            message: format!(
                                "canonical {} `{}` is missing visibility metadata",
                                dependency.kind(),
                                dependency.name()
                            ),
                            src: src.clone(),
                            span: dependency.span().into(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn collect_struct_type_defs_from_declared_type(
    declared: &crate::registry::checked_type::CheckedType,
    ctx: ModuleTypeContext<'_>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    match declared {
        crate::registry::checked_type::CheckedType::Struct(name, generic_args) => {
            record_resolved_struct_type_def(name.resolved(), ctx, defs)?;
            for arg in generic_args {
                if let crate::registry::checked_type::CheckedGenericArg::Type(type_expr) = arg {
                    collect_struct_type_defs_from_declared_type(type_expr, ctx, defs)?;
                }
            }
        }
        crate::registry::checked_type::CheckedType::Indexed { element, .. } => {
            collect_struct_type_defs_from_declared_type(element, ctx, defs)?;
        }
        crate::registry::checked_type::CheckedType::Quantity(_)
        | crate::registry::checked_type::CheckedType::Complex(_)
        | crate::registry::checked_type::CheckedType::Bool
        | crate::registry::checked_type::CheckedType::Int
        | crate::registry::checked_type::CheckedType::Datetime(_)
        | crate::registry::checked_type::CheckedType::Key(_) => {}
    }
    Ok(())
}

fn collect_struct_type_defs_from_resolved_type(
    resolved: &ResolvedValueType,
    ctx: ModuleTypeContext<'_>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    match resolved {
        ResolvedValueType::Struct {
            name, generic_args, ..
        } => {
            record_resolved_struct_type_def(name, ctx, defs)?;
            for arg in generic_args {
                if let crate::tir::typed::ResolvedGenericArg::Type(type_expr) = arg {
                    collect_struct_type_defs_from_resolved_type(type_expr, ctx, defs)?;
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

fn record_resolved_struct_type_def(
    name: &ResolvedStructTypeName,
    ctx: ModuleTypeContext<'_>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    if defs.struct_types.contains_key(name) {
        return Ok(());
    }
    let Some(type_def) = ctx.types.get_struct_type_handle(name).cloned() else {
        return Ok(());
    };
    let definition_src = type_def.source();

    // Record the owner before walking defaults and fields so recursive nominal
    // types terminate naturally. Model-schema expansion needs this closure,
    // not only the types named directly by entry declarations: a prepared
    // boundary value can contain nested records whose names never appear in
    // the consumer module.
    defs.struct_types
        .insert(name.clone(), Arc::clone(&type_def));

    for param in type_def.generic_params() {
        if let Some(default) = param.default() {
            let resolved = resolve_hir_generic_arg(param, default, definition_src, ctx)?;
            if let ResolvedGenericArg::Type(type_expr) = &resolved {
                collect_struct_type_defs_from_resolved_type(type_expr, ctx, defs)?;
            }
            defs.generic_defaults
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
    if let Some(members) = type_def.union_members() {
        for member in members {
            for field in member.fields() {
                let key = ResolvedStructFieldTypeKey {
                    owning_type: name.clone(),
                    constructor: member.name(),
                    field: field.name().clone(),
                };
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
                        src: definition_src.clone(),
                    })
                    .collect();
                collect_struct_type_defs_from_resolved_type(resolved.element(), ctx, defs)?;
                defs.insert_field(key, ResolvedStructFieldSemantics::new(resolved, bounds));
            }
        }
    }

    Ok(())
}

/// HIR-level body policies that replaced the retired syntax-AST scope checks.
///
/// Walks every lowered body of one DAG and enforces:
/// - const bodies and every domain bound must not `@`-reference runtime
///   declarations (E020-style [`GraphcalError::GraphRefInConst`]) or use runtime
///   units in literals / conversion targets;
/// - no body may `@`-reference an assert declaration
///   ([`GraphcalError::GraphRefToAssert`]);
/// - A10(c) / V004: bodies of non-bindable kinds owned by this module must
///   not mention variant literals of the module's own `pub(bind)` indexes
///   ([`GraphcalError::PubIndexVariantLiteral`]). Params are exempt (A10(a));
///   sink kinds (assert/plot/figure/layer) are checked only when `pub`.
fn check_hir_body_policies(
    dag: &DagTIR,
    external_surface: &ExternalDeclSurface,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let semantic = &dag.semantic;
    let local = |key: &ResolvedDeclName| key.owner() == ctx.owner;

    for entry in dag.consts() {
        let key = entry.identity();
        HirPolicyChecker { ctx, src }.check_expr(
            &entry.expr,
            BodyPhase::CompileTime,
            local(&key),
        )?;
    }
    check_domain_bound_policies(semantic, ctx)?;
    check_dynamic_unit_policies(semantic, ctx)?;
    for entry in dag.nodes() {
        let key = entry.identity();
        entry.definition.formula().map_or(Ok(()), |expression| {
            HirPolicyChecker { ctx, src }.check_expr(expression, BodyPhase::Runtime, local(&key))
        })?;
    }
    for entry in dag.params() {
        let Some(default) = &entry.default else {
            continue;
        };
        // Params are exempt from A10 (a rebinding importer is forced to
        // rebind the param too — V005 at the include site).
        HirPolicyChecker { ctx, src }.check_expr(default, BodyPhase::Runtime, false)?;
    }
    check_sink_body_policies(dag, external_surface, ctx, src)
}

fn check_domain_bound_policies(
    semantic: &DagSemanticBody,
    ctx: ModuleTypeContext<'_>,
) -> Result<(), GraphcalError> {
    let check_bounds = |bounds: &[ResolvedDomainBound],
                        check_pub_bind_literals: bool|
     -> Result<(), GraphcalError> {
        for bound in bounds {
            HirPolicyChecker {
                ctx,
                src: &bound.src,
            }
            .check_expr(
                &bound.value,
                BodyPhase::CompileTime,
                check_pub_bind_literals,
            )?;
            // Domain bounds are evaluated without a host function registry.
            if let Some((external, span)) = hir::find_extern_call(&bound.value) {
                return Err(GraphcalError::ExternCallNotAllowed {
                    name: external.to_string(),
                    context: "domain bound".to_string(),
                    src: bound.src.clone(),
                    span: span.into(),
                });
            }
        }
        Ok(())
    };
    for (key, bounds) in &semantic.domain_bounds {
        check_bounds(bounds, key.owner() == ctx.owner)?;
    }
    for (_, field) in semantic.type_defs.constrained_fields() {
        check_bounds(field.domain_bounds(), false)?;
    }
    Ok(())
}

fn check_dynamic_unit_policies(
    semantic: &DagSemanticBody,
    ctx: ModuleTypeContext<'_>,
) -> Result<(), GraphcalError> {
    // Dynamic unit scales may read runtime params/nodes, but otherwise obey
    // ordinary runtime-body policy (notably, assertions cannot be read).
    // They also resolve in contexts with no host-function registry.
    for entry in semantic.dynamic_unit_scales.values() {
        HirPolicyChecker {
            ctx,
            src: &entry.src,
        }
        .check_expr(&entry.expr, BodyPhase::Runtime, false)?;
        if let Some((external, span)) = hir::find_extern_call(&entry.expr) {
            return Err(GraphcalError::ExternCallNotAllowed {
                name: external.to_string(),
                context: "unit scale expression".to_string(),
                src: entry.src.clone(),
                span: span.into(),
            });
        }
    }
    Ok(())
}

fn check_sink_body_policies(
    dag: &DagTIR,
    external_surface: &ExternalDeclSurface,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let is_explicit_export = |name: &DeclName| external_surface.is_explicit_export(name);
    for entry in dag.asserts() {
        let check_literals =
            entry.declaration_owner == *ctx.owner && is_explicit_export(&entry.name);
        let checker = HirPolicyChecker { ctx, src };
        match &*entry.body {
            hir::AssertBody::Expr(expr) => {
                checker.check_expr(expr, BodyPhase::Runtime, check_literals)?;
            }
            hir::AssertBody::Tolerance {
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
        let check_literals = is_explicit_export(&entry.name);
        let checker = HirPolicyChecker { ctx, src };
        for (_, expr) in &body.encodings {
            checker.check_expr(expr, BodyPhase::Runtime, check_literals)?;
        }
        for field in body.mark_properties.iter().chain(&body.properties) {
            checker.check_expr(&field.value, BodyPhase::Runtime, check_literals)?;
        }
    }
    for (name, fields) in dag
        .figures()
        .map(|entry| (&entry.name, &entry.fields))
        .chain(dag.layers().map(|entry| (&entry.name, &entry.fields)))
    {
        let check_literals = is_explicit_export(name);
        let checker = HirPolicyChecker { ctx, src };
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
    src: &'a NamedSource<Arc<String>>,
}

impl HirPolicyChecker<'_> {
    fn check_expr(
        &self,
        expr: &hir::Expr,
        phase: BodyPhase,
        check_pub_bind_literals: bool,
    ) -> Result<(), GraphcalError> {
        // Recursion choke point: recurses once per tree level.
        crate::stack::with_stack_growth(|| {
            self.check_expr_inner(expr, phase, check_pub_bind_literals)
        })
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind policy walk")]
    fn check_expr_inner(
        &self,
        expr: &hir::Expr,
        phase: BodyPhase,
        check_pub_bind_literals: bool,
    ) -> Result<(), GraphcalError> {
        let recurse = |inner: &hir::Expr| self.check_expr(inner, phase, check_pub_bind_literals);
        match expr.kind() {
            hir::ExprKind::Error(no_error) => no_error.absurd(),
            hir::ExprKind::Number(_)
            | hir::ExprKind::Integer(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::StringLiteral(_)
            | hir::ExprKind::OffsetDateTimeLiteral(_)
            | hir::ExprKind::CivilDateTimeLiteral(_)
            | hir::ExprKind::ZonedDateTimeLiteral(_)
            | hir::ExprKind::IanaTimeZoneLiteral(_)
            | hir::ExprKind::TypeSystemRef(_)
            | hir::ExprKind::ConstRef(_)
            | hir::ExprKind::LocalRef(_) => Ok(()),
            hir::ExprKind::QuantityLiteral { unit, .. } => self.check_unit_expr(unit, phase),
            hir::ExprKind::GraphRef(target) => {
                // Use the whole `@name` span (the reference Spanned covers
                // only the name) so the label includes the sigil.
                self.check_graph_ref(target, expr.span, phase)
            }
            hir::ExprKind::VariantLiteral(variant) => {
                self.check_variant_literal(variant, check_pub_bind_literals)
            }
            hir::ExprKind::BinOp { lhs, rhs, .. } => {
                recurse(lhs)?;
                recurse(rhs)
            }
            hir::ExprKind::UnaryOp { operand, .. }
            | hir::ExprKind::DisplayTimezone { expr: operand, .. }
            | hir::ExprKind::FieldAccess { expr: operand, .. } => recurse(operand),
            hir::ExprKind::Convert {
                expr: operand,
                target,
            } => {
                self.check_unit_expr(target, phase)?;
                recurse(operand)
            }
            hir::ExprKind::FnCall { callee, args, .. } => {
                // Extern functions are runtime-provided; const expressions
                // evaluate at compile time without a host function registry.
                if phase.is_compile_time()
                    && let hir::FunctionRef::External(ext) = &callee.value
                {
                    return Err(GraphcalError::ExternCallNotAllowed {
                        name: ext.to_string(),
                        context: "const expression".to_string(),
                        src: self.src.clone(),
                        span: callee.span.into(),
                    });
                }
                args.iter().try_for_each(recurse)
            }
            hir::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                recurse(condition)?;
                recurse(then_branch)?;
                recurse(else_branch)
            }
            hir::ExprKind::ConstructorCall { fields, .. } => {
                fields.iter().try_for_each(|field| recurse(&field.value))
            }
            hir::ExprKind::MapLiteral { entries } => {
                for entry in entries {
                    for key in &entry.keys {
                        if let hir::expr::MapEntryKey::IndexVariant(variant) = key {
                            self.check_variant_literal(variant, check_pub_bind_literals)?;
                        }
                    }
                    recurse(&entry.value)?;
                }
                Ok(())
            }
            hir::ExprKind::ForComp { body, .. } => recurse(body),
            hir::ExprKind::IndexAccess { expr: inner, args } => {
                recurse(inner)?;
                for arg in args {
                    match arg {
                        hir::expr::IndexArg::Variant(variant) => {
                            self.check_variant_literal(variant, check_pub_bind_literals)?;
                        }
                        hir::expr::IndexArg::Expr(arg_expr) => recurse(arg_expr)?,
                        hir::expr::IndexArg::Var(_) => {}
                    }
                }
                Ok(())
            }
            hir::ExprKind::Scan {
                source, init, body, ..
            } => {
                recurse(source)?;
                recurse(init)?;
                recurse(body)
            }
            hir::ExprKind::Unfold { init, body, .. } => {
                recurse(init)?;
                recurse(body)
            }
            hir::ExprKind::KeyForm { arg, .. } => recurse(arg),
            hir::ExprKind::Match { scrutinee, arms } => {
                recurse(scrutinee)?;
                for arm in arms {
                    if let hir::expr::MatchPattern::IndexLabel { variant, .. } = &arm.pattern {
                        self.check_variant_literal(variant, check_pub_bind_literals)?;
                    }
                    recurse(&arm.body)?;
                }
                Ok(())
            }
            hir::ExprKind::DagCall { target, args, .. } => {
                if phase.is_compile_time() {
                    return Err(GraphcalError::DagCallInCompileTime {
                        name: target.value.to_string(),
                        src: self.src.clone(),
                        span: expr.span.into(),
                    });
                }
                args.iter().try_for_each(|arg| recurse(&arg.value))
            }
        }
    }

    fn check_unit_expr(
        &self,
        unit: &hir::ResolvedUnitExpr,
        phase: BodyPhase,
    ) -> Result<(), GraphcalError> {
        if !phase.is_compile_time() {
            return Ok(());
        }
        for term in &unit.terms {
            let Some(info) = self.ctx.types.get_unit(term.name.value.resolved()) else {
                // Missing semantic unit definitions get their own diagnostics
                // from dimension checking.
                continue;
            };
            if !info.scale.constness().is_const() {
                return Err(GraphcalError::NonConstUnitInConst {
                    name: term.name.value.spelling().clone(),
                    src: self.src.clone(),
                    span: term.name.span.into(),
                });
            }
        }
        Ok(())
    }

    fn check_graph_ref(
        &self,
        target: &Spanned<ResolvedDeclName>,
        ref_span: Span,
        phase: BodyPhase,
    ) -> Result<(), GraphcalError> {
        let Some(kind) = self
            .ctx
            .resolver
            .symbol(&target.value)
            .map(|symbol| *symbol.kind())
        else {
            // Unknown targets get their own diagnostic from dependency
            // collection; the policy walk only classifies known ones.
            return Ok(());
        };
        if matches!(kind, crate::resolve::category::DeclSymbolKind::Assert) {
            return Err(GraphcalError::GraphRefToAssert {
                name: target.value.to_unowned_def_name(),
                src: self.src.clone(),
                span: ref_span.into(),
            });
        }
        if phase.is_compile_time() && !kind.is_const() {
            return Err(GraphcalError::GraphRefInConst {
                name: ScopedName::local(target.value.to_unowned_def_name()),
                src: self.src.clone(),
                span: ref_span.into(),
            });
        }
        Ok(())
    }

    fn check_variant_literal(
        &self,
        variant: &hir::expr::IndexVariantRef,
        check_pub_bind_literals: bool,
    ) -> Result<(), GraphcalError> {
        if !check_pub_bind_literals {
            return Ok(());
        }
        let index = variant.variant.index();
        if index.owner() != self.ctx.owner {
            return Ok(());
        }
        let is_pub_bind = self
            .ctx
            .resolver
            .symbols(self.ctx.owner)
            .and_then(|symbols| symbols.indexes().get(&index.to_unowned_def_name()))
            .is_some_and(|symbol| {
                // A bindable index with declared variants.
                symbol.visibility().is_bindable() && !symbol.data().is_empty()
            });
        if is_pub_bind {
            return Err(GraphcalError::PubIndexVariantLiteral {
                index: index.as_str().to_string(),
                variant: variant.variant.variant().as_str().to_string(),
                src: self.src.clone(),
                span: variant.path_span().into(),
            });
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
    included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
    static_ports: Vec<crate::hir::StaticPort>,
    assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    expected_fail: HashMap<ResolvedDeclName, crate::ir::lower::ResolvedExpectedFailMetadata>,
    dynamic_unit_scales: Vec<crate::ir::lower::DynamicUnitScaleEntry>,
    semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
}

/// Partially-built [`DagTIR`] returned by [`type_resolve_dag`]; finalized
/// by [`DagTIRSeed::with_body`] which fills in the rest of the per-DAG
/// fields.
struct DagTIRSeed {
    dag_id: crate::dag_id::DagId,
    decls: crate::ir::decl_table::DeclTable<Typed>,
    semantic: DagSemanticBody,
}

impl DagTIRSeed {
    fn with_body(
        self,
        body: HirBody,
        imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
        module_ctx: ModuleTypeContext<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<DagTIR, GraphcalError> {
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
                return Err(GraphcalError::internal_error(
                    format!("duplicate dynamic unit semantic entry `{unit}`"),
                    src,
                    DiagnosticAnchor::Source(span),
                ));
            }
        }

        let mut dag = DagTIR {
            dag_id: self.dag_id,
            body_revision: crate::body_revision::BodyRevision::fresh(),
            decls: self.decls,
            included_plots,
            semantic,
            static_ports,
            assumes_map,
            expected_fail,
            imported_bindings,
            semantic_instances,
            semantic_specialization: None,
            runtime_owner_rebases: HashMap::new(),
            projectable_outputs: std::collections::HashSet::new(),
        };
        // The complete owned-root inventory includes nominal bounds, even when
        // a constructor occurs nowhere in a declaration's ordinary value body.
        let mut constructed_types = HashSet::new();
        for root in dag.owned_expression_roots() {
            collect_constructed_types_from_expr(root, module_ctx, src, &mut constructed_types)?;
        }
        for owning_type in &constructed_types {
            record_resolved_struct_type_def(owning_type, module_ctx, &mut dag.semantic.type_defs)?;
        }
        Ok(dag)
    }
}

/// Build a temporary TIR view in which the optional dimension `ports` of
/// `dag_id` are rigid.
///
/// The ordinary checked TIR retains default-resolved signatures for parameter
/// default reconciliation. This view re-resolves source-authored declaration
/// signatures against opaque base identities (with every dimension defined
/// over a rigid port recomputed) so Option A can verify executable bodies,
/// and an include binding the ports can specialize them, without mutating the
/// authoritative result.
pub(crate) fn rigid_dimension_view(
    tir: &TIR,
    dag_id: &crate::dag_id::DagId,
    ports: &[ResolvedDimName],
    src: &NamedSource<Arc<String>>,
) -> Result<TIR, GraphcalError> {
    let rigid_types = tir
        .project_types
        .with_rigid_dimensions(ports)
        .map_err(|_| GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: Span::new(0, src.inner().len()).into(),
        })?;
    let mut rigid = tir.clone();
    for port in ports {
        rigid.registry.dimensions.register_rigid_dimension(port);
    }
    let rigid_dag = rigid.dags.localized_mut(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("template DAG `{dag_id}` is unavailable for rigid checking"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let resolved = resolve_declared_types(
        rigid_dag
            .value_decl_types()
            .map(|(identity, annotation)| (identity, &annotation.decl_type)),
        &rigid_dag.semantic_instances,
        &rigid_types,
        src,
        &crate::cancellation::CancellationToken::unbounded(),
    )?;
    rigid_dag.replace_value_decl_types(resolved);
    rigid.project_types = Arc::new(rigid_types);
    Ok(rigid)
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
pub(crate) mod specialization;
mod substitution;
mod type_expr;
pub use specialization::instantiate_semantic_edges;
pub(crate) use specialization::{
    install_semantic_plot_projection_facts, install_semantic_presentation_facts,
};
pub use substitution::{Substitution, SubstitutionError};
pub use type_expr::resolve_hir_decl_type;
use type_expr::{internal_error, module_resolve_error, resolve_hir_generic_arg};

#[cfg(test)]
mod tests;
