//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! The TIR layer resolves ambiguous syntax-level type paths (`NamePath` in
//! `DimTerm::name`, type applications, and `TypeExprKind::Indexed::indexes`) into
//! concrete dimensions, struct types, generic dimension parameters, or generic
//! index parameters.

use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::type_name::ResolvedStructTypeName;
use std::collections::HashMap;
use std::sync::Arc;

use crate::dimension::Dimension;
use crate::hir;
use crate::hir::diagnostics::{expr_lower_error_to_graphcal, resolved_decl_key};
pub use crate::ir::lower::{LoweredPlotBody, LoweredPlotField};
pub use crate::nat::NatPolyForm;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;
use miette::NamedSource;

use crate::ir::lower::IR;
use crate::ir::resolve::{DeclCategory, ParsedExpectedFail};
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::registry::types::{Registry, TypeDef, TypeGenericConstraint};
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_resolve::ModuleResolver;
use crate::syntax::names::NamePath;

mod model;
pub use model::*;

impl TIR {
    /// Complete constructor targets needed by independently lowered closed
    /// external values.
    ///
    /// Normal source expressions collect only the constructors they mention.
    /// A prepared evaluator can later receive a constructor value that was not
    /// present in the source text, so every constructor of every concrete type
    /// visible in the entry DAG must be available to checking and evaluation.
    pub fn prepare_external_value_constructors(&mut self) {
        let targets = self
            .root()
            .semantic
            .type_defs
            .struct_types
            .iter()
            .flat_map(|(owning_type, type_def)| {
                let members = match &type_def.kind {
                    crate::registry::type_def::TypeDefKind::Required => &[][..],
                    crate::registry::type_def::TypeDefKind::Union { members } => members.as_slice(),
                };
                members.iter().map(|variant| {
                    let constructor = crate::syntax::type_name::ResolvedConstructorName::new(
                        owning_type.owner().clone(),
                        variant.name.atom().clone(),
                    );
                    let target = ResolvedConstructorTarget {
                        owning_type: owning_type.clone(),
                        type_def: type_def.clone(),
                        variant: variant.clone(),
                    };
                    (constructor, target)
                })
            })
            .collect::<Vec<_>>();
        self.root_mut()
            .semantic
            .constructor_refs
            .constructor_defs
            .extend(targets);
    }
}

