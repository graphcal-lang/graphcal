//! Execution plan — the result of compiling a TIR.
//!
//! Contains evaluated const values, topologically sorted runtime declarations,
//! and their expressions, ready for evaluation.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::resolved_ast::{AssertBody, Expr, FigureDecl, LayerDecl, PlotDecl};
use graphcal_compiler::syntax::names::{FieldName, ScopedName, StructTypeName};
use graphcal_compiler::syntax::span::Span;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::eval_expr::{EvalContext, RuntimeValue, eval_expr};
use graphcal_compiler::registry::builtins::{builtin_constants, builtin_functions};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{ResolvedDomainConstraint, TIR};

/// An assert body entry for execution.
#[derive(Debug, Clone)]
pub struct AssertBodyEntry {
    pub(crate) name: ScopedName,
    pub(crate) body: AssertBody,
    pub(crate) span: Span,
}

/// A plot body entry for execution.
#[derive(Debug, Clone)]
pub struct PlotBodyEntry {
    pub(crate) name: ScopedName,
    pub(crate) decl: PlotDecl,
    /// Whether this plot is `pub` (visible in standalone output).
    pub(crate) is_pub: bool,
}

/// A figure body entry for execution.
#[derive(Debug, Clone)]
pub struct FigureBodyEntry {
    pub(crate) name: ScopedName,
    pub(crate) decl: FigureDecl,
}

/// A layer body entry for execution.
#[derive(Debug, Clone)]
pub struct LayerBodyEntry {
    pub(crate) name: ScopedName,
    pub(crate) decl: LayerDecl,
}

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: HashMap<ScopedName, RuntimeValue>,
    /// Pre-evaluated values imported from dependency files.
    /// These are injected directly into the evaluation environment.
    /// Iterated once during env setup; feeds into `HashMap` (key-lookup only).
    pub(crate) imported_values: HashMap<ScopedName, RuntimeValue>,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub(crate) topo_order: Vec<ScopedName>,
    /// Runtime expressions keyed by declaration name (params + nodes).
    /// Key-lookup only, order irrelevant.
    pub(crate) expressions: HashMap<ScopedName, Expr>,
    /// Assert bodies in source order.
    pub(crate) assert_bodies: Vec<AssertBodyEntry>,
    /// Plot declarations in source order.
    pub(crate) plot_bodies: Vec<PlotBodyEntry>,
    /// Figure declarations in source order.
    pub(crate) figure_bodies: Vec<FigureBodyEntry>,
    /// Layer declarations in source order.
    pub(crate) layer_bodies: Vec<LayerBodyEntry>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<ScopedName, graphcal_compiler::ir::resolve::ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: HashMap<ScopedName, ResolvedDomainConstraint>,
    /// Resolved domain constraints for struct/union member fields, keyed by
    /// `(struct type name, field name)`. Looked up at every
    /// `ExprKind::ConstructorCall` evaluation to validate field values.
    pub(crate) struct_field_constraints:
        HashMap<(StructTypeName, FieldName), ResolvedDomainConstraint>,
}

