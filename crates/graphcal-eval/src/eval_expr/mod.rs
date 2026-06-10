mod aggregations;
mod arithmetic;
mod conversions;
mod functions;
mod hir_eval;
mod numeric;
mod unit_scale;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::syntax::names::{
    ConstructorName, FieldName, ResolvedName, ScopedName, namespace,
};

use graphcal_compiler::registry::builtins::BuiltinFunction;
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::{Registry, TypeDef};
use graphcal_compiler::tir::typed::{
    DagTIR, ResolvedDagDependencies, ResolvedDomainConstraint, StructFieldConstraintKey,
};

use crate::decl_key::RuntimeDeclKey;

pub use graphcal_compiler::registry::runtime_value::RuntimeValue;
pub use hir_eval::{HirLocalValueMap, eval_hir_expr};
pub use unit_scale::resolve_unit_scale;
pub(in crate::eval_expr) use unit_scale::{checked_finite_scalar, checked_unit_scaled_value};
pub type RuntimeValueMap = HashMap<RuntimeDeclKey, RuntimeValue>;

/// Immutable evaluation environment shared across all expression evaluations.
///
/// Bundles built-in constants, built-in functions, the type/unit registry,
/// and source information for diagnostics, plus optional unfold context
/// for evaluating `unfold(...)` expressions inline.
pub struct EvalContext<'a> {
    pub builtin_consts: &'a HashMap<&'a str, f64>,
    pub builtin_fns: &'a HashMap<&'a str, BuiltinFunction>,
    pub registry: &'a Registry,
    pub src: &'a NamedSource<Arc<String>>,
    /// When set, enables inline evaluation of `ExprKind::Unfold` expressions.
    /// Contains the name of the node being evaluated and the declared types map.
    pub unfold_context: Option<UnfoldContext<'a>>,
    /// The enclosing file's full TIR.
    ///
    /// Used by [`eval_inline_dag_call`] to reach the file's flat per-DAG body
    /// map after semantic call metadata has selected a canonical DAG id.
    pub tir: &'a graphcal_compiler::tir::typed::TIR,
    /// DAG whose expression is currently being evaluated. When present, inline
    /// DAG calls can use HIR-derived canonical `DagId` / `ResolvedName<Decl>`
    /// identities instead of resolving source paths again at eval time.
    pub current_dag: Option<&'a graphcal_compiler::tir::typed::DagTIR>,
    /// Root-file values visible to nested inline DAG calls. This lets DAG-body
    /// self-imports route by their canonical source `DagId` rather than by a
    /// same-leaf name in the immediate caller's local value map.
    pub root_values: Option<&'a RuntimeValueMap>,
    /// Resolved domain constraints declared on struct/union member fields,
    /// keyed by owner-qualified struct/constructor/field identity. Looked up at
    /// every `ExprKind::ConstructorCall` to validate field values immediately.
    /// `None` means "skip the check" (used by paths that haven't resolved
    /// constraints yet, e.g. const-bound evaluation inside `exec_plan`).
    pub struct_field_constraints:
        Option<&'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
}

/// Context required to evaluate an `unfold(...)` expression inline.
///
/// Provides the self-referencing node name and declared types needed
/// to look up the range index for iterative evaluation.
pub struct UnfoldContext<'a> {
    pub self_name: &'a str,
    pub declared_types: &'a HashMap<ScopedName, DeclaredType>,
}

pub fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &ResolvedName<namespace::Index>,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

fn runtime_struct_type_def<'a>(
    type_name: &StructTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a TypeDef> {
    ctx.tir
        .dags
        .values()
        .find_map(|dag| {
            dag.semantic
                .type_defs
                .struct_types
                .get(type_name.resolved())
        })
        .or_else(|| ctx.registry.types.get_type(type_name.as_str()))
}

fn constructor_fields_for_runtime_struct<'a>(
    type_def: &'a TypeDef,
    type_name: &StructTypeRef,
) -> Option<&'a [graphcal_compiler::registry::types::StructField]> {
    type_def.union_members()?.iter().find_map(|member| {
        (member.name.as_str() == type_name.as_str()).then_some(member.fields.as_slice())
    })
}

fn find_struct_field_constraint<'a>(
    constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        constraints.get(&StructFieldConstraintKey::new(
            owning_type.clone(),
            constructor.clone(),
            field.clone(),
        ))
    })
}

impl EvalContext<'_> {
    /// Build a `GraphcalError::EvalError` using this context's source.
    pub fn eval_error(
        &self,
        message: impl Into<String>,
        span: graphcal_compiler::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::EvalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }

    /// Build a `GraphcalError::InternalError` using this context's source.
    pub fn internal_error(
        &self,
        message: impl Into<String>,
        span: graphcal_compiler::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::InternalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }
}

