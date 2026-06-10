//! Type inference for expressions.
//!
//! Contains the main `infer_type` function that walks the AST and determines
//! the type (dimension, Bool, Int, struct, or indexed) of each expression.
//! Complex match arms are delegated to submodules.

mod collections;
mod control;
mod functions;
pub(super) mod hir;
mod rules;
mod scalar;

pub(super) use rules::match_arms_rule;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, ExprKind};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{
    GenericParamName, IndexName, NamePath, ResolvedIndexVariant, ResolvedName, ScopedName,
    namespace,
};

use super::{DeclaredType, InferredIndex, InferredType};

/// Collapse a syntactic index path to a leaf-only name at syntax boundaries.
///
/// Module-aware variant literals must use `ResolvedCollectionRefs`; this
/// adapter is only for callers that still receive syntax-only variant literals.
fn standalone_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

fn inference_owner(dag: Option<&crate::tir::typed::DagTIR>) -> crate::dag_id::DagId {
    dag.map_or_else(
        || crate::dag_id::DagId::root("<type-inference>"),
        |dag| dag.dag_id.clone(),
    )
}

fn resolved_value_index_variant<'a>(
    dag: Option<&'a crate::tir::typed::DagTIR>,
    written: &crate::syntax::names::WrittenVariantRef,
) -> Option<&'a ResolvedIndexVariant> {
    dag.map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.variant_literals.get(written))
}

fn infer_resolved_index_variant_label(resolved_variant: &ResolvedIndexVariant) -> InferredType {
    InferredType::Label(InferredIndex::from_resolved(
        resolved_variant.index().clone(),
    ))
}