/// Compile a TIR into an execution plan.
///
/// This performs:
/// 1. Topological sort of const declarations + compile-time evaluation
/// 2. Topological sort of runtime declarations (params + nodes) into evaluation order
///
/// # Errors
///
/// Returns a [`GraphcalError`] if there is a cyclic dependency or if
/// const evaluation fails.
pub fn compile(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<ExecPlan, GraphcalError> {
    let const_values = eval_consts_from_tir(tir, src)?;
    let (topo_order, expressions) = build_runtime_dag(tir, src)?;

    let assert_bodies: Vec<AssertBodyEntry> = tir
        .root()
        .asserts
        .iter()
        .map(|entry| AssertBodyEntry {
            name: entry.name.clone(),
            body: entry.body.clone(),
            span: entry.span,
        })
        .collect();

    let plot_bodies: Vec<PlotBodyEntry> = tir
        .root()
        .plots
        .iter()
        .map(|entry| PlotBodyEntry {
            name: entry.name.clone(),
            decl: entry.decl.clone(),
            is_pub: entry.is_pub,
        })
        .collect();

    let figure_bodies: Vec<FigureBodyEntry> = tir
        .root()
        .figures
        .iter()
        .map(|entry| FigureBodyEntry {
            name: entry.name.clone(),
            decl: entry.decl.clone(),
        })
        .collect();

    let layer_bodies: Vec<LayerBodyEntry> = tir
        .root()
        .layers
        .iter()
        .map(|entry| LayerBodyEntry {
            name: entry.name.clone(),
            decl: entry.decl.clone(),
        })
        .collect();

    // Resolve domain constraints from type annotations.
    let domain_constraints = resolve_domain_constraints(tir, &const_values, src)?;
    // Resolve domain constraints declared on struct/union member fields.
    let struct_field_constraints = resolve_struct_field_constraints(tir, &const_values, src)?;

    // Validate struct field constraints against const struct values. Const
    // evaluation runs before field constraints are resolved (the constraint
    // bound exprs themselves need const values), so the violation check is
    // deferred to here. Top-level struct-typed consts that violate any field
    // constraint produce a compile-time `DomainViolation`.
    check_const_struct_field_constraints_at_compile_time(
        tir,
        &const_values,
        &struct_field_constraints,
        src,
    )?;

    Ok(ExecPlan {
        const_values,
        imported_values: tir
            .root()
            .imported_values
            .iter()
            .map(|(k, (v, _dt))| (k.clone(), v.clone()))
            .collect(),
        topo_order,
        expressions,
        assert_bodies,
        plot_bodies,
        figure_bodies,
        layer_bodies,
        assumes_map: tir.root().assumes_map.clone(),
        expected_fail: tir.root().expected_fail.clone(),
        domain_constraints,
        struct_field_constraints,
    })
}

/// Topologically sort and evaluate const declarations from a TIR.
pub fn eval_consts_from_tir(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, RuntimeValue>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    if tir.root().consts.is_empty() {
        return Ok(HashMap::new());
    }

    let mut graph = DiGraph::<ScopedName, ()>::new();
    let mut index_map: HashMap<ScopedName, petgraph::graph::NodeIndex> = HashMap::new();

    // Sort consts by name for canonical tie-breaking among incomparable nodes.
    let mut sorted_consts: Vec<&_> = tir.root().consts.iter().collect();
    sorted_consts.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in &sorted_consts {
        let idx = graph.add_node(entry.name.clone());
        index_map.insert(entry.name.clone(), idx);
    }

    for entry in &tir.root().consts {
        if let Some(deps) = tir.root().const_deps.get(&entry.name) {
            let from = index_map[&entry.name];
            for dep in deps {
                let to = index_map[dep];
                graph.add_edge(to, from, ());
            }
        }
    }

    let sorted = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = tir
            .root()
            .consts
            .iter()
            .find(|e| &e.name == cycle_node)
            .map_or_else(|| Span::new(0, 0), |e| e.span);
        GraphcalError::CyclicDependency {
            name: cycle_node.to_string().into(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    let const_exprs: HashMap<ScopedName, &Expr> = tir
        .root()
        .consts
        .iter()
        .map(|entry| (entry.name.clone(), &entry.expr))
        .collect();

    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();
    let mut const_values: HashMap<ScopedName, RuntimeValue> = HashMap::new();

    let ctx = EvalContext {
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        unfold_context: None,
        tir,
        struct_field_constraints: None,
    };

    for idx in sorted {
        let name = &graph[idx];
        let expr = const_exprs[name];
        let val = eval_expr(expr, &const_values, &empty_locals, &ctx)?;
        const_values.insert(name.clone(), val);
    }

    Ok(const_values)
}

/// Build a topologically sorted runtime DAG from params and nodes in a TIR.
fn build_runtime_dag(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(Vec<ScopedName>, HashMap<ScopedName, Expr>), GraphcalError> {
    // Merge params and nodes, then sort by name for canonical tie-breaking
    // among incomparable nodes in the topological sort.
    enum DeclRef<'a> {
        Param(&'a graphcal_compiler::ir::lower::ParamEntry),
        Node(&'a graphcal_compiler::ir::lower::NodeEntry),
    }

    impl DeclRef<'_> {
        const fn name(&self) -> &ScopedName {
            match self {
                Self::Param(e) => &e.name,
                Self::Node(e) => &e.name,
            }
        }

        const fn span(&self) -> Span {
            match self {
                Self::Param(e) => e.span,
                Self::Node(e) => e.span,
            }
        }
    }

    let mut graph = DiGraph::<ScopedName, ()>::new();
    let mut index_map: HashMap<ScopedName, petgraph::graph::NodeIndex> = HashMap::new();
    let mut expressions: HashMap<ScopedName, Expr> = HashMap::new();
    // Lookup used to recover source spans for cycle-error reporting without
    // re-iterating params/nodes on the error path.
    let mut span_by_name: HashMap<ScopedName, Span> = HashMap::new();

    let mut all_decls: Vec<DeclRef<'_>> = tir
        .root()
        .params
        .iter()
        .map(DeclRef::Param)
        .chain(tir.root().nodes.iter().map(DeclRef::Node))
        .collect();
    all_decls.sort_by(|a, b| a.name().cmp(b.name()));

    for decl in &all_decls {
        let name = decl.name().clone();
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        span_by_name.insert(name.clone(), decl.span());
        match decl {
            DeclRef::Param(entry) => match &entry.default_expr {
                Some(expr) => {
                    expressions.insert(name, expr.clone());
                }
                None => {
                    return Err(GraphcalError::RequiredParamNotProvided {
                        name: name.to_string(),
                        src: src.clone(),
                        span: entry.span.into(),
                    });
                }
            },
            DeclRef::Node(entry) => {
                expressions.insert(name, entry.expr.clone());
            }
        }
    }

    for (name, deps) in &tir.root().runtime_deps {
        if let Some(&to_idx) = index_map.get(name) {
            for dep in deps {
                if let Some(&from_idx) = index_map.get(dep) {
                    graph.add_edge(from_idx, to_idx, ());
                }
            }
        }
    }

    let topo_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = span_by_name
            .get(cycle_node)
            .copied()
            .unwrap_or_else(|| Span::new(0, 0));
        GraphcalError::CyclicDependency {
            name: cycle_node.to_string().into(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    let topo_order: Vec<ScopedName> = topo_indices
        .into_iter()
        .map(|idx| graph[idx].clone())
        .collect();

    Ok((topo_order, expressions))
}

/// Resolve domain constraints from type annotations on consts, params, and nodes.
///
/// Evaluates each constraint bound expression using const values and builtins
/// to obtain SI min/max scalars, validates that the target type accepts
/// constraints, and checks min <= max. Bound dimensions are validated earlier
/// in `dim_check::check_dimensions_tir`.
///
/// For const declarations, the resolved constraint is also checked against the
/// already-evaluated const value at compile time, raising `DomainViolation`
/// if the value is out of bounds.
#[expect(
    clippy::too_many_lines,
    reason = "linear iteration over domain bounds with bound eval, range, and const-value checks"
)]
pub fn resolve_domain_constraints(
    tir: &TIR,
    const_values: &HashMap<ScopedName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedDomainConstraint>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let ctx = EvalContext {
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        unfold_context: None,
        tir,
        struct_field_constraints: None,
    };

    let mut constraints = HashMap::new();

    // Iterate over consts, params, and nodes with non-empty constraints in
    // their type annotations. The `is_const` flag selects whether an immediate
    // compile-time value-vs-constraint check runs below; params/nodes defer
    // that check to `eval/runtime.rs`.
    let decl_iter = tir
        .root()
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_ann, e.span, true))
        .chain(
            tir.root()
                .params
                .iter()
                .map(|e| (&e.name, &e.type_ann, e.span, false)),
        )
        .chain(
            tir.root()
                .nodes
                .iter()
                .map(|e| (&e.name, &e.type_ann, e.span, false)),
        );

    for (name, type_ann, decl_span, is_const) in decl_iter {
        // Get constraints from the type annotation (could be on base type if indexed).
        let domain_bounds = extract_domain_bounds(type_ann);
        if domain_bounds.is_empty() {
            continue;
        }

        // Validate that the base type supports constraints.
        // (Bound dimensions are validated in `dim_check::check_dimensions_tir`.)
        let resolved = tir.root().resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        validate_constraint_target(&name.to_string(), base_resolved, decl_span, src)?;

        let mut min_val: Option<f64> = None;
        let mut max_val: Option<f64> = None;
        let mut min_display: Option<String> = None;
        let mut max_display: Option<String> = None;
        let mut constraint_span = domain_bounds[0].span;

        for bound in domain_bounds {
            // Evaluate the bound expression.
            let rv = eval_expr(&bound.value, const_values, &empty_locals, &ctx)?;

            let si_value = match &rv {
                RuntimeValue::Scalar(v) => *v,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "domain bound integers are small"
                )]
                RuntimeValue::Int(i) => *i as f64,
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "domain constraint `{}` must evaluate to a scalar value",
                            bound.kind,
                        ),
                        src: src.clone(),
                        span: bound.value.span.into(),
                    });
                }
            };

            // Format display text from the expression and pre-evaluated value.
            let display_text = format_bound_display(&bound.value, si_value);

            constraint_span = constraint_span.merge(bound.span);

            match bound.kind {
                graphcal_compiler::desugar::resolved_ast::DomainBoundKind::Min => {
                    min_val = Some(si_value);
                    min_display = Some(display_text);
                }
                graphcal_compiler::desugar::resolved_ast::DomainBoundKind::Max => {
                    max_val = Some(si_value);
                    max_display = Some(display_text);
                }
            }
        }

        // Validate min <= max.
        if let (Some(min), Some(max)) = (min_val, max_val)
            && min > max
        {
            return Err(GraphcalError::DomainMinExceedsMax {
                name: name.to_string(),
                min: min_display.unwrap_or_else(|| format!("{min}")),
                max: max_display.unwrap_or_else(|| format!("{max}")),
                src: src.clone(),
                span: constraint_span.into(),
            });
        }

        let resolved_constraint = ResolvedDomainConstraint {
            min: min_val,
            max: max_val,
            min_display,
            max_display,
            span: constraint_span,
        };

        // For const declarations, validate the (already-known) value at compile time.
        if is_const
            && let Some(value) = const_values.get(name)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(value, &resolved_constraint)
        {
            return Err(GraphcalError::DomainViolation {
                name: name.to_string(),
                value: format_runtime_value(value),
                violation: violation.message,
                src: src.clone(),
                span: decl_span.into(),
            });
        }

        constraints.insert(name.clone(), resolved_constraint);
    }

    Ok(constraints)
}