fn dag_decl_runtime_key(name: &ResolvedName<namespace::Decl>) -> RuntimeDeclKey {
    RuntimeDeclKey::resolved(name.clone())
}

fn imported_value_source_key(
    source: &graphcal_compiler::ir::lower::ImportedValueSource,
) -> RuntimeDeclKey {
    RuntimeDeclKey::resolved(ResolvedName::from_def(
        source.dag_id.clone(),
        source.source_name.clone(),
    ))
}

fn imported_value_source_value<'a>(
    source: &graphcal_compiler::ir::lower::ImportedValueSource,
    caller_values: &'a RuntimeValueMap,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a RuntimeValue> {
    match ctx.current_dag {
        Some(caller_dag) if source.dag_id.eq(&caller_dag.dag_id) => {
            caller_values.get(&imported_value_source_key(source))
        }
        _ if source.dag_id.eq(&ctx.tir.root_dag_id) => ctx
            .root_values
            .and_then(|values| values.get(&imported_value_source_key(source))),
        _ => None,
    }
}

/// Kahn-style topological sort over a dag body's combined dep graph.
///
/// Produces an order in which each const/param/node appears after every one
/// of its runtime and const dependencies. Cycles are impossible in a
/// well-typed dag body because compile-time dep collection rejects them.
/// # Errors
///
/// Returns the names left unordered if the dependency graph contains a
/// cycle — impossible for a well-typed dag body, but a silently truncated
/// order would surface later as a misleading "dag has no node X" error.
fn topo_order_for_dag_body(dag_tir: &DagTIR) -> Result<Vec<ScopedName>, Vec<ScopedName>> {
    topo_order_for_dag_body_resolved(dag_tir, &dag_tir.semantic.dependencies)
}

type ResolvedDeclKey = ResolvedName<namespace::Decl>;

fn topo_order_for_dag_body_resolved(
    dag_tir: &DagTIR,
    deps: &ResolvedDagDependencies,
) -> Result<Vec<ScopedName>, Vec<ScopedName>> {
    use std::collections::BTreeSet;

    let mut names: Vec<ScopedName> = dag_tir
        .source_order
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();

    let mut key_by_name: HashMap<ScopedName, ResolvedDeclKey> = HashMap::new();
    let mut name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    for name in &names {
        let Some(key) = dag_tir.resolved_decl_key_for_local(name) else {
            continue;
        };
        key_by_name.insert(name.clone(), key.clone());
        name_by_key.insert(key, name.clone());
    }

    let mut incoming: HashMap<ResolvedDeclKey, usize> = HashMap::new();
    let mut outgoing: HashMap<ResolvedDeclKey, Vec<ResolvedDeclKey>> = HashMap::new();
    for key in key_by_name.values() {
        incoming.insert(key.clone(), 0);
        outgoing.insert(key.clone(), Vec::new());
    }

    let add_edge =
        |from: &ResolvedDeclKey,
         to: &ResolvedDeclKey,
         incoming: &mut HashMap<ResolvedDeclKey, usize>,
         outgoing: &mut HashMap<ResolvedDeclKey, Vec<ResolvedDeclKey>>| {
            if let (Some(out), Some(deg)) = (outgoing.get_mut(from), incoming.get_mut(to)) {
                out.push(to.clone());
                *deg += 1;
            }
        };

    for (name, dep_set) in deps.runtime_deps.iter().chain(deps.const_deps.iter()) {
        for dep in dep_set {
            add_edge(dep, name, &mut incoming, &mut outgoing);
        }
    }

    let mut ready: BTreeSet<ResolvedDeclKey> = incoming
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    let mut order = Vec::with_capacity(names.len());
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        if let Some(name) = name_by_key.get(&key) {
            order.push(name.clone());
        }
        if let Some(succs) = outgoing.remove(&key) {
            for succ in succs {
                if let Some(deg) = incoming.get_mut(&succ) {
                    *deg -= 1;
                    if *deg == 0 {
                        ready.insert(succ);
                    }
                }
            }
        }
    }
    if order.len() == key_by_name.len() {
        Ok(order)
    } else {
        // Nodes with nonzero in-degree remain: a cycle. Report the leftovers
        // instead of silently truncating the evaluation order.
        let ordered: std::collections::HashSet<&ScopedName> = order.iter().collect();
        let remaining: Vec<ScopedName> = names
            .iter()
            .filter(|name| key_by_name.contains_key(*name) && !ordered.contains(name))
            .cloned()
            .collect();
        Err(remaining)
    }
}