impl DagTIR {
    /// Build a concrete `DeclaredType` map from this DAG's resolved types
    /// plus its imported-value metadata. Adds builtin constants as
    /// `Dimensionless`.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any resolved type contains unresolved
    /// generic parameters.
    pub fn build_declared_types(
        &self,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HashMap<ScopedName, crate::registry::declared_type::DeclaredType>, GraphcalError>
    {
        // A DAG's own resolved declarations remain authoritative over imported
        // lexical bindings. The imported binding record keeps type/value/target
        // metadata atomic, so no collision can mix independently keyed facts.
        let mut declared_types = HashMap::new();
        for constant in crate::builtin::BuiltinConst::ALL {
            declared_types.insert(
                ScopedName::local(DeclName::expect_valid(constant.as_str())),
                crate::registry::declared_type::DeclaredType::Quantity(Dimension::dimensionless()),
            );
        }
        for (name, binding) in &self.imported_bindings {
            declared_types.insert(name.clone(), binding.declared_type().clone());
        }
        for (name, resolved) in &self.resolved_decl_types {
            let dt = resolved_to_declared_type(resolved, src)?;
            declared_types.insert(name.clone(), dt);
        }
        Ok(declared_types)
    }

    /// Populate the values that callers may project from this DAG.
    ///
    /// Explicitly exported nodes and annotation-free param input ports are both
    /// readable outputs. Projecting a param returns its effective value after
    /// applying the call binding or default.
    pub fn populate_projectable_outputs(
        &mut self,
        body: &[crate::desugar::desugared_ast::Declaration],
    ) {
        use crate::desugar::desugared_ast::DeclKind;

        for decl in body {
            match &decl.kind {
                DeclKind::Param(param) => {
                    self.projectable_outputs.insert(param.name.value.clone());
                }
                DeclKind::Node(node) if node.visibility.is_public() => {
                    self.projectable_outputs.insert(node.name.value.clone());
                }
                _ => {}
            }
        }
    }

    /// Return the resolved declaration key for a declaration visible from this DAG.
    ///
    /// Qualified source keys synthesize a child owner under this DAG so
    /// source-facing entries still use resolved identities instead of
    /// source-keyed runtime maps.
    #[must_use]
    pub fn resolved_decl_key_for_local(&self, name: &ScopedName) -> ResolvedDeclName {
        if let Some(resolved) = self.semantic.decl_bindings.get(name) {
            return resolved.clone();
        }
        if self.resolved_decl_types.contains_key(name)
            || self
                .source_order
                .iter()
                .any(|(source_name, _)| source_name == name)
        {
            return resolved_decl_key(&self.dag_id, name);
        }
        if !name.is_qualified() {
            let mut candidates = self
                .resolved_decl_types
                .keys()
                .filter(|candidate| candidate.member() == name.member())
                .map(|candidate| resolved_decl_key(&self.dag_id, candidate));
            if let Some(candidate) = candidates.next()
                && candidates.next().is_none()
            {
                return candidate;
            }
        }
        resolved_decl_key(&self.dag_id, name)
    }
}

/// Resolve all type annotations in an `IR` using module-aware type-system
/// resolution for syntactic paths.
///
/// Qualified source paths such as `lib.Length`, `lib.Vec3<...>`, and
/// `lib.Phase` are first lowered into HIR canonical references using
/// `module_resolver`; TIR then consumes those HIR references and reads the
/// corresponding definition from `module_types`. Runtime-facing values still
/// keep display leaves for diagnostics, but semantic lookup no longer depends on
/// source alias strings.
pub fn type_resolve_with_modules(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
) -> Result<TIR, GraphcalError> {
    type_resolve_with_modules_and_cancellation(
        ir,
        root_dag_id,
        src,
        module_resolver,
        module_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Resolve an IR to TIR while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid types or cancellation.
pub fn type_resolve_with_modules_and_cancellation(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TIR, GraphcalError> {
    cancellation.checkpoint()?;
    let owner_for_ctx = root_dag_id.clone();
    let ctx = ModuleTypeContext::new(&owner_for_ctx, module_resolver, module_types);
    type_resolve_impl(ir, root_dag_id, src, ctx, cancellation)
}

fn type_resolve_impl(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<TIR, GraphcalError> {
    cancellation.checkpoint()?;
    let imported_bindings_for_hir = ir.imported_bindings.clone();
    let asserts_for_hir = ir.asserts.clone();
    let mut root_dag = type_resolve_dag(
        ir.consts,
        ir.params,
        ir.nodes,
        &asserts_for_hir,
        &ir.registry,
        src,
        &root_dag_id,
        module_ctx,
        &imported_bindings_for_hir,
        cancellation,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.included_plots,
        ir.source_order,
        ir.assumes_map,
        ir.expected_fail,
        ir.dynamic_unit_scales,
        ir.imported_bindings,
        module_ctx,
        src,
    )?;
    cancellation.checkpoint()?;
    augment_runtime_deps_for_dynamic_units(&mut root_dag.semantic);
    cancellation.checkpoint()?;
    check_hir_body_policies(&root_dag, &ir.external_surface, module_ctx, src)?;
    let mut dags = DagRegistry::new();
    dags.insert(root_dag_id.clone(), root_dag);
    Ok(TIR {
        registry: ir.registry,
        module_types: module_ctx.types.clone(),
        root_dag_id,
        dags,
        module_aliases: HashMap::new(),
        extern_functions: ir.extern_functions,
    })
}

/// Resolve type annotations for one DAG body with module-aware type-system
/// path lookup.
pub fn type_resolve_single_with_modules(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
) -> Result<DagTIR, GraphcalError> {
    type_resolve_single_with_modules_and_cancellation(
        ir,
        dag_id,
        src,
        module_resolver,
        module_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Resolve one DAG IR while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid types or cancellation.
pub fn type_resolve_single_with_modules_and_cancellation(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError> {
    cancellation.checkpoint()?;
    let ctx = ModuleTypeContext::new(dag_id, module_resolver, module_types);
    type_resolve_single_impl(ir, dag_id, src, ctx, cancellation)
}

fn type_resolve_single_impl(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIR, GraphcalError> {
    cancellation.checkpoint()?;
    let imported_bindings_for_hir = ir.imported_bindings.clone();
    let asserts_for_hir = ir.asserts.clone();
    let mut dag = type_resolve_dag(
        ir.consts,
        ir.params,
        ir.nodes,
        &asserts_for_hir,
        &ir.registry,
        src,
        dag_id,
        module_ctx,
        &imported_bindings_for_hir,
        cancellation,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.included_plots,
        ir.source_order,
        ir.assumes_map,
        ir.expected_fail,
        ir.dynamic_unit_scales,
        ir.imported_bindings,
        module_ctx,
        src,
    )?;
    cancellation.checkpoint()?;
    augment_runtime_deps_for_dynamic_units(&mut dag.semantic);
    cancellation.checkpoint()?;
    check_hir_body_policies(&dag, &ir.external_surface, module_ctx, src)?;
    Ok(dag)
}

/// Internal helper: resolve type annotations for the const/param/node
/// declarations of a single DAG, returning a partially-built [`DagTIR`].
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "orchestrates per-DAG type resolution across IR declarations and semantic body data"
)]
fn type_resolve_dag(
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    asserts: &[crate::ir::lower::AssertEntry],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    module_ctx: ModuleTypeContext<'_>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<DagTIRSeed, GraphcalError> {
    cancellation.checkpoint()?;
    let mut resolved_decl_types = HashMap::new();
    let no_generic_params: &[GenericParamName] = &[];

    // A merged dependency declaration's type annotation keeps the dependency
    // file's offsets, so resolution errors must render against its own source
    // rather than the importer's `src` (#868).
    for entry in &consts {
        cancellation.checkpoint()?;
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.type_src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &params {
        cancellation.checkpoint()?;
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.type_src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &nodes {
        cancellation.checkpoint()?;
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.type_src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }

    cancellation.checkpoint()?;
    let LoweredDagExpressions {
        exprs: expressions,
        domain_bounds,
    } = lower_resolved_expressions(
        &consts,
        &params,
        &nodes,
        asserts,
        module_ctx,
        imported_bindings,
        registry,
        src,
    )?;
    cancellation.checkpoint()?;
    let dependencies =
        collect_resolved_dag_dependencies(&consts, &params, &nodes, &expressions, module_ctx, src)?;
    cancellation.checkpoint()?;
    let collection_refs = collect_resolved_collection_refs(
        &expressions,
        &domain_bounds,
        &resolved_decl_types,
        imported_bindings,
        module_ctx,
        src,
    )?;
    cancellation.checkpoint()?;
    let constructor_refs =
        collect_resolved_constructor_refs(&expressions, &domain_bounds, module_ctx, src)?;
    let override_reconciliations = resolve_override_reconciliations(&params, module_ctx)?;
    cancellation.checkpoint()?;
    let type_defs = collect_resolved_type_defs(
        &resolved_decl_types,
        &constructor_refs,
        imported_bindings,
        module_ctx,
        registry,
        src,
    )?;

    let semantic = DagSemanticBody {
        expressions,
        domain_bounds,
        plot_exprs: ResolvedPlotExprs::default(),
        dynamic_unit_scales: HashMap::new(),
        dependencies,
        collection_refs,
        constructor_refs,
        override_reconciliations,
        type_defs,
        decl_bindings: HashMap::new(),
    };

    Ok(DagTIRSeed {
        dag_id: dag_id.clone(),
        consts,
        params,
        nodes,
        resolved_decl_types,
        semantic,
    })
}

fn resolve_override_target(
    target: &crate::ir::override_reconciliation::PendingOverrideTarget,
    src: &NamedSource<Arc<String>>,
    include_span: Span,
    ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedOverrideTarget, GraphcalError> {
    use crate::ir::override_reconciliation::PendingOverrideTarget;
    use crate::registry::declared_type::IndexTypeRef;

    let resolve_error = |error| module_resolve_error(&error, src, include_span);
    match target {
        PendingOverrideTarget::Index {
            overridden,
            source_owner,
            replacement,
            replacement_owner,
        } => {
            let source = ctx
                .resolver
                .resolve_index_path(source_owner, &NamePath::local(overridden.atom().clone()))
                .map_err(resolve_error)?;
            let replacement = match replacement {
                crate::registry::types::IndexBindingTarget::Declared(name) => {
                    let resolved = ctx
                        .resolver
                        .resolve_index_path(
                            replacement_owner,
                            &NamePath::local(name.atom().clone()),
                        )
                        .map_err(resolve_error)?;
                    IndexTypeRef::from_resolved(resolved)
                }
                crate::registry::types::IndexBindingTarget::Finite(index) => {
                    IndexTypeRef::from_finite_index(*index)
                }
            };
            Ok(ResolvedOverrideTarget::Index {
                overridden: overridden.clone(),
                source,
                replacement,
            })
        }
        PendingOverrideTarget::Type {
            overridden,
            source_owner,
            replacement,
            replacement_owner,
        } => {
            let source = ctx
                .resolver
                .resolve_struct_type_path(source_owner, &NamePath::local(overridden.atom().clone()))
                .map_err(resolve_error)?;
            let replacement = ctx
                .resolver
                .resolve_struct_type_path(
                    replacement_owner,
                    &NamePath::local(replacement.atom().clone()),
                )
                .map_err(resolve_error)?;
            Ok(ResolvedOverrideTarget::Type {
                overridden: overridden.clone(),
                source,
                replacement,
            })
        }
    }
}

fn resolve_override_reconciliations(
    params: &[crate::ir::lower::ParamEntry],
    ctx: ModuleTypeContext<'_>,
) -> Result<HashMap<ResolvedDeclName, Vec<OverrideReconciliation>>, GraphcalError> {
    params
        .iter()
        .filter(|entry| !entry.override_reconciliations.is_empty())
        .map(|entry| {
            let reconciliations = entry
                .override_reconciliations
                .iter()
                .map(|pending| {
                    let targets = pending
                        .targets
                        .iter()
                        .map(|target| {
                            resolve_override_target(target, &pending.src, pending.include_span, ctx)
                        })
                        .collect::<Result<Vec<_>, GraphcalError>>()?;
                    Ok(OverrideReconciliation {
                        orphan_decl: pending.orphan_decl.clone(),
                        targets,
                        src: pending.src.clone(),
                        include_span: pending.include_span,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?;
            Ok((resolved_decl_key(ctx.owner, &entry.name), reconciliations))
        })
        .collect()
}

fn collect_resolved_type_defs(
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    constructor_refs: &ResolvedConstructorRefs,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeDefs, GraphcalError> {
    let mut defs = ResolvedTypeDefs::default();
    if let Some(symbols) = ctx.resolver.modules().get(ctx.owner) {
        for symbol in symbols.struct_types().values() {
            record_resolved_struct_type_def(symbol.resolved(), ctx, registry, src, &mut defs)?;
        }
    }
    for resolved in resolved_decl_types.values() {
        collect_struct_type_defs_from_resolved_type(resolved, ctx, registry, src, &mut defs)?;
    }
    for binding in imported_bindings.values() {
        collect_struct_type_defs_from_declared_type(
            binding.declared_type(),
            ctx,
            registry,
            src,
            &mut defs,
        )?;
    }
    for target in constructor_refs.constructor_defs.values() {
        record_resolved_struct_type_def(&target.owning_type, ctx, registry, src, &mut defs)?;
    }
    Ok(defs)
}

fn collect_struct_type_defs_from_declared_type(
    declared: &crate::registry::declared_type::DeclaredType,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    match declared {
        crate::registry::declared_type::DeclaredType::Struct(name, generic_args) => {
            record_resolved_struct_type_def(name.resolved(), ctx, registry, src, defs)?;
            for arg in generic_args {
                if let crate::registry::declared_type::DeclaredGenericArg::Type(type_expr) = arg {
                    collect_struct_type_defs_from_declared_type(
                        type_expr, ctx, registry, src, defs,
                    )?;
                }
            }
        }
        crate::registry::declared_type::DeclaredType::Indexed { element, .. } => {
            collect_struct_type_defs_from_declared_type(element, ctx, registry, src, defs)?;
        }
        crate::registry::declared_type::DeclaredType::Quantity(_)
        | crate::registry::declared_type::DeclaredType::Complex(_)
        | crate::registry::declared_type::DeclaredType::Bool
        | crate::registry::declared_type::DeclaredType::Int
        | crate::registry::declared_type::DeclaredType::Datetime(_)
        | crate::registry::declared_type::DeclaredType::IndexArg(_)
        | crate::registry::declared_type::DeclaredType::Key(_) => {}
    }
    Ok(())
}

fn collect_struct_type_defs_from_resolved_type(
    resolved: &ResolvedTypeExpr,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    match resolved {
        ResolvedTypeExpr::Struct(name, _) => {
            record_resolved_struct_type_def(name, ctx, registry, src, defs)?;
        }
        ResolvedTypeExpr::GenericStruct {
            name, generic_args, ..
        } => {
            record_resolved_struct_type_def(name, ctx, registry, src, defs)?;
            for arg in generic_args {
                if let crate::tir::typed::ResolvedGenericArg::Type(type_expr) = arg {
                    collect_struct_type_defs_from_resolved_type(
                        type_expr, ctx, registry, src, defs,
                    )?;
                }
            }
        }
        ResolvedTypeExpr::Indexed { base, indexes: _ } => {
            collect_struct_type_defs_from_resolved_type(base, ctx, registry, src, defs)?;
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Complex { .. }
        | ResolvedTypeExpr::Key { .. }
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::IndexArg(_)
        | ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => {}
    }
    Ok(())
}

fn record_resolved_struct_type_def(
    name: &ResolvedStructTypeName,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    defs: &mut ResolvedTypeDefs,
) -> Result<(), GraphcalError> {
    if defs.struct_types.contains_key(name) {
        return Ok(());
    }
    let Some(type_def) = ctx.types.get_struct_type(name) else {
        return Ok(());
    };
    let definition_src = ctx.types.source_for_owner(name.owner()).unwrap_or(src);

    // Record the owner before walking defaults and fields so recursive nominal
    // types terminate naturally. Model-schema expansion needs this closure,
    // not only the types named directly by entry declarations: a prepared
    // boundary value can contain nested records whose names never appear in
    // the consumer module.
    defs.struct_types.insert(name.clone(), type_def.clone());

    for param in &type_def.generic_params {
        if let Some(default) = &param.default {
            let resolved = resolve_generic_default_in_struct_scope(
                default,
                param,
                name,
                type_def,
                ctx,
                registry,
                definition_src,
            )?;
            if let ResolvedGenericArg::Type(type_expr) = &resolved {
                collect_struct_type_defs_from_resolved_type(
                    type_expr,
                    ctx,
                    registry,
                    definition_src,
                    defs,
                )?;
            }
            defs.generic_defaults
                .insert((name.clone(), param.name.clone()), resolved);
        }
    }

    if let Some(members) = type_def.union_members() {
        let generic_scope = generic_scope_for_type_def(name, type_def, definition_src)?;
        let prelude = hir::PreludeTypeScope::graphcal();
        let bound_expr_ctx = hir::ExprLoweringContext::new(
            name.owner(),
            ctx.resolver,
            &generic_scope,
            &registry.time_zones,
        )
        .with_prelude(&prelude)
        .with_unit_registry(&registry.units);
        for member in members {
            for field in &member.fields {
                let key = ResolvedStructFieldTypeKey {
                    owning_type: name.clone(),
                    constructor: member.name.clone(),
                    field: field.name.clone(),
                };
                let resolved = resolve_type_expr_in_struct_scope(
                    &field.type_ann,
                    name,
                    type_def,
                    ctx,
                    registry,
                    definition_src,
                )?;
                let bounds = lower_domain_bounds(&field.type_ann, bound_expr_ctx, definition_src)?;
                collect_struct_type_defs_from_resolved_type(
                    &resolved,
                    ctx,
                    registry,
                    definition_src,
                    defs,
                )?;
                if !bounds.is_empty() {
                    defs.field_bounds.insert(key.clone(), bounds);
                }
                defs.field_types.insert(key, resolved);
            }
        }
    }

    Ok(())
}

/// Build the lexical generic scope of a type definition so field-bound
/// expressions can lower references to the type's generic parameters.
fn generic_scope_for_type_def(
    name: &ResolvedStructTypeName,
    type_def: &TypeDef,
    src: &NamedSource<Arc<String>>,
) -> Result<hir::GenericScope, GraphcalError> {
    let owner = hir::GenericParamOwner::Type(name.clone());
    let mut scope = hir::GenericScope::new();
    for param in &type_def.generic_params {
        let constraint = match param.constraint {
            TypeGenericConstraint::Dim => crate::syntax::ast::GenericConstraint::Dim,
            TypeGenericConstraint::Index => crate::syntax::ast::GenericConstraint::Index,
            TypeGenericConstraint::Nat => crate::syntax::ast::GenericConstraint::Nat,
            TypeGenericConstraint::Type => crate::syntax::ast::GenericConstraint::Type,
        };
        scope
            .insert_binding(hir::GenericParamBinding::new(
                hir::GenericParamId::new(owner.clone(), param.name.clone()),
                constraint,
                Span::new(0, 0),
            ))
            .map_err(|err| GraphcalError::InternalError {
                message: format!("duplicate generic param while scoping `{name}`: {err}"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
    }
    Ok(scope)
}

/// Assemble the semantic expression maps from the IR's lowered bodies and
/// lower the declaration domain bounds.
///
/// Bodies were lowered to HIR at [`crate::ir::lower::UnfrozenIR::freeze`];
/// this step keys them by canonical declaration identity and lowers the
/// type-annotation domain-bound expressions, which live in declaration
/// signatures rather than bodies.
#[expect(
    clippy::too_many_arguments,
    reason = "expression lowering needs declaration slices, module identity, timezone registry, and diagnostics source"
)]
fn lower_resolved_expressions(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    asserts: &[crate::ir::lower::AssertEntry],
    ctx: ModuleTypeContext<'_>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<LoweredDagExpressions, GraphcalError> {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let decl_bindings =
        collect_hir_decl_bindings(ctx.owner, consts, params, nodes, imported_bindings);
    let lower_bounds_in = |type_ann: &crate::desugar::desugared_ast::TypeExpr,
                           resolution_owner: &crate::dag_id::DagId,
                           body_src: &NamedSource<Arc<String>>| {
        let expr_ctx = hir::ExprLoweringContext::new(
            resolution_owner,
            ctx.resolver,
            &generic_scope,
            &registry.time_zones,
        )
        .with_prelude(&prelude)
        .with_unit_registry(&registry.units)
        .with_decl_bindings(&decl_bindings);
        lower_domain_bounds(type_ann, expr_ctx, body_src)
    };
    let mut exprs = ResolvedExpressions::default();
    let mut domain_bounds = HashMap::new();

    // Domain-bound and key errors for a merged dependency body must render
    // against the dependency's own source, not the importer's `src` (#868).
    for entry in consts {
        let signature_src = entry.type_src.resolve(src);
        let key = resolved_decl_key(ctx.owner, &entry.name);
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, signature_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        exprs.consts.insert(key, entry.expr.clone());
    }
    for entry in params {
        let signature_src = entry.type_src.resolve(src);
        let key = resolved_decl_key(ctx.owner, &entry.name);
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, signature_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        let Some(expr) = &entry.default_expr else {
            continue;
        };
        exprs.param_defaults.insert(key, expr.clone());
    }
    for entry in nodes {
        let signature_src = entry.type_src.resolve(src);
        let key = resolved_decl_key(ctx.owner, &entry.name);
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, signature_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        exprs.nodes.insert(key, entry.expr.clone());
    }
    for entry in asserts {
        let key = resolved_decl_key(ctx.owner, &entry.name);
        exprs.asserts.insert(key, entry.body.clone());
    }

    Ok(LoweredDagExpressions {
        exprs,
        domain_bounds,
    })
}

/// Collect semantic index/constructor definitions reached only from dynamic
/// unit scale expressions.
fn collect_dynamic_unit_refs(
    ctx: ModuleTypeContext<'_>,
    semantic: &mut DagSemanticBody,
) -> Result<(), GraphcalError> {
    let DagSemanticBody {
        dynamic_unit_scales,
        collection_refs,
        constructor_refs,
        ..
    } = semantic;
    for entry in dynamic_unit_scales.values() {
        collect_resolved_collection_refs_from_expr(&entry.expr, ctx, &entry.src, collection_refs)?;
        collect_resolved_constructor_refs_from_expr(
            &entry.expr,
            ctx,
            &entry.src,
            constructor_refs,
        )?;
    }
    Ok(())
}

/// Populate the semantic body's plot expression maps from the IR's strictly
/// lowered plot/figure/layer entries and run the reference-collection walks
/// over them.
fn collect_plot_exprs(
    plots: &[crate::ir::lower::PlotEntry],
    figures: &[crate::ir::lower::FigureEntry],
    layers: &[crate::ir::lower::LayerEntry],
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    semantic: &mut DagSemanticBody,
) -> Result<(), GraphcalError> {
    let mut plot_exprs = ResolvedPlotExprs::default();

    let collect = |expr: &hir::Expr,
                   expr_src: &NamedSource<Arc<String>>,
                   collection_refs: &mut ResolvedCollectionRefs,
                   constructor_refs: &mut ResolvedConstructorRefs|
     -> Result<(), GraphcalError> {
        collect_resolved_collection_refs_from_expr(expr, ctx, expr_src, collection_refs)?;
        collect_resolved_constructor_refs_from_expr(expr, ctx, expr_src, constructor_refs)
    };

    for entry in plots {
        let body = &entry.body;
        let body_src = entry.body_src.resolve(src);
        for (_, expr) in &body.encodings {
            collect(
                expr,
                body_src,
                &mut semantic.collection_refs,
                &mut semantic.constructor_refs,
            )?;
        }
        for field in body.mark_properties.iter().chain(&body.properties) {
            collect(
                &field.value,
                body_src,
                &mut semantic.collection_refs,
                &mut semantic.constructor_refs,
            )?;
        }
        plot_exprs.plots.insert(entry.name.clone(), body.clone());
    }

    for (name, fields, body_src, is_figure) in figures
        .iter()
        .map(|entry| (&entry.name, &entry.fields, &entry.body_src, true))
        .chain(
            layers
                .iter()
                .map(|entry| (&entry.name, &entry.fields, &entry.body_src, false)),
        )
    {
        let body_src = body_src.resolve(src);
        for field in fields {
            collect(
                &field.value,
                body_src,
                &mut semantic.collection_refs,
                &mut semantic.constructor_refs,
            )?;
        }
        if is_figure {
            plot_exprs.figures.insert(name.clone(), fields.clone());
        } else {
            plot_exprs.layers.insert(name.clone(), fields.clone());
        }
    }

    semantic.plot_exprs = plot_exprs;
    Ok(())
}

/// Lower a declaration type annotation's domain bounds to HIR.
fn lower_domain_bounds(
    type_ann: &crate::desugar::desugared_ast::TypeExpr,
    expr_ctx: hir::ExprLoweringContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<ResolvedDomainBound>, GraphcalError> {
    type_ann
        .domain_bounds()
        .iter()
        .map(|bound| {
            let value = hir::lower_expr(&bound.value, expr_ctx)
                .map_err(|err| expr_lower_error_to_graphcal(&err, src))?;
            Ok(ResolvedDomainBound {
                kind: bound.kind,
                value,
                span: bound.span,
                src: src.clone(),
            })
        })
        .collect()
}

/// Output of [`lower_resolved_expressions`]: the lowered declaration bodies
/// plus the HIR domain bounds collected while lowering them.
struct LoweredDagExpressions {
    exprs: ResolvedExpressions,
    domain_bounds: HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
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

    for entry in &dag.consts {
        let key = dag.resolved_decl_key_for_local(&entry.name);
        let body_src = entry.body_src.resolve(src);
        let expr = semantic.expressions.consts.get(&key).ok_or_else(|| {
            internal_error(
                format!("semantic HIR expression missing for const `{}`", entry.name),
                body_src,
                entry.span,
            )
        })?;
        HirPolicyChecker { ctx, src: body_src }.check_expr(
            expr,
            BodyPhase::CompileTime,
            local(&key),
        )?;
    }
    check_domain_bound_policies(semantic, ctx)?;
    check_dynamic_unit_policies(semantic, ctx)?;
    for entry in &dag.nodes {
        let key = dag.resolved_decl_key_for_local(&entry.name);
        let body_src = entry.body_src.resolve(src);
        let expr = semantic.expressions.nodes.get(&key).ok_or_else(|| {
            internal_error(
                format!("semantic HIR expression missing for node `{}`", entry.name),
                body_src,
                entry.span,
            )
        })?;
        HirPolicyChecker { ctx, src: body_src }.check_expr(
            expr,
            BodyPhase::Runtime,
            local(&key),
        )?;
    }
    for entry in &dag.params {
        let key = dag.resolved_decl_key_for_local(&entry.name);
        let Some(expr) = semantic.expressions.param_defaults.get(&key) else {
            continue;
        };
        let default_src = entry.default_src.as_ref().ok_or_else(|| {
            internal_error(
                format!("parameter `{}` is missing default provenance", entry.name),
                src,
                entry.span,
            )
        })?;
        // Params are exempt from A10 (a rebinding importer is forced to
        // rebind the param too — V005 at the include site).
        HirPolicyChecker {
            ctx,
            src: default_src.resolve(src),
        }
        .check_expr(expr, BodyPhase::Runtime, false)?;
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
    for bounds in semantic.type_defs.field_bounds.values() {
        check_bounds(bounds, false)?;
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
    let is_explicit_export =
        |leaf: &str| external_surface.is_explicit_export(&DeclName::expect_valid(leaf));
    for entry in &dag.asserts {
        let key = dag.resolved_decl_key_for_local(&entry.name);
        let body_src = entry.body_src.resolve(src);
        let body = dag.semantic.expressions.asserts.get(&key).ok_or_else(|| {
            internal_error(
                format!("semantic HIR body missing for assert `{}`", entry.name),
                body_src,
                entry.span,
            )
        })?;
        let check_literals = key.owner() == ctx.owner && is_explicit_export(key.as_str());
        let checker = HirPolicyChecker { ctx, src: body_src };
        match body {
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
    for entry in &dag.plots {
        let body = &entry.body;
        let check_literals =
            !entry.name.is_qualified() && is_explicit_export(entry.name.member().as_str());
        let checker = HirPolicyChecker {
            ctx,
            src: entry.body_src.resolve(src),
        };
        for (_, expr) in &body.encodings {
            checker.check_expr(expr, BodyPhase::Runtime, check_literals)?;
        }
        for field in body.mark_properties.iter().chain(&body.properties) {
            checker.check_expr(&field.value, BodyPhase::Runtime, check_literals)?;
        }
    }
    for (name, fields, body_src) in dag
        .figures
        .iter()
        .map(|entry| (&entry.name, &entry.fields, &entry.body_src))
        .chain(
            dag.layers
                .iter()
                .map(|entry| (&entry.name, &entry.fields, &entry.body_src)),
        )
    {
        let check_literals = !name.is_qualified() && is_explicit_export(name.member().as_str());
        let checker = HirPolicyChecker {
            ctx,
            src: body_src.resolve(src),
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
        match &expr.kind {
            hir::ExprKind::Error { children } => children.iter().try_for_each(recurse),
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
            hir::ExprKind::UnitLiteral { unit, .. } => self.check_unit_expr(unit, phase),
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
            if !info.constness.is_const() {
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
        let Ok(kind) = self.ctx.resolver.decl_symbol_kind(&target.value) else {
            // Unknown targets get their own diagnostic from dependency
            // collection; the policy walk only classifies known ones.
            return Ok(());
        };
        if matches!(kind, crate::syntax::module_resolve::DeclSymbolKind::Assert) {
            return Err(GraphcalError::GraphRefToAssert {
                name: DeclName::expect_valid(target.value.as_str()),
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
            .modules()
            .get(self.ctx.owner)
            .and_then(|symbols| {
                symbols
                    .indexes()
                    .get(&IndexName::expect_valid(index.as_str()))
            })
            .is_some_and(|symbol| {
                symbol.visibility().is_bindable() && !symbol.variants().is_empty()
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
    augment_runtime_deps_for_dynamic_units, collect_hir_decl_bindings,
    collect_resolved_collection_refs, collect_resolved_collection_refs_from_expr,
    collect_resolved_constructor_refs, collect_resolved_constructor_refs_from_expr,
    collect_resolved_dag_dependencies, collect_resolved_decl_bindings, resolve_expected_fail_keys,
};

/// Partially-built [`DagTIR`] returned by [`type_resolve_dag`]; finalized
/// by [`DagTIRSeed::with_body`] which fills in the rest of the per-DAG
/// fields.
struct DagTIRSeed {
    dag_id: crate::dag_id::DagId,
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
    semantic: DagSemanticBody,
}

impl DagTIRSeed {
    #[expect(
        clippy::too_many_arguments,
        reason = "single conversion that absorbs every IR field beyond the resolved decls"
    )]
    fn with_body(
        self,
        asserts: Vec<crate::ir::lower::AssertEntry>,
        plots: Vec<crate::ir::lower::PlotEntry>,
        figures: Vec<crate::ir::lower::FigureEntry>,
        layers: Vec<crate::ir::lower::LayerEntry>,
        included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
        source_order: Vec<(ScopedName, DeclCategory)>,
        assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
        expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
        dynamic_unit_scales: Vec<crate::ir::lower::DynamicUnitScaleEntry>,
        imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
        module_ctx: ModuleTypeContext<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<DagTIR, GraphcalError> {
        let decl_bindings = collect_resolved_decl_bindings(
            module_ctx.owner,
            &self.consts,
            &self.params,
            &self.nodes,
            &imported_bindings,
        );
        let expected_fail = resolve_expected_fail_keys(expected_fail, module_ctx, src)?;

        let mut semantic = self.semantic;
        semantic.decl_bindings = decl_bindings;
        for entry in dynamic_unit_scales {
            let unit = entry.unit.clone();
            if semantic
                .dynamic_unit_scales
                .insert(unit.clone(), entry)
                .is_some()
            {
                return Err(GraphcalError::InternalError {
                    message: format!("duplicate dynamic unit semantic entry `{unit}`"),
                    src: src.clone(),
                    span: Span::new(0, 0).into(),
                });
            }
        }
        collect_dynamic_unit_refs(module_ctx, &mut semantic)?;
        collect_plot_exprs(&plots, &figures, &layers, module_ctx, src, &mut semantic)?;

        Ok(DagTIR {
            dag_id: self.dag_id,
            consts: self.consts,
            params: self.params,
            nodes: self.nodes,
            asserts,
            plots,
            figures,
            layers,
            included_plots,
            semantic,
            source_order,
            assumes_map,
            expected_fail,
            resolved_decl_types: self.resolved_decl_types,
            imported_bindings,
            projectable_outputs: std::collections::HashSet::new(),
        })
    }
}

// ---------------------------------------------------------------------------
mod ops;
#[cfg(test)]
use ops::unify_nat_poly_form;
pub use ops::{resolved_to_declared_type, unify_resolved_type};
pub(crate) use ops::{
    substitute_resolved_generic_arg, substitute_resolved_type, substitute_resolved_type_with_types,
};

// ---------------------------------------------------------------------------
mod type_expr;
#[cfg(test)]
use type_expr::resolve_type_expr;
use type_expr::{
    internal_error, module_resolve_error, resolve_generic_default_in_struct_scope,
    resolve_type_expr_in_struct_scope, resolve_type_expr_inner,
};
pub use type_expr::{resolve_hir_type_expr, resolve_type_expr_with_modules};

#[cfg(test)]
mod tests;