/// Infer the type (dimension or struct) of an expression.
///
/// `owner_decl_name` is the name of the top-level declaration (node/const/param)
/// that contains this expression. It is threaded through to `infer_unfold` so
/// the unfold can look up the owning declaration's range index precisely.
/// Pass `None` when the owner is not known (e.g., in override dimension checks).
pub(super) fn infer_type(
    expr: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    infer_type_with_owner(
        expr,
        None,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

/// Infer the type of an expression, with the owning declaration name for
/// precise unfold range-index lookup.
pub(super) fn infer_type_with_owner(
    expr: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Recursion choke point: inference recurses once per tree level
    // (unbounded for left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        infer_type_with_owner_inner(
            expr,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors infer_type_with_owner's signature"
)]
fn infer_type_with_owner_inner(
    expr: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_) => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ExprKind::Integer(_) => Ok(InferredType::Int),
        ExprKind::Bool(_) => Ok(InferredType::Bool),
        ExprKind::StringLiteral(_) => Err(GraphcalError::DimensionMismatch {
            expected: "a numeric or boolean expression".to_string(),
            found: "string literal".to_string(),
            help: "string literals can only be used as arguments to datetime() or epoch()"
                .to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
        ExprKind::TypeSystemRef(name) => Err(GraphcalError::DimensionMismatch {
            expected: "a value expression".to_string(),
            found: format!("type-system name `{}`", name.value),
            help: "type-system names can only be used in include/import bindings".to_string(),
            src: src.clone(),
            span: name.span.into(),
        }),

        ExprKind::VariantLiteral { index, variant } => {
            let written = crate::syntax::names::WrittenVariantRef::IndexVariant(
                crate::syntax::names::WrittenIndexVariant::new(
                    index.value.clone(),
                    variant.value.clone(),
                ),
            );
            if let Some(resolved_variant) = resolved_value_index_variant(dag, &written) {
                return Ok(infer_resolved_index_variant_label(resolved_variant));
            }

            let index_name = standalone_index_name_from_path(&index.value);
            // Validate index exists
            let idx_def = registry
                .indexes
                .get_index(index_name.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index_name.clone(),
                    src: src.clone(),
                    span: index.span.into(),
                })?;
            // Validate variant exists in this index
            if !idx_def
                .variants()
                .iter()
                .any(|v| v.as_str() == variant.value.as_str())
            {
                return Err(GraphcalError::UnknownVariant {
                    index_name: index_name.clone(),
                    variant_name: variant.value.clone(),
                    src: src.clone(),
                    span: variant.span.into(),
                });
            }
            Ok(InferredType::Label(InferredIndex::with_owner(
                inference_owner(dag),
                index_name,
            )))
        }

        ExprKind::UnitLiteral { unit, .. } => {
            let dim = rules::resolve_unit_dimension_or_diagnose(unit, registry, src)?;
            Ok(InferredType::Scalar(dim))
        }

        ExprKind::ConstRef(ident) => {
            let written = crate::syntax::names::WrittenVariantRef::ConstPath(ident.value.clone());
            if let Some(resolved_variant) = resolved_value_index_variant(dag, &written) {
                return Ok(infer_resolved_index_variant_label(resolved_variant));
            }

            infer_decl_ref_type(&ident.value, ident.span, declared_types, dag, src).map_err(|err| {
                match err {
                    GraphcalError::UnknownGraphRef { name, src, span } => {
                        GraphcalError::UnknownConstRef { name, src, span }
                    }
                    err => err,
                }
            })
        }

        ExprKind::GraphRef(ident) => {
            infer_decl_ref_type(&ident.value, ident.span, declared_types, dag, src)
        }

        ExprKind::LocalRef(ident) => {
            local_types
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: ident.name.to_string(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
        }

        // --- Scalar operations ---
        ExprKind::BinOp { op, lhs, rhs } => scalar::infer_binop(
            expr,
            op,
            lhs,
            rhs,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::UnaryOp { op, operand } => scalar::infer_unary(
            op,
            operand,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Convert {
            expr: inner,
            target,
        } => scalar::infer_convert(
            inner,
            target,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => scalar::infer_display_timezone(
            expr,
            inner,
            timezone,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Function calls ---
        ExprKind::FnCall { callee, args, .. } => functions::infer_fn_call(
            callee,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Control flow ---
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => control::infer_if(
            condition,
            then_branch,
            else_branch,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Match {
            scrutinee, arms, ..
        } => control::infer_match(
            expr,
            scrutinee,
            arms,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Collections / indexed expressions ---
        ExprKind::ForComp { bindings, body } => collections::infer_for_comp(
            bindings,
            body,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::MapLiteral { entries } => collections::infer_map_or_table_literal(
            expr,
            entries,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::IndexAccess { expr: inner, args } => collections::infer_index_access(
            expr,
            inner,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => collections::infer_scan(
            source,
            init,
            acc_name,
            val_name,
            body,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => collections::infer_unfold(
            init,
            prev_name,
            curr_name,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::FieldAccess { expr: inner, field } => collections::infer_field_access(
            inner,
            field,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::ConstructorCall {
            callee,
            generic_args: constructor_generic_args,
            fields,
        } => collections::infer_constructor_call(
            expr,
            callee,
            constructor_generic_args,
            fields,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::InlineDagRef { path, args, output } => infer_inline_dag_ref(
            expr,
            path,
            args,
            output,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // `Sugar` and `UnresolvedRef` payloads are `Infallible` in `Resolved`
        // — both arms are statically unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar/UnresolvedRef(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) | ExprKind::UnresolvedRef(s) => match *s {},
    }
}

fn infer_decl_ref_type(
    name: &ScopedName,
    span: crate::syntax::span::Span,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    fn resolved_decl_type_for_key<'a>(
        dag: &'a crate::tir::typed::DagTIR,
        key: &ResolvedName<namespace::Decl>,
    ) -> Option<&'a crate::tir::typed::ResolvedTypeExpr> {
        dag.resolved_decl_types
            .iter()
            .find_map(|(name, resolved_type)| {
                (dag.resolved_decl_key_for_local(name).as_ref() == Some(key))
                    .then_some(resolved_type)
            })
    }

    let resolved_type = dag.and_then(|dag| {
        dag.semantic
            .dependencies
            .graph_ref_targets
            .get(&span)
            .or_else(|| dag.semantic.dependencies.const_ref_targets.get(&span))
            .and_then(|key| resolved_decl_type_for_key(dag, key))
            .or_else(|| dag.resolved_decl_types.get(name))
            .or_else(|| {
                (!name.is_qualified()).then(|| {
                    let mut candidates = dag
                        .resolved_decl_types
                        .iter()
                        .filter(|(candidate, _)| candidate.member() == name.member())
                        .map(|(_, resolved_type)| resolved_type);
                    candidates.next().filter(|_| candidates.next().is_none())
                })?
            })
    });

    if let Some(resolved) = resolved_type {
        let dim_sub = HashMap::new();
        let index_sub =
            HashMap::<GenericParamName, crate::registry::declared_type::IndexTypeRef>::new();
        let nat_sub = HashMap::new();
        return crate::tir::typed::substitute_resolved_type(
            resolved, &dim_sub, &index_sub, &nat_sub, src,
        );
    }

    declared_types
        .get(name)
        .or_else(|| {
            (!name.is_qualified()).then(|| {
                let mut candidates = declared_types
                    .iter()
                    .filter(|(candidate, _)| candidate.member() == name.member())
                    .map(|(_, declared)| declared);
                candidates.next().filter(|_| candidates.next().is_none())
            })?
        })
        .map(InferredType::from)
        .ok_or_else(|| GraphcalError::UnknownGraphRef {
            name: name.clone(),
            src: src.clone(),
            span: span.into(),
        })
}

/// Infer the type of an inline DAG invocation `@<path>(args).<out>`.
///
/// Semantic TIR carries HIR-derived [`ResolvedInlineDagCall`] metadata for
/// call routing. The canonical `DagId` / `ResolvedName<Decl>` identities are
/// authoritative.
#[expect(
    clippy::too_many_arguments,
    reason = "passes inference context through"
)]
fn infer_inline_dag_ref(
    expr: &Expr,
    path: &crate::syntax::ast::ModulePath,
    args: &[crate::desugar::resolved_ast::ParamBinding],
    output: &crate::syntax::span::Spanned<crate::syntax::names::DeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::syntax::names::{ResolvedName, namespace};

    type ResolvedDeclKey = ResolvedName<namespace::Decl>;

    let display_path = path.display_path();
    let dag = dag.ok_or_else(|| GraphcalError::InternalError {
        message: format!("semantic TIR missing current DAG for inline-DAG call `{display_path}`"),
        src: src.clone(),
        span: expr.span.into(),
    })?;
    let resolved_call = dag
        .semantic
        .inline_dag_refs
        .calls
        .get(&expr.span)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("semantic TIR missing inline-DAG call metadata for `{display_path}`"),
            src: src.clone(),
            span: expr.span.into(),
        })?;
    let dag_tir = tir
        .dags
        .get(&resolved_call.target)
        .ok_or_else(|| GraphcalError::UnknownDag {
            name: resolved_call.target.to_string(),
            src: src.clone(),
            span: path.span.into(),
        })?;

    let mut required_param_keys = std::collections::HashSet::new();
    let mut param_decl_types_by_key: HashMap<ResolvedDeclKey, DeclaredType> = HashMap::new();
    for p in &dag_tir.params {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&p.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            if let Some(key) = dag_tir.resolved_decl_key_for_local(&p.name) {
                if p.default_expr.is_none() {
                    required_param_keys.insert(key.clone());
                }
                param_decl_types_by_key.insert(key, dt);
            }
        }
    }
    let mut node_decl_types_by_key: HashMap<ResolvedDeclKey, DeclaredType> = HashMap::new();
    for n in &dag_tir.nodes {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&n.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            if let Some(key) = dag_tir.resolved_decl_key_for_local(&n.name) {
                node_decl_types_by_key.insert(key, dt);
            }
        }
    }

    let mut bound_resolved_names: std::collections::HashSet<ResolvedDeclKey> =
        std::collections::HashSet::with_capacity(args.len());
    for binding in args {
        let binding_name = binding.name.name.as_str();
        let target = resolved_call
            .arg_targets
            .get(&binding.name.span)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "semantic TIR has no inline-DAG arg target for binding `{binding_name}`"
                ),
                src: src.clone(),
                span: binding.name.span.into(),
            })?;
        bound_resolved_names.insert(target.clone());
        let expected = param_decl_types_by_key.get(target).ok_or_else(|| {
            GraphcalError::UnknownInlineDagParam {
                name: target.as_str().to_string(),
                dag_name: display_path.clone(),
                src: src.clone(),
                span: binding.name.span.into(),
            }
        })?;
        let found = infer_type(
            &binding.value,
            declared_types,
            local_types,
            Some(dag),
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        if !super::helpers::types_match(expected, &found) {
            return Err(GraphcalError::InlineDagArgDimensionMismatch {
                param_name: binding_name.to_string(),
                expected: super::helpers::format_declared_type(expected, registry),
                found: super::helpers::format_inferred_type(&found, registry),
                src: src.clone(),
                span: binding.value.span.into(),
            });
        }
    }

    let mut missing: Vec<String> = required_param_keys
        .iter()
        .filter(|p| !bound_resolved_names.contains(*p))
        .map(|p| p.as_str().to_string())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(GraphcalError::MissingInlineDagBindings {
            missing,
            dag_name: display_path.clone(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let output_key = &resolved_call.output.value;
    let output_decl = node_decl_types_by_key.get(output_key).ok_or_else(|| {
        GraphcalError::UnknownInlineDagOutput {
            name: output_key.as_str().to_string(),
            dag_name: display_path.clone(),
            src: src.clone(),
            span: output.span.into(),
        }
    })?;
    let output_name = output_key.as_str();
    if !dag_tir.pub_nodes.contains(output_name) {
        return Err(GraphcalError::ImportPrivateItem {
            name: output_name.to_string(),
            file_path: display_path,
            src: src.clone(),
            span: output.span.into(),
        });
    }
    Ok(InferredType::from(output_decl))
}
