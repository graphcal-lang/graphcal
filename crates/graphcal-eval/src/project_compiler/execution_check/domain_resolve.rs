//! Domain-bound resolution and compile-time constraint validation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::declared_type::{DeclaredGenericArg, DeclaredType, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, GenericParamName};
use graphcal_compiler::tir::typed::{DagTIR, StructFieldConstraintKey, TIR};

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::{
    ResolvedDomainBound as EvaluatedDomainBound,
    ResolvedDomainBounds as EvaluatedDomainBounds, ResolvedDomainConstraint,
};
use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValue, eval_hir_expr};
use crate::execution_facts::{CheckedDagExecutionFacts, RuntimeValueMap};

use super::visible_values_with_imports;
#[cfg(test)]
use super::known_const_values;

/// Resolve domain constraints from type annotations on consts, params, and nodes.
///
/// Evaluates each compile-time bound expression using const values and builtins,
/// selects the target's quantity, integer, or datetime representation, and
/// checks `min <= max`. Bound types and target compatibility are validated
/// earlier by TIR checking.
///
/// Const constraints are also checked against their already-evaluated values.
pub(super) fn resolve_domain_constraints_for_dag(
    tir: &TIR,
    dag: &DagTIR,
    const_values: &RuntimeValueMap,
    all_const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let visible_const_values = visible_values_with_imports(dag, const_values, all_const_values);

    let ctx = EvalContext {
        cancellation: cancellation.clone(),
        work_budget: crate::eval_expr::fresh_work_budget(),
        builtin_fns,
        registry: tir.registry(),
        src,
        tir,
        current_dag: dag,
        current_decl: None,
        root_values: Some(&visible_const_values),
        root_presentation_instances: None,
        checked_execution_facts: None,
        presentation_calls: None,
        struct_field_constraints: None,
        generic_nat_bindings: None,
        host_fns: None,
    };
    let mut constraints = HashMap::new();
    let decl_iter = dag
        .consts()
        .iter()
        .map(|entry| (&entry.name, entry.span, true))
        .chain(
            dag.params()
                .iter()
                .map(|entry| (&entry.name, entry.span, false)),
        )
        .chain(
            dag.nodes()
                .iter()
                .map(|entry| (&entry.name, entry.span, false)),
        );

    for (name, decl_span, is_const) in decl_iter {
        cancellation.checkpoint()?;
        let resolved_key =
            dag.require_bound_decl_identity(name, src, DiagnosticAnchor::Source(decl_span))?;
        let Some(domain_bounds) = dag.semantic().domain_bounds.get(&resolved_key) else {
            continue;
        };
        let constraint_src = domain_bounds.first().map_or(src, |bound| &bound.src);
        let target = resolve_constraint_target(
            &name.to_string(),
            dag.resolved_decl_types().get(name).map(strip_indexed),
            decl_span,
            constraint_src,
        )?;
        let resolved_constraint = resolve_constraint_from_bounds(
            domain_bounds,
            &name.to_string(),
            target,
            &visible_const_values,
            &ctx.for_decl(&resolved_key).with_src(constraint_src),
            constraint_src,
        )?;
        let runtime_key = RuntimeDeclKey::resolved(resolved_key);
        if is_const
            && let Some(value) = const_values.get(&runtime_key)
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
        constraints.insert(runtime_key, resolved_constraint);
    }
    Ok(constraints)
}

#[derive(Debug, Clone, Copy)]
enum ConstraintTarget {
    Quantity,
    Int,
    Datetime(graphcal_compiler::registry::time_scale::TimeScale),
}

