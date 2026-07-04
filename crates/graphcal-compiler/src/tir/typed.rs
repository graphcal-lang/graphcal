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
use crate::registry::types::{Registry, TypeDef, TypeGenericConstraint};
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_resolve::ModuleResolver;

mod model;
pub use model::*;

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
        // Layer the sources so the most authoritative wins on key collisions:
        //   builtins  <  imported_decl_types  <  imported_values  <  resolved_decl_types
        // A DAG's own resolved decls always shadow imports of the same name —
        // necessary because `merge_dependency` may propagate placeholder
        // imported decl types from an inline DAG's self-import back onto the
        // importer for names the importer already declares itself.
        let mut declared_types = HashMap::new();
        for name in crate::registry::builtins::builtin_constants().keys() {
            declared_types.insert(
                ScopedName::local(*name),
                crate::registry::declared_type::DeclaredType::Scalar(Dimension::dimensionless()),
            );
        }
        for (name, dt) in &self.imported_decl_types {
            declared_types.insert(name.clone(), dt.clone());
        }
        for (name, (_rv, dt)) in &self.imported_values {
            declared_types.insert(name.clone(), dt.clone());
        }
        for (name, resolved) in &self.resolved_decl_types {
            let dt = resolved_to_declared_type(resolved, src)?;
            declared_types.insert(name.clone(), dt);
        }
        Ok(declared_types)
    }

    /// Populate this DAG's `pub_nodes` set from its source body.
    pub fn populate_pub_nodes(&mut self, body: &[crate::desugar::desugared_ast::Declaration]) {
        use crate::desugar::desugared_ast::DeclKind;

        for decl in body {
            if let DeclKind::Node(n) = &decl.kind
                && n.visibility.is_public()
            {
                self.pub_nodes.insert(n.name.value.clone());
            }
        }
    }

    /// Return the resolved declaration key for a declaration visible from this DAG.
    ///
    /// Qualified source keys synthesize a child owner under this DAG so
    /// source-facing entries still use resolved identities instead of
    /// source-keyed runtime maps.
    #[must_use]
    pub fn resolved_decl_key_for_local(&self, name: &ScopedName) -> Option<ResolvedDeclName> {
        if let Some(resolved) = self.semantic.decl_bindings.get(name) {
            return Some(resolved.clone());
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
                .filter_map(|candidate| resolved_decl_key(&self.dag_id, candidate));
            if let Some(candidate) = candidates.next()
                && candidates.next().is_none()
            {
                return Some(candidate);
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
    let owner_for_ctx = root_dag_id.clone();
    let ctx = ModuleTypeContext::new(&owner_for_ctx, module_resolver, module_types);
    type_resolve_impl(ir, root_dag_id, src, ctx)
}

fn type_resolve_impl(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<TIR, GraphcalError> {
    let imported_value_sources_for_hir = ir.imported_value_sources.clone();
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
        &ir.imported_values,
        &ir.imported_decl_types,
        &imported_value_sources_for_hir,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.included_plots,
        ir.source_order,
        ir.assert_names,
        ir.assumes_map,
        ir.expected_fail,
        ir.imported_values,
        ir.imported_decl_types,
        ir.imported_value_sources,
        module_ctx,
        src,
    )?;
    lower_dynamic_unit_scales(&ir.registry, module_ctx, &mut root_dag.semantic);
    augment_runtime_deps_for_dynamic_units(&mut root_dag.semantic);
    check_hir_body_policies(
        &root_dag.semantic,
        &ir.registry,
        &ir.pub_names,
        module_ctx,
        src,
    )?;
    let mut dags = DagRegistry::new();
    dags.insert(root_dag_id.clone(), root_dag);
    Ok(TIR {
        registry: ir.registry,
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
    let ctx = ModuleTypeContext::new(dag_id, module_resolver, module_types);
    type_resolve_single_impl(ir, dag_id, src, ctx)
}

fn type_resolve_single_impl(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<DagTIR, GraphcalError> {
    let imported_value_sources_for_hir = ir.imported_value_sources.clone();
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
        &ir.imported_values,
        &ir.imported_decl_types,
        &imported_value_sources_for_hir,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.included_plots,
        ir.source_order,
        ir.assert_names,
        ir.assumes_map,
        ir.expected_fail,
        ir.imported_values,
        ir.imported_decl_types,
        ir.imported_value_sources,
        module_ctx,
        src,
    )?;
    lower_dynamic_unit_scales(&ir.registry, module_ctx, &mut dag.semantic);
    augment_runtime_deps_for_dynamic_units(&mut dag.semantic);
    check_hir_body_policies(&dag.semantic, &ir.registry, &ir.pub_names, module_ctx, src)?;
    Ok(dag)
}

/// Lower the registry's dynamic unit scale expressions to HIR into the DAG's
/// semantic body.
///
/// A scale expression that fails to lower is omitted; evaluation reports a
/// dynamic-scale resolution error if such a unit is actually used. This keeps
/// the laziness of the previous evaluation-time path, where an unused broken
/// dynamic unit never surfaced an error.
fn lower_dynamic_unit_scales(
    registry: &Registry,
    ctx: ModuleTypeContext<'_>,
    semantic: &mut DagSemanticBody,
) {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let expr_ctx = hir::ExprLoweringContext::new(ctx.owner, ctx.resolver, &generic_scope)
        .with_prelude(&prelude)
        .with_decl_bindings(&semantic.decl_bindings);
    for (name, _dim, scale) in registry.units.all_units() {
        if let crate::registry::types::UnitScale::Dynamic { scale_expr, .. } = scale
            && let Ok(lowered) = hir::lower_expr(scale_expr, expr_ctx)
        {
            semantic.dynamic_unit_scales.insert(name.clone(), lowered);
        }
    }
}

/// Internal helper: resolve type annotations for the const/param/node
/// declarations of a single DAG, returning a partially-built [`DagTIR`].
#[expect(
    clippy::too_many_arguments,
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
    imported_values: &HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    imported_decl_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
) -> Result<DagTIRSeed, GraphcalError> {
    let mut resolved_decl_types = HashMap::new();
    let no_generic_params: &[GenericParamName] = &[];

    // A merged dependency declaration's type annotation keeps the dependency
    // file's offsets, so resolution errors must render against its own source
    // rather than the importer's `src` (#868).
    for entry in &consts {
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &params {
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &nodes {
        let entry_ctx = module_ctx.with_owner(&entry.type_resolution_owner);
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            &entry.type_resolution_owner,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            entry.src.resolve(src),
            Some(entry_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }

    let LoweredDagExpressions {
        exprs: expressions,
        domain_bounds,
    } = lower_resolved_expressions(
        &consts,
        &params,
        &nodes,
        asserts,
        module_ctx,
        imported_value_sources,
        src,
    )?;
    let dependencies =
        collect_resolved_dag_dependencies(&consts, &params, &nodes, &expressions, module_ctx, src)?;
    let collection_refs = collect_resolved_collection_refs(
        &expressions,
        &domain_bounds,
        &resolved_decl_types,
        imported_values,
        imported_decl_types,
        module_ctx,
        src,
    )?;
    let constructor_refs =
        collect_resolved_constructor_refs(&expressions, &domain_bounds, module_ctx, src)?;
    let inline_dag_refs = collect_resolved_inline_dag_refs(&expressions);
    let type_defs = collect_resolved_type_defs(
        &resolved_decl_types,
        &constructor_refs,
        imported_values,
        imported_decl_types,
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
        inline_dag_refs,
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

fn collect_resolved_type_defs(
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    constructor_refs: &ResolvedConstructorRefs,
    imported_values: &HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    imported_decl_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
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
    for declared in imported_decl_types
        .values()
        .chain(imported_values.values().map(|(_value, declared)| declared))
    {
        collect_struct_type_defs_from_declared_type(declared, ctx, registry, src, &mut defs)?;
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
        crate::registry::declared_type::DeclaredType::Struct(name, type_args) => {
            record_resolved_struct_type_def(name.resolved(), ctx, registry, src, defs)?;
            for arg in type_args {
                collect_struct_type_defs_from_declared_type(arg, ctx, registry, src, defs)?;
            }
        }
        crate::registry::declared_type::DeclaredType::Indexed { element, .. } => {
            collect_struct_type_defs_from_declared_type(element, ctx, registry, src, defs)?;
        }
        crate::registry::declared_type::DeclaredType::Scalar(_)
        | crate::registry::declared_type::DeclaredType::Bool
        | crate::registry::declared_type::DeclaredType::Int
        | crate::registry::declared_type::DeclaredType::Datetime(_)
        | crate::registry::declared_type::DeclaredType::IndexArg(_) => {}
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
            name, type_args, ..
        } => {
            record_resolved_struct_type_def(name, ctx, registry, src, defs)?;
            for arg in type_args {
                collect_struct_type_defs_from_resolved_type(arg, ctx, registry, src, defs)?;
            }
        }
        ResolvedTypeExpr::Indexed { base, indexes: _ } => {
            collect_struct_type_defs_from_resolved_type(base, ctx, registry, src, defs)?;
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::IndexArg(_)
        | ResolvedTypeExpr::Scalar(_)
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

    for param in &type_def.generic_params {
        if let Some(default) = &param.default {
            let resolved =
                resolve_type_expr_in_struct_scope(default, name, type_def, ctx, registry, src)?;
            defs.generic_defaults
                .insert((name.clone(), param.name.clone()), resolved);
        }
    }

    if let Some(members) = type_def.union_members() {
        let generic_scope = generic_scope_for_type_def(name, type_def, src)?;
        let prelude = hir::PreludeTypeScope::graphcal();
        let bound_expr_ctx =
            hir::ExprLoweringContext::new(name.owner(), ctx.resolver, &generic_scope)
                .with_prelude(&prelude);
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
                    src,
                )?;
                let bounds = lower_domain_bounds(&field.type_ann, bound_expr_ctx, src)?;
                if !bounds.is_empty() {
                    defs.field_bounds.insert(key.clone(), bounds);
                }
                defs.field_types.insert(key, resolved);
            }
        }
    }

    defs.struct_types.insert(name.clone(), type_def.clone());
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
            TypeGenericConstraint::Unconstrained => crate::syntax::ast::GenericConstraint::Type,
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
fn lower_resolved_expressions(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    asserts: &[crate::ir::lower::AssertEntry],
    ctx: ModuleTypeContext<'_>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<LoweredDagExpressions, GraphcalError> {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let decl_bindings = collect_hir_decl_bindings(
        ctx.owner,
        consts,
        params,
        nodes,
        imported_value_sources,
        src,
    )?;
    let lower_bounds_in = |type_ann: &crate::desugar::desugared_ast::TypeExpr,
                           resolution_owner: &crate::dag_id::DagId,
                           body_src: &NamedSource<Arc<String>>| {
        let expr_ctx =
            hir::ExprLoweringContext::new(resolution_owner, ctx.resolver, &generic_scope)
                .with_prelude(&prelude)
                .with_decl_bindings(&decl_bindings);
        lower_domain_bounds(type_ann, expr_ctx, body_src)
    };
    let mut exprs = ResolvedExpressions::default();
    let mut domain_bounds = HashMap::new();

    // Domain-bound and key errors for a merged dependency body must render
    // against the dependency's own source, not the importer's `src` (#868).
    for entry in consts {
        let body_src = entry.src.resolve(src);
        let key = decl_key_or_internal_error(ctx.owner, &entry.name, entry.span, body_src)?;
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, body_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        exprs.consts.insert(key, entry.expr.clone());
    }
    for entry in params {
        let body_src = entry.src.resolve(src);
        let key = decl_key_or_internal_error(ctx.owner, &entry.name, entry.span, body_src)?;
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, body_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        let Some(expr) = &entry.default_expr else {
            continue;
        };
        exprs.param_defaults.insert(key, expr.clone());
    }
    for entry in nodes {
        let body_src = entry.src.resolve(src);
        let key = decl_key_or_internal_error(ctx.owner, &entry.name, entry.span, body_src)?;
        let bounds = lower_bounds_in(&entry.type_ann, &entry.type_resolution_owner, body_src)?;
        if !bounds.is_empty() {
            domain_bounds.insert(key.clone(), bounds);
        }
        exprs.nodes.insert(key, entry.expr.clone());
    }
    for entry in asserts {
        let key =
            decl_key_or_internal_error(ctx.owner, &entry.name, entry.span, entry.src.resolve(src))?;
        exprs.asserts.insert(key, entry.body.clone());
    }

    Ok(LoweredDagExpressions {
        exprs,
        domain_bounds,
    })
}

/// Populate the semantic body's plot expression maps from the IR's lowered
/// plot/figure/layer entries and run the reference-collection walks over
/// them.
///
/// Plot bodies were lowered (best-effort) at
/// [`crate::ir::lower::UnfrozenIR::freeze`]; an entry without a complete
/// body is omitted here, and the runtime then skips that plot.
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
                   collection_refs: &mut ResolvedCollectionRefs,
                   constructor_refs: &mut ResolvedConstructorRefs|
     -> Result<(), GraphcalError> {
        collect_resolved_collection_refs_from_expr(expr, ctx, src, collection_refs)?;
        collect_resolved_constructor_refs_from_expr(expr, ctx, src, constructor_refs)
    };

    for entry in plots {
        let Some(body) = &entry.body else {
            continue;
        };
        for (_, expr) in &body.encodings {
            collect(
                expr,
                &mut semantic.collection_refs,
                &mut semantic.constructor_refs,
            )?;
        }
        for field in body.mark_properties.iter().chain(&body.properties) {
            collect(
                &field.value,
                &mut semantic.collection_refs,
                &mut semantic.constructor_refs,
            )?;
        }
        plot_exprs.plots.insert(entry.name.clone(), body.clone());
    }

    for (name, fields, is_figure) in figures
        .iter()
        .map(|entry| (&entry.name, &entry.fields, true))
        .chain(
            layers
                .iter()
                .map(|entry| (&entry.name, &entry.fields, false)),
        )
    {
        for field in fields {
            collect(
                &field.value,
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

/// Build the canonical declaration key for `name`, reporting an internal
/// error when the name cannot form one.
fn decl_key_or_internal_error(
    owner: &crate::dag_id::DagId,
    name: &ScopedName,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDeclName, GraphcalError> {
    resolved_decl_key(owner, name).ok_or_else(|| {
        internal_error(
            format!("could not build canonical declaration key for `{name}`"),
            src,
            span,
        )
    })
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
                kind_span: bound.kind_span,
                value,
                span: bound.span,
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
/// - const bodies must not `@`-reference runtime declarations (E020-style
///   [`GraphcalError::GraphRefInConst`]) or use runtime units in literals / conversion targets;
/// - no body may `@`-reference an assert declaration
///   ([`GraphcalError::GraphRefToAssert`]);
/// - A10(c) / V004: bodies of non-bindable kinds owned by this module must
///   not mention variant literals of the module's own `pub(bind)` indexes
///   ([`GraphcalError::PubIndexVariantLiteral`]). Params are exempt (A10(a));
///   sink kinds (assert/plot/figure/layer) are checked only when `pub`.
fn check_hir_body_policies(
    semantic: &DagSemanticBody,
    registry: &Registry,
    pub_names: &std::collections::HashSet<DeclName>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let checker = HirPolicyChecker { registry, ctx, src };
    let local = |key: &ResolvedDeclName| key.owner() == ctx.owner;
    let is_pub = |leaf: &str| pub_names.contains(&DeclName::expect_valid(leaf));

    for (key, expr) in &semantic.expressions.consts {
        checker.check_expr(expr, true, local(key))?;
    }
    for (key, bounds) in &semantic.domain_bounds {
        if semantic.expressions.consts.contains_key(key) {
            for bound in bounds {
                checker.check_expr(&bound.value, true, local(key))?;
            }
        }
        // Domain bounds are evaluated without a host function registry, so
        // extern calls are rejected in all of them (const and runtime alike).
        for bound in bounds {
            if let Some((ext, span)) = hir::find_extern_call(&bound.value) {
                return Err(GraphcalError::ExternCallNotAllowed {
                    name: ext.to_string(),
                    context: "domain bound".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        }
    }
    // Dynamic unit scales resolve in contexts that carry no host function
    // registry (including dependency export); reject extern calls there.
    for expr in semantic.dynamic_unit_scales.values() {
        if let Some((ext, span)) = hir::find_extern_call(expr) {
            return Err(GraphcalError::ExternCallNotAllowed {
                name: ext.to_string(),
                context: "unit scale expression".to_string(),
                src: src.clone(),
                span: span.into(),
            });
        }
    }
    for (key, expr) in &semantic.expressions.nodes {
        checker.check_expr(expr, false, local(key))?;
    }
    for expr in semantic.expressions.param_defaults.values() {
        // Params are exempt from A10 (a rebinding importer is forced to
        // rebind the param too — V005 at the include site).
        checker.check_expr(expr, false, false)?;
    }
    for (key, body) in &semantic.expressions.asserts {
        let check_literals = local(key) && is_pub(key.as_str());
        match body {
            hir::AssertBody::Expr(expr) => checker.check_expr(expr, false, check_literals)?,
            hir::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                checker.check_expr(actual, false, check_literals)?;
                checker.check_expr(expected, false, check_literals)?;
                checker.check_expr(tolerance, false, check_literals)?;
            }
        }
    }
    for (name, body) in &semantic.plot_exprs.plots {
        let check_literals = !name.is_qualified() && is_pub(name.member());
        for (_, expr) in &body.encodings {
            checker.check_expr(expr, false, check_literals)?;
        }
        for field in body.mark_properties.iter().chain(&body.properties) {
            checker.check_expr(&field.value, false, check_literals)?;
        }
    }
    for (name, fields) in semantic
        .plot_exprs
        .figures
        .iter()
        .chain(&semantic.plot_exprs.layers)
    {
        let check_literals = !name.is_qualified() && is_pub(name.member());
        for field in fields {
            checker.check_expr(&field.value, false, check_literals)?;
        }
    }
    Ok(())
}

struct HirPolicyChecker<'a> {
    registry: &'a Registry,
    ctx: ModuleTypeContext<'a>,
    src: &'a NamedSource<Arc<String>>,
}

impl HirPolicyChecker<'_> {
    fn check_expr(
        &self,
        expr: &hir::Expr,
        const_body: bool,
        check_pub_bind_literals: bool,
    ) -> Result<(), GraphcalError> {
        // Recursion choke point: recurses once per tree level.
        crate::stack::with_stack_growth(|| {
            self.check_expr_inner(expr, const_body, check_pub_bind_literals)
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive ExprKind policy walk"
    )]
    fn check_expr_inner(
        &self,
        expr: &hir::Expr,
        const_body: bool,
        check_pub_bind_literals: bool,
    ) -> Result<(), GraphcalError> {
        let recurse =
            |inner: &hir::Expr| self.check_expr(inner, const_body, check_pub_bind_literals);
        match &expr.kind {
            hir::ExprKind::Error
            | hir::ExprKind::Number(_)
            | hir::ExprKind::Integer(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::StringLiteral(_)
            | hir::ExprKind::TypeSystemRef(_)
            | hir::ExprKind::ConstRef(_)
            | hir::ExprKind::LocalRef(_) => Ok(()),
            hir::ExprKind::UnitLiteral { unit, .. } => self.check_const_unit_expr(unit, const_body),
            hir::ExprKind::GraphRef(target) => {
                // Use the whole `@name` span (the reference Spanned covers
                // only the name) so the label includes the sigil.
                self.check_graph_ref(target, expr.span, const_body)
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
                self.check_const_unit_expr(target, const_body)?;
                recurse(operand)
            }
            hir::ExprKind::FnCall { callee, args, .. } => {
                // Extern functions are runtime-provided; const expressions
                // evaluate at compile time without a host function registry.
                if const_body && let hir::FunctionRef::External(ext) = &callee.value {
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
            hir::ExprKind::InlineDagRef { args, .. } => {
                args.iter().try_for_each(|arg| recurse(&arg.value))
            }
        }
    }

    fn check_const_unit_expr(
        &self,
        unit: &crate::desugar::desugared_ast::UnitExpr,
        const_body: bool,
    ) -> Result<(), GraphcalError> {
        if !const_body {
            return Ok(());
        }
        for term in &unit.terms {
            let Some(info) = self.registry.units.get_unit(&term.name.value) else {
                // Unknown units get their own diagnostics from dimension checking.
                continue;
            };
            if !info.constness.is_const() {
                return Err(GraphcalError::NonConstUnitInConst {
                    name: term.name.value.clone(),
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
        const_body: bool,
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
        if const_body && !kind.is_const() {
            return Err(GraphcalError::GraphRefInConst {
                name: ScopedName::local(target.value.as_str()),
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
    collect_resolved_dag_dependencies, collect_resolved_decl_bindings,
    collect_resolved_inline_dag_refs, resolve_expected_fail_keys,
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
        assert_names: std::collections::HashSet<ScopedName>,
        assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
        expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
        imported_values: HashMap<
            ScopedName,
            (
                crate::registry::runtime_value::RuntimeValue,
                crate::registry::declared_type::DeclaredType,
            ),
        >,
        imported_decl_types: HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
        imported_value_sources: HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
        module_ctx: ModuleTypeContext<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<DagTIR, GraphcalError> {
        let decl_bindings = collect_resolved_decl_bindings(
            module_ctx,
            &self.consts,
            &self.params,
            &self.nodes,
            &imported_values,
            &imported_decl_types,
            &imported_value_sources,
            src,
        )?;
        let expected_fail = resolve_expected_fail_keys(expected_fail, module_ctx, src)?;

        let mut semantic = self.semantic;
        semantic.decl_bindings = decl_bindings;
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
            assert_names,
            assumes_map,
            expected_fail,
            resolved_decl_types: self.resolved_decl_types,
            domain_constraints: HashMap::new(), // Resolved later in compile()
            imported_values,
            imported_decl_types,
            imported_value_sources,
            pub_nodes: std::collections::HashSet::new(),
        })
    }
}

// ---------------------------------------------------------------------------
mod ops;
#[cfg(test)]
use ops::unify_nat_poly_form;
pub use ops::{
    resolved_to_declared_type, substitute_resolved_type, substitute_resolved_type_with_types,
    unify_resolved_type,
};

// ---------------------------------------------------------------------------
mod type_expr;
#[cfg(test)]
use type_expr::resolve_type_expr;
use type_expr::{
    internal_error, module_resolve_error, resolve_type_expr_in_struct_scope,
    resolve_type_expr_inner,
};
pub use type_expr::{resolve_hir_type_expr, resolve_type_expr_with_modules};

#[cfg(test)]
mod tests;