/// Resolve domain constraints declared on struct/union member fields.
///
/// For every `TypeDef` in the registry, evaluates each constrained field's
/// `min`/`max` bound expressions to SI scalars, validates `min ≤ max`, and
/// stores the result keyed by `(struct type name, field name)`.
///
/// Bound dimensions and target compatibility are validated earlier in
/// `dim_check::check_field_domain_constraint_*`. This pass focuses on the
/// runtime-relevant pieces: bound evaluation, `min ≤ max`, and storage.
pub fn resolve_struct_field_constraints(
    tir: &TIR,
    const_values: &HashMap<ScopedName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<(StructTypeName, FieldName), ResolvedDomainConstraint>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let ctx = EvalContext {
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        unfold_context: None,
        tir,
        struct_field_constraints: None,
    };

    let mut constraints = HashMap::new();

    for type_def in tir.registry.types.all_types() {
        // Walk every variant's payload fields. The constraint registry
        // is keyed by `(constructor_name, field_name)` — for the
        // single-variant record-shape this is `(Type, field)`; for a
        // true multi-variant union it's `(Variant, field)`.
        let Some(members) = type_def.union_members() else {
            continue;
        };
        for (variant, field) in members
            .iter()
            .flat_map(|m| m.fields.iter().map(move |f| (m, f)))
        {
            let domain_bounds = extract_domain_bounds(&field.type_ann);
            if domain_bounds.is_empty() {
                continue;
            }

            let mut min_val: Option<f64> = None;
            let mut max_val: Option<f64> = None;
            let mut min_display: Option<String> = None;
            let mut max_display: Option<String> = None;
            let mut constraint_span = domain_bounds[0].span;
            // Display name uses the constructor's name (matches what
            // appears in the runtime value's `type_name`).
            let display_name = format!("{}.{}", variant.name, field.name);

            for bound in domain_bounds {
                let rv = eval_expr(&bound.value, const_values, &empty_locals, &ctx)?;
                let si_value = match &rv {
                    RuntimeValue::Scalar(v) => *v,
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "domain bound integers are small"
                    )]
                    RuntimeValue::Int(i) => *i as f64,
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "domain constraint `{}` on field `{display_name}` must evaluate to a scalar value",
                                bound.kind,
                            ),
                            src: src.clone(),
                            span: bound.value.span.into(),
                        });
                    }
                };

                let display_text = format_bound_display(&bound.value, si_value);
                constraint_span = constraint_span.merge(bound.span);

                match bound.kind {
                    graphcal_compiler::desugar::resolved_ast::DomainBoundKind::Min => {
                        min_val = Some(si_value);
                        min_display = Some(display_text);
                    }
                    graphcal_compiler::desugar::resolved_ast::DomainBoundKind::Max => {
                        max_val = Some(si_value);
                        max_display = Some(display_text);
                    }
                }
            }

            if let (Some(min), Some(max)) = (min_val, max_val)
                && min > max
            {
                return Err(GraphcalError::DomainMinExceedsMax {
                    name: display_name,
                    min: min_display.unwrap_or_else(|| format!("{min}")),
                    max: max_display.unwrap_or_else(|| format!("{max}")),
                    src: src.clone(),
                    span: constraint_span.into(),
                });
            }

            constraints.insert(
                (
                    graphcal_compiler::syntax::names::StructTypeName::new(variant.name.as_str()),
                    field.name.clone(),
                ),
                ResolvedDomainConstraint {
                    min: min_val,
                    max: max_val,
                    min_display,
                    max_display,
                    span: constraint_span,
                },
            );
        }
    }

    Ok(constraints)
}