/// Evaluate a declaration's or field's stored HIR domain bounds into the
/// representation selected by its constrained value family.
fn resolve_constraint_from_bounds(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    target: ConstraintTarget,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDomainConstraint, GraphcalError> {
    match target {
        ConstraintTarget::Quantity => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Quantity(value) => Ok(*value),
                RuntimeValue::Int(value) => exact_domain_int_bound(*value, src, bound.value.span),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "a quantity",
                    other,
                    src,
                )),
            },
            |expr, value| format_quantity_bound_display(expr, *value),
        )
        .map(ResolvedDomainConstraint::quantity),
        ConstraintTarget::Int => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Int(value) => Ok(*value),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "Int",
                    other,
                    src,
                )),
            },
            |_expr, value| value.to_string(),
        )
        .map(ResolvedDomainConstraint::int),
        ConstraintTarget::Datetime(scale) => {
            let evaluated = evaluate_domain_bounds(
                bounds,
                display_name,
                values,
                ctx,
                src,
                |value, bound| match value {
                    RuntimeValue::Datetime(epoch) if epoch.time_scale == scale.to_hifitime() => {
                        Ok(*epoch)
                    }
                    other => Err(domain_bound_value_error(
                        display_name,
                        bound,
                        &format!("Datetime<{scale}>"),
                        other,
                        src,
                    )),
                },
                |_expr, epoch| epoch.to_string(),
            )?;
            ResolvedDomainConstraint::datetime(scale, evaluated).map_err(|error| {
                let anchor = bounds.first().map_or(DiagnosticAnchor::WholeFile, |bound| {
                    DiagnosticAnchor::Source(bound.span)
                });
                GraphcalError::internal_error(
                    format!(
                        "datetime domain bounds on `{display_name}` violated their checked scale invariant: {error}"
                    ),
                    src,
                    anchor,
                )
            })
        }
    }
}