/// Walk every top-level const value and validate it against resolved
/// struct-field constraints. Used both inside [`compile`] and from the
/// `check`-only path in `eval/project/lowering.rs` so that struct-field
/// violations on const nodes surface under `graphcal check`, not only
/// during full evaluation.
///
/// # Errors
///
/// Returns the first [`GraphcalError::DomainViolation`] encountered.
pub fn check_const_struct_field_constraints_at_compile_time(
    tir: &TIR,
    const_values: &HashMap<ScopedName, RuntimeValue>,
    field_constraints: &HashMap<(StructTypeName, FieldName), ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for entry in &tir.root().consts {
        if let Some(value) = const_values.get(&entry.name) {
            check_const_struct_field_constraints(
                value,
                entry.name.member(),
                entry.span,
                field_constraints,
                src,
            )?;
        }
    }
    Ok(())
}

/// Recursively validate a const value against resolved struct-field
/// constraints. For `RuntimeValue::Struct`, looks up each field's
/// `(struct, field)` constraint and emits `DomainViolation` on the first
/// violation. Indexed values recurse element-wise; nested structs recurse
/// field-wise. Other variants short-circuit to `Ok(())`.
fn check_const_struct_field_constraints(
    value: &RuntimeValue,
    decl_name: &str,
    decl_span: Span,
    field_constraints: &HashMap<(StructTypeName, FieldName), ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match value {
        RuntimeValue::Struct { type_name, fields } => {
            for (field_name, field_value) in fields {
                let key = (type_name.clone(), field_name.clone());
                if let Some(constraint) = field_constraints.get(&key)
                    && let Err(violation) =
                        crate::domain_check::check_domain_constraint(field_value, constraint)
                {
                    return Err(GraphcalError::DomainViolation {
                        name: format!("{decl_name}.{field_name}"),
                        value: format_runtime_value(field_value),
                        violation: violation.message,
                        src: src.clone(),
                        span: decl_span.into(),
                    });
                }
                // Recurse for nested struct fields.
                check_const_struct_field_constraints(
                    field_value,
                    &format!("{decl_name}.{field_name}"),
                    decl_span,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        RuntimeValue::Indexed { entries, .. } => {
            for (variant, entry) in entries {
                check_const_struct_field_constraints(
                    entry,
                    &format!("{decl_name}.{variant}"),
                    decl_span,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Format a runtime value for inclusion in a `DomainViolation` error message.
fn format_runtime_value(rv: &RuntimeValue) -> String {
    match rv {
        RuntimeValue::Scalar(v) => graphcal_compiler::registry::format::format_number(*v),
        RuntimeValue::Int(i) => format!("{i}"),
        RuntimeValue::Indexed { entries, .. } => {
            // Show the first violating entry's value if recoverable; otherwise summary.
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{k}: {}", format_runtime_value(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// Extract `DomainBound`s from a `TypeExpr`, handling indexed types.
///
/// For `Velocity(min: 0)[Maneuver]`, the constraints are on the base `Velocity`,
/// not on the outer `Indexed` wrapper.
fn extract_domain_bounds(
    type_ann: &graphcal_compiler::desugar::resolved_ast::TypeExpr,
) -> &[graphcal_compiler::desugar::resolved_ast::DomainBound] {
    if !type_ann.constraints.is_empty() {
        return &type_ann.constraints;
    }
    // For indexed types, check the base type's constraints.
    if let graphcal_compiler::desugar::resolved_ast::TypeExprKind::Indexed { base, .. } =
        &type_ann.kind
    {
        return &base.constraints;
    }
    &[]
}

/// Strip `Indexed` wrapper to get the base resolved type.
fn strip_indexed(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> &graphcal_compiler::tir::typed::ResolvedTypeExpr {
    match resolved {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => {
            strip_indexed(base)
        }
        other => other,
    }
}

/// Validate that the resolved type supports domain constraints.
fn validate_constraint_target(
    _name: &str,
    base_resolved: Option<&graphcal_compiler::tir::typed::ResolvedTypeExpr>,
    decl_span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let Some(resolved) = base_resolved else {
        return Ok(()); // No resolved type — skip validation (will be caught elsewhere)
    };
    match resolved {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Scalar(_)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::Dimensionless
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::Int => Ok(()),
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Bool => {
            Err(GraphcalError::InvalidDomainTarget {
                type_kind: "Bool".to_string(),
                src: src.clone(),
                span: decl_span.into(),
            })
        }
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Datetime(_) => {
            Err(GraphcalError::InvalidDomainTarget {
                type_kind: "Datetime".to_string(),
                src: src.clone(),
                span: decl_span.into(),
            })
        }
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Label(idx, _) => {
            Err(GraphcalError::InvalidDomainTarget {
                type_kind: format!("Label({idx})"),
                src: src.clone(),
                span: decl_span.into(),
            })
        }
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Struct(name_s, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericStruct { name: name_s, .. } => {
            Err(GraphcalError::InvalidDomainTarget {
                type_kind: format!("struct `{name_s}`"),
                src: src.clone(),
                span: decl_span.into(),
            })
        }
        graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericDimParam(_, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericTypeParam(_, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericDimExpr { .. } => {
            // Generic types in function signatures — constraints don't apply here
            Ok(())
        }
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Indexed { .. } => {
            // Already stripped, shouldn't reach here
            Ok(())
        }
    }
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
///
/// For simple expressions (numbers, unit literals, unary negation), the
/// original syntactic form is preserved. For complex expressions, the
/// pre-evaluated SI value is displayed as a fallback — no re-evaluation needed.
fn format_bound_display(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    si_value: f64,
) -> String {
    use graphcal_compiler::desugar::resolved_ast::ExprKind;
    match &expr.kind {
        ExprKind::Number(n) => graphcal_compiler::registry::format::format_number(*n),
        ExprKind::Integer(n) => format!("{n}"),
        ExprKind::UnitLiteral { value, unit } => {
            let unit_str = graphcal_compiler::registry::format::format_unit_expr(unit);
            let val_str = graphcal_compiler::registry::format::format_number(*value);
            format!("{val_str} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::resolved_ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_bound_display(operand, -si_value))
        }
        // Fallback: display the already-evaluated SI value.
        _ => graphcal_compiler::registry::format::format_number(si_value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::type_resolve;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = graphcal_compiler::syntax::name_resolve::resolve_name_refs(desugared);
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
        let dag_id =
            graphcal_compiler::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let tir = type_resolve(ir, dag_id, &src).unwrap();
        compile(&tir, &src)
    }

    fn scalar(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Scalar(v) => *v,
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!(
            (scalar(&plan.const_values[&ScopedName::local("g0")]) - 9.80665).abs() < f64::EPSILON
        );
        assert!(plan.topo_order.is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!((scalar(&plan.const_values[&ScopedName::local("two_g0")]) - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "x")
            .unwrap();
        let y_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "y")
            .unwrap();
        let z_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "z")
            .unwrap();
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }

    #[test]
    fn compile_const_cycle() {
        let err = compile_source(
            "const node a: Dimensionless = @b + 1.0;\nconst node b: Dimensionless = @a + 1.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_runtime_cycle() {
        let err =
            compile_source("node a: Dimensionless = @b + 1.0;\nnode b: Dimensionless = @a + 1.0;")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    // -----------------------------------------------------------------------
    // Domain constraints on const nodes (#441)
    // -----------------------------------------------------------------------

    #[test]
    fn const_domain_value_within_bounds_passes() {
        compile_source("const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;").unwrap();
    }

    #[test]
    fn const_domain_value_below_min_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_value_above_max_rejected() {
        let err = compile_source("const node X: Mass(max: 10.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_min_exceeds_max_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg, max: 50.0 kg) = 75.0 kg;")
            .unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainMinExceedsMax { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_invalid_target_rejected() {
        // `Bool` is not a valid constraint target; this should now fire on consts too.
        let err = compile_source("const node FLAG: Bool(min: 0.0) = true;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::InvalidDomainTarget { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_value_within_bounds() {
        compile_source("const node N: Int(min: 1, max: 100) = 5;").unwrap();
    }

    #[test]
    fn const_domain_int_value_out_of_bounds_rejected() {
        let err = compile_source("const node N: Int(min: 1, max: 10) = 100;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }
}