fn evaluate_domain_bounds<T: PartialOrd>(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
    convert: impl Fn(
        &RuntimeValue,
        &graphcal_compiler::tir::typed::ResolvedDomainBound,
    ) -> Result<T, GraphcalError>,
    format_display: impl Fn(&graphcal_compiler::hir::Expr, &T) -> String,
) -> Result<EvaluatedDomainBounds<T>, GraphcalError> {
    let Some(first) = bounds.first() else {
        return Err(GraphcalError::internal_error(
            format!("domain constraint on `{display_name}` has no bounds"),
            src,
            DiagnosticAnchor::WholeFile,
        ));
    };
    let empty_locals = HirLocalValueMap::root();
    let evaluated = bounds
        .iter()
        .map(|bound| {
            let runtime_value = eval_hir_expr(&bound.value, values, &empty_locals, ctx)?;
            let value = convert(&runtime_value, bound)?;
            let display = format_display(&bound.value, &value);
            Ok((bound.kind, EvaluatedDomainBound::new(value, display)))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let constraint_span = bounds
        .iter()
        .skip(1)
        .fold(first.span, |span, bound| span.merge(bound.span));
    let (min, max) =
        evaluated
            .into_iter()
            .fold((None, None), |(min, max), (kind, bound)| match kind {
                graphcal_compiler::syntax::ast::DomainBoundKind::Min => (Some(bound), max),
                graphcal_compiler::syntax::ast::DomainBoundKind::Max => (min, Some(bound)),
            });
    if let (Some(min), Some(max)) = (&min, &max)
        && min.value() > max.value()
    {
        return Err(GraphcalError::DomainMinExceedsMax {
            name: display_name.to_string(),
            min: min.display().to_string(),
            max: max.display().to_string(),
            src: src.clone(),
            span: constraint_span.into(),
        });
    }
    Ok(EvaluatedDomainBounds::new(min, max))
}

fn domain_bound_value_error(
    display_name: &str,
    bound: &graphcal_compiler::tir::typed::ResolvedDomainBound,
    expected: &str,
    actual: &RuntimeValue,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: format!(
            "{} domain bound on `{display_name}` must evaluate to {expected}, got {}",
            bound.kind,
            actual.kind()
        ),
        src: src.clone(),
        span: bound.value.span.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConcreteNominalApplication {
    identity: StructTypeRef,
    generic_args: Vec<DeclaredGenericArg>,
}

fn collect_concrete_nominal_applications(
    declared: &DeclaredType,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    applications: &mut HashSet<ConcreteNominalApplication>,
) -> Result<(), GraphcalError> {
    match declared {
        DeclaredType::Struct(identity, generic_args) => {
            for arg in generic_args {
                if let DeclaredGenericArg::Type(type_arg) = arg {
                    collect_concrete_nominal_applications(type_arg, tir, src, applications)?;
                }
            }
            let application = ConcreteNominalApplication {
                identity: identity.clone(),
                generic_args: generic_args.clone(),
            };
            if !applications.insert(application) {
                return Ok(());
            }
            let model_type = graphcal_compiler::tir::dim_check::ValidatedModelType::try_new(
                tir,
                identity,
                generic_args,
                src,
            )
            .map_err(|error| error.into_graphcal_error(src))?;
            for constructor in model_type.constructors(src)? {
                for field in constructor.fields() {
                    collect_concrete_nominal_applications(
                        field.declared_type(),
                        tir,
                        src,
                        applications,
                    )?;
                }
            }
            Ok(())
        }
        DeclaredType::Indexed { element, .. } => {
            collect_concrete_nominal_applications(element, tir, src, applications)
        }
        DeclaredType::Quantity(_)
        | DeclaredType::Complex(_)
        | DeclaredType::Bool
        | DeclaredType::Int
        | DeclaredType::Datetime(_)
        | DeclaredType::IndexArg(_)
        | DeclaredType::Key(_) => Ok(()),
    }
}

fn generic_nat_bindings(
    type_def: &graphcal_compiler::hir::NominalTypeDef,
    generic_args: &[DeclaredGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<HashMap<GenericParamName, u64>, GraphcalError> {
    if type_def.generic_params().len() != generic_args.len() {
        return Err(GraphcalError::InternalError {
            message: format!(
                "concrete application of `{}` has {} generic arguments, expected {}",
                type_def.name(),
                generic_args.len(),
                type_def.generic_params().len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    type_def
        .generic_params()
        .iter()
        .zip(generic_args)
        .filter_map(|(param, arg)| match arg {
            DeclaredGenericArg::Nat(form) => Some((param, form)),
            DeclaredGenericArg::Dim(_)
            | DeclaredGenericArg::Index(_)
            | DeclaredGenericArg::Type(_) => None,
        })
        .map(|(param, form)| {
            form.constant_value()
                .map(|value| (param.name().clone(), value))
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "concrete Nat argument `{}` for `{}` remained symbolic",
                        form.format(),
                        param.name()
                    ),
                    src: src.clone(),
                    span: span.into(),
                })
        })
        .collect()
}

/// Resolve domain constraints declared on struct/union member fields.
///
/// Field bounds are stored atomically with their resolved target types in each
/// DAG's semantic type defs; this evaluates each constrained field's
/// typed `min`/`max` bounds, validates `min ≤ max`, and stores the result keyed
/// by the canonical owning struct type, constructor, and field name. Concrete
/// include instances already receive their own canonical owner during lowering;
/// no root-owned display alias is fabricated here.
///
/// Bound types and target compatibility are validated earlier by the compiler's
/// field-domain checks. This pass focuses on the runtime-relevant pieces: bound
/// evaluation, `min ≤ max`, and storage.
#[cfg(test)]
pub(crate) fn resolve_struct_field_constraints(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    resolve_struct_field_constraints_with_cancellation(
        tir,
        const_values,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

struct FieldConstraintResolutionContext<'a> {
    tir: &'a TIR,
    dag_facts:
        &'a HashMap<graphcal_compiler::dag_id::DagId, Arc<CheckedDagExecutionFacts>>,
    all_const_values: &'a RuntimeValueMap,
    builtin_fns: &'a graphcal_compiler::registry::builtins::BuiltinFunctions,
    fallback_src: &'a NamedSource<Arc<String>>,
    cancellation: &'a graphcal_compiler::cancellation::CancellationToken,
}

type DagFieldConstraints = HashMap<
    graphcal_compiler::dag_id::DagId,
    HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
>;
type ApplicationFieldConstraints = (
    graphcal_compiler::dag_id::DagId,
    Vec<(StructFieldConstraintKey, ResolvedDomainConstraint)>,
);

fn application_field_constraint_key(
    application: &ConcreteNominalApplication,
    key: &graphcal_compiler::tir::typed::ResolvedStructFieldTypeKey,
) -> StructFieldConstraintKey {
    StructFieldConstraintKey::for_application(
        StructTypeRef::from_resolved(key.owning_type.clone()),
        application.generic_args.clone(),
        key.constructor.clone(),
        key.field.clone(),
    )
}

fn resolve_application_field_constraints(
    application: &ConcreteNominalApplication,
    ctx: &FieldConstraintResolutionContext<'_>,
) -> Result<ApplicationFieldConstraints, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    let dag_id = application.identity.resolved().owner();
    let dag = ctx.tir.dag_registry().get(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("type owner `{dag_id}` has no checked DAG"),
            ctx.fallback_src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let type_def = dag
        .semantic()
        .type_defs
        .struct_types
        .get(application.identity.resolved())
        .ok_or_else(|| {
            GraphcalError::internal_error(
                format!(
                    "semantic type metadata missing concrete application `{}`",
                    application.identity
                ),
                ctx.fallback_src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let facts = ctx.dag_facts.get(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("type owner `{dag_id}` has no checked execution facts"),
            ctx.fallback_src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let owner_src = facts.source();
    let visible_const_values =
        visible_values_with_imports(dag, &facts.const_values, ctx.all_const_values);
    let nat_bindings = generic_nat_bindings(
        type_def,
        &application.generic_args,
        type_def.source(),
        type_def.span(),
    )?;
    let application_ctx = EvalContext {
        cancellation: ctx.cancellation.clone(),
        work_budget: crate::eval_expr::fresh_work_budget(),
        builtin_fns: ctx.builtin_fns,
        registry: ctx.tir.registry(),
        src: owner_src,
        tir: ctx.tir,
        current_dag: dag,
        current_decl: None,
        root_values: Some(&visible_const_values),
        root_presentation_instances: None,
        checked_execution_facts: None,
        presentation_calls: None,
        struct_field_constraints: None,
        generic_nat_bindings: Some(&nat_bindings),
        host_fns: None,
    };
    let mut constraints = Vec::new();
    for (key, field_semantics) in dag
        .semantic()
        .type_defs
        .constrained_fields()
        .filter(|(key, _)| key.owning_type == *application.identity.resolved())
    {
        let display_name = format!("{}.{}", key.constructor, key.field);
        let bounds = field_semantics.domain_bounds();
        let Some(first_bound) = bounds.first() else {
            return Err(GraphcalError::internal_error(
                format!("constrained field `{display_name}` has no domain bounds"),
                type_def.source(),
                DiagnosticAnchor::Source(type_def.span()),
            ));
        };
        let bound_span = first_bound.span;
        let constraint_src = &first_bound.src;
        let target = resolve_constraint_target(
            &display_name,
            Some(strip_indexed(field_semantics.resolved_type())),
            bound_span,
            constraint_src,
        )?;
        let constraint = resolve_constraint_from_bounds(
            bounds,
            &display_name,
            target,
            &visible_const_values,
            &application_ctx.with_src(constraint_src),
            constraint_src,
        )?;
        constraints.push((
            application_field_constraint_key(application, key),
            constraint,
        ));
    }
    Ok((dag_id.clone(), constraints))
}

fn collect_field_constraint_applications(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<ConcreteNominalApplication>, GraphcalError> {
    let mut applications = HashSet::new();
    for declared in tir.build_declared_types(src)?.values() {
        collect_concrete_nominal_applications(declared, tir, src, &mut applications)?;
    }
    for dag in tir.dag_registry().values() {
        for application in
            graphcal_compiler::tir::dim_check::concrete_constructor_applications(tir, dag, src)?
        {
            applications.insert(ConcreteNominalApplication {
                identity: application.identity().clone(),
                generic_args: application.generic_args().to_vec(),
            });
        }
        for (identity, type_def) in &dag.semantic().type_defs.struct_types {
            if type_def.generic_params().is_empty() {
                applications.insert(ConcreteNominalApplication {
                    identity: StructTypeRef::from_resolved(identity.clone()),
                    generic_args: Vec::new(),
                });
            }
        }
    }
    Ok(applications)
}

pub(super) fn resolve_struct_field_constraints_for_dags(
    tir: &TIR,
    dag_facts: &HashMap<graphcal_compiler::dag_id::DagId, Arc<CheckedDagExecutionFacts>>,
    all_const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<DagFieldConstraints, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let context = FieldConstraintResolutionContext {
        tir,
        dag_facts,
        all_const_values,
        builtin_fns,
        fallback_src: src,
        cancellation,
    };
    let mut grouped = DagFieldConstraints::new();
    for application in collect_field_constraint_applications(tir, src)? {
        let (owner, entries) = resolve_application_field_constraints(&application, &context)?;
        grouped.entry(owner).or_default().extend(entries);
    }
    Ok(grouped)
}

#[cfg(test)]
pub(super) fn resolve_struct_field_constraints_with_cancellation(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    let dag_facts = tir
        .dag_registry()
        .keys()
        .map(|dag_id| {
            let values = if dag_id == tir.root_dag_id() {
                const_values.clone()
            } else {
                RuntimeValueMap::new()
            };
            (
                dag_id.clone(),
                Arc::new(CheckedDagExecutionFacts {
                    dag_id: dag_id.clone(),
                    source: src.clone(),
                    const_values: Arc::new(values),
                    topo_order: Arc::new(Vec::new()),
                    domain_constraints: Arc::new(HashMap::new()),
                    struct_field_constraints: Arc::new(HashMap::new()),
                }),
            )
        })
        .collect::<HashMap<_, _>>();
    let all_const_values = known_const_values(tir, &dag_facts);
    resolve_struct_field_constraints_for_dags(tir, &dag_facts, &all_const_values, src, cancellation)
        .map(|grouped| grouped.into_values().flatten().collect())
}

pub(super) fn check_dag_const_struct_field_constraints_at_compile_time(
    dag: &DagTIR,
    const_values: &RuntimeValueMap,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for entry in dag.consts() {
        let key = RuntimeDeclKey::resolved(dag.require_bound_decl_identity(
            &entry.name,
            src,
            DiagnosticAnchor::Source(entry.span),
        )?);
        if let Some(value) = const_values.get(&key) {
            let owning_type = dag
                .resolved_decl_types()
                .get(&entry.name)
                .and_then(struct_type_ref_from_resolved_type);
            check_const_struct_field_constraints(
                value,
                entry.name.member().as_str(),
                entry.span,
                owning_type.as_ref(),
                field_constraints,
                src,
            )?;
        }
    }
    Ok(())
}

fn struct_type_ref_from_resolved_type(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> Option<StructTypeRef> {
    match strip_indexed(resolved) {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Struct(name, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericStruct { name, .. } => {
            Some(StructTypeRef::from_resolved(name.clone()))
        }
        _ => None,
    }
}

fn find_struct_field_constraint<'a>(
    field_constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    generic_args: &[graphcal_compiler::registry::declared_type::DeclaredGenericArg],
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        field_constraints.get(&StructFieldConstraintKey::for_application(
            owning_type.clone(),
            generic_args.to_vec(),
            constructor.clone(),
            field.clone(),
        ))
    })
}

/// Recursively validate a const value against resolved struct-field
/// constraints. For `RuntimeValue::Struct`, looks up each field's
/// owner-qualified struct/constructor/field constraint and emits
/// `DomainViolation` on the first violation. Indexed values recurse
/// element-wise; nested structs recurse field-wise. Other variants short-circuit
/// to `Ok(())`.
fn check_const_struct_field_constraints(
    value: &RuntimeValue,
    decl_name: &str,
    decl_span: Span,
    owning_type: Option<&StructTypeRef>,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match value {
        RuntimeValue::Struct {
            type_name,
            generic_args,
            fields,
        } => {
            let runtime_owning_type = StructTypeRef::from_resolved(type_name.resolved().clone());
            let effective_owning_type = owning_type.or(Some(&runtime_owning_type));
            let constructor = ConstructorName::from_atom(type_name.name().atom().clone());
            for (field_name, field_value) in fields {
                if let Some(constraint) = find_struct_field_constraint(
                    field_constraints,
                    effective_owning_type,
                    generic_args,
                    &constructor,
                    field_name,
                ) && let Err(violation) =
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
                // Recurse for nested struct fields. The nested runtime value
                // carries its canonical owner when module-aware constructor
                // evaluation created it, so the recursive call can recover the
                // owner even without a field-declared type side channel.
                check_const_struct_field_constraints(
                    field_value,
                    &format!("{decl_name}.{field_name}"),
                    decl_span,
                    None,
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
                    owning_type,
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
        RuntimeValue::Quantity(v) => graphcal_compiler::registry::format::format_number(*v),
        RuntimeValue::Int(i) => format!("{i}"),
        RuntimeValue::Datetime(epoch) => epoch.to_string(),
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

/// Resolve the typed constraint family selected by a declaration or field type.
fn resolve_constraint_target(
    name: &str,
    base_resolved: Option<&graphcal_compiler::tir::typed::ResolvedTypeExpr>,
    decl_span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<ConstraintTarget, GraphcalError> {
    use graphcal_compiler::tir::typed::ResolvedTypeExpr;

    let Some(resolved) = base_resolved else {
        return Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` has no resolved type"),
            src: src.clone(),
            span: decl_span.into(),
        });
    };
    match resolved {
        ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => Ok(ConstraintTarget::Quantity),
        ResolvedTypeExpr::Int => Ok(ConstraintTarget::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(ConstraintTarget::Datetime(*scale)),
        ResolvedTypeExpr::Bool => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Bool".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Complex { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Complex".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Key { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Key".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::IndexArg(index) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("index {}", index.format_for_diagnostic()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Struct(struct_name, _)
        | ResolvedTypeExpr::GenericStruct {
            name: struct_name, ..
        } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("struct `{}`", struct_name.as_str()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::GenericTypeParam(param, _) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("generic Type parameter `{param}`"),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Indexed { .. } => Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` was not reduced to its base type"),
            src: src.clone(),
            span: decl_span.into(),
        }),
    }
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
///
/// For simple expressions (numbers, quantity literals, unary negation), the
/// original syntactic form is preserved. For complex expressions, the
/// pre-evaluated SI value is displayed as a fallback — no re-evaluation needed.
fn exact_domain_int_bound(
    value: i64,
    src: &NamedSource<Arc<String>>,
    span: graphcal_compiler::syntax::span::Span,
) -> Result<f64, GraphcalError> {
    crate::eval_expr::numeric::exact_i64_to_f64(value).map_err(|_| GraphcalError::EvalError {
        message: format!("domain bound integer {value} is too large for exact quantity comparison"),
        src: src.clone(),
        span: span.into(),
    })
}

fn format_quantity_bound_display(expr: &graphcal_compiler::hir::Expr, si_value: f64) -> String {
    use graphcal_compiler::hir::ExprKind;
    match &expr.kind {
        ExprKind::Number(n) => graphcal_compiler::registry::format::format_number(*n),
        ExprKind::Integer(n) => format!("{n}"),
        ExprKind::QuantityLiteral { value, unit } => {
            let unit_str = graphcal_compiler::registry::format::format_unit_terms_with_config(
                unit.terms
                    .iter()
                    .map(|item| (item.op, item.name.value.to_string(), item.power)),
                true,
            );
            let val_str = graphcal_compiler::registry::format::format_number(*value);
            format!("{val_str} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_quantity_bound_display(operand, -si_value))
        }
        // Fallback: display the already-evaluated SI value.
        _ => graphcal_compiler::registry::format::format_number(si_value),
    }
}

