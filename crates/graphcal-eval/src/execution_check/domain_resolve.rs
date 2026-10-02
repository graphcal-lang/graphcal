//! Domain-bound resolution and compile-time constraint validation.

use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::semantic_error::domain::{
    DomainBoundSpelling, DomainError, DomainSubject, ValuePath, ValuePathStep,
};
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::DomainFamily;
use std::collections::{HashMap, HashSet};

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic::checked_type::{CheckedGenericArg, CheckedType, StructTypeRef};
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use graphcal_compiler::tir::typed::{
    CheckedDag, CheckedTir, DeclarationBody, ResolvedDeclType, ResolvedValueType, Scoped,
    StructFieldConstraintKey,
};

use crate::constant_pools::RuntimeValueMap;
use crate::domain_constraint::{
    DomainInstant, ResolvedDomainBound as EvaluatedDomainBound,
    ResolvedDomainBounds as EvaluatedDomainBounds, ResolvedDomainConstraint,
};
use crate::eval_expr::{EvalSession, RuntimeValue, eval_root};
use crate::invariant::Failure;
use graphcal_compiler::resolved_name::ResolvedDeclName;

/// Resolve domain constraints from type annotations on consts, params, and nodes.
///
/// Evaluates each compile-time bound expression using const values and builtins,
/// selects the target's quantity, integer, or datetime representation, and
/// checks `min <= max`. Bound types and target compatibility are validated
/// earlier by TIR checking.
///
/// Const constraints are also checked against their already-evaluated values.
pub(super) fn resolve_domain_constraints_for_dag(
    tir: &CheckedTir,
    dag: &CheckedDag,
    const_values: &RuntimeValueMap,
    all_const_values: &RuntimeValueMap,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<ResolvedDeclName, ResolvedDomainConstraint>, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let visible_const_values = visible_values_with_imports(const_values, all_const_values);

    let ctx = EvalSession::provisional_constants(tir, src, sources, cancellation.clone());
    let mut constraints = HashMap::new();
    // Constants first, then parameters, then nodes, each in source order.
    let decl_iter = [
        ValueDeclCategory::Const,
        ValueDeclCategory::Param,
        ValueDeclCategory::Node,
    ]
    .into_iter()
    .flat_map(|category| {
        dag.declarations().filter_map(move |entry| {
            entry
                .value()
                .filter(|value| value.category == category)
                .map(|value| {
                    (
                        entry.name(),
                        entry.identity().clone(),
                        value.annotation,
                        value.span,
                        category == ValueDeclCategory::Const,
                    )
                })
        })
    });

    for (name, resolved_key, annotation, decl_span, is_const) in decl_iter {
        cancellation.checkpoint()?;
        let Some(domain_bounds) = tir
            .declaration_body(&resolved_key)
            .and_then(DeclarationBody::domain_bounds)
        else {
            continue;
        };
        let constraint_src = domain_bounds.get().first().src;
        let target = resolve_constraint_target(
            annotation.checked().resolved().element(),
            decl_span,
            constraint_src,
        )?;
        let resolved_constraint = resolve_constraint_from_bounds(
            domain_bounds,
            &DomainSubject::Declaration(name.clone()),
            target,
            &visible_const_values,
            BoundCheckingContext {
                evaluation: &ctx.for_decl(&resolved_key).with_src(constraint_src),
                bindings: &HashMap::new(),
            },
            constraint_src,
        )?;
        if is_const
            && let Some(value) = const_values.get(&resolved_key)
            && let Err(failure) =
                crate::domain_check::check_domain_constraint(value, &resolved_constraint)
        {
            return Err(domain_check_error(
                failure,
                DomainSubject::Declaration(name.clone()),
                value,
                src,
                decl_span,
            )
            .into());
        }
        constraints.insert(resolved_key, resolved_constraint);
    }
    Ok(constraints)
}

/// Checking-only substitution services never enter the interpreter context.
#[derive(Clone, Copy)]
struct BoundCheckingContext<'a, 'b> {
    evaluation: &'a EvalSession<'b>,
    bindings: &'a HashMap<graphcal_compiler::hir::types::GenericParamId, u64>,
}

/// Evaluate a declaration's or field's stored HIR domain bounds into the
/// representation selected by its constrained value family.
fn resolve_constraint_from_bounds(
    bounds: Scoped<'_, NonEmpty<graphcal_compiler::tir::typed::ResolvedDomainBound>>,
    display_name: &DomainSubject,
    target: DomainFamily,
    values: &RuntimeValueMap,
    ctx: BoundCheckingContext<'_, '_>,
    src: SourceId,
) -> Result<ResolvedDomainConstraint, Outcome<SemanticError>> {
    match target {
        DomainFamily::Quantity => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            &"a quantity",
            |value, bound| {
                let quantity = match value {
                    RuntimeValue::Quantity(value) => *value,
                    RuntimeValue::Int(value) => {
                        match FiniteQuantity::try_new(
                            exact_domain_int_bound(*value, src, bound.value.span)
                                .map_err(Outcome::Failed)?,
                        ) {
                            Ok(quantity) => quantity,
                            Err(_) => return Ok(None),
                        }
                    }
                    _ => return Ok(None),
                };
                Ok(Some((
                    quantity,
                    quantity_bound_spelling(&bound.value, quantity.get()),
                )))
            },
        )
        .map(ResolvedDomainConstraint::quantity),
        DomainFamily::Int => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            &"Int",
            |value, _| match value {
                RuntimeValue::Int(value) => {
                    Ok(Some((*value, DomainBoundSpelling::Integer(*value))))
                }
                _ => Ok(None),
            },
        )
        .map(ResolvedDomainConstraint::int),
        DomainFamily::Datetime(scale) => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            &format_args!("Datetime<{scale}>"),
            // Each bound is admitted as an instant only from an epoch in the
            // constrained scale; its spelling keeps the epoch.
            |value, _| match value {
                RuntimeValue::Datetime(epoch) => Ok(DomainInstant::from_epoch(*epoch, scale)
                    .ok()
                    .map(|instant| (instant, DomainBoundSpelling::Datetime(*epoch)))),
                _ => Ok(None),
            },
        )
        .map(|bounds| ResolvedDomainConstraint::datetime(scale, bounds)),
    }
}

/// Evaluate each bound and read it with `read`, which returns the bound's
/// value and its spelling, or `None` for a value that is not `expected`:
/// checking proved every bound of the constrained family, so such a value is
/// a violated invariant.
fn evaluate_domain_bounds<T: PartialOrd>(
    scoped_bounds: Scoped<'_, NonEmpty<graphcal_compiler::tir::typed::ResolvedDomainBound>>,
    display_name: &DomainSubject,
    values: &RuntimeValueMap,
    ctx: BoundCheckingContext<'_, '_>,
    src: SourceId,
    expected: &dyn std::fmt::Display,
    read: impl Fn(
        &RuntimeValue,
        &graphcal_compiler::tir::typed::ResolvedDomainBound,
    ) -> Result<Option<(T, DomainBoundSpelling)>, Outcome<SemanticError>>,
) -> Result<EvaluatedDomainBounds<T>, Outcome<SemanticError>> {
    let (first, rest) = scoped_bounds.get().split_first();
    let evaluated = scoped_bounds
        .map(NonEmpty::as_slice)
        .iter()
        .map(|scoped_bound| {
            let bound = scoped_bound.get();
            let tree = graphcal_compiler::tir::dim_check::body_specialization::specialize_bound_expression(
                ctx.evaluation.tir, scoped_bound, ctx.bindings,
            )?;
            let runtime_value = eval_root(&tree, values, ctx.evaluation)?;
            let (value, display) = read(&runtime_value, bound)?.ok_or_else(|| {
                SemanticError::internal_error(
                    format!(
                        "{} domain bound on `{display_name}` checked as {expected} evaluated to {}",
                        bound.kind,
                        runtime_value.describe()
                    ),
                    src,
                    DiagnosticAnchor::Source(bound.value.span),
                )
            })?;
            Ok((bound.kind, EvaluatedDomainBound::new(value, display)))
        })
        .collect::<Result<Vec<_>, Outcome<SemanticError>>>()?;
    let constraint_span = rest
        .iter()
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
        return Err(SemanticError::located(
            src,
            constraint_span,
            DomainError::DomainMinExceedsMax {
                name: display_name.clone(),
                min: Box::new(min.display().clone()),
                max: Box::new(max.display().clone()),
            },
        )
        .into());
    }
    Ok(EvaluatedDomainBounds::new(min, max))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConcreteNominalApplication {
    identity: StructTypeRef,
    generic_args: Vec<CheckedGenericArg>,
}

fn collect_concrete_nominal_applications(
    declared: &CheckedType,
    tir: &CheckedTir,
    src: SourceId,
    applications: &mut HashSet<ConcreteNominalApplication>,
) -> Result<(), SemanticError> {
    match declared {
        CheckedType::Struct(identity, generic_args) => {
            for arg in generic_args {
                if let CheckedGenericArg::Type(type_arg) = arg {
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
            .map_err(|error| error.into_semantic_error(src))?;
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
        CheckedType::Indexed { element, .. } => {
            collect_concrete_nominal_applications(element, tir, src, applications)
        }
        CheckedType::Quantity(_)
        | CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Int
        | CheckedType::Datetime(_)
        | CheckedType::Key(_) => Ok(()),
    }
}

fn generic_nat_bindings(
    type_def: &graphcal_compiler::hir::nominal::NominalTypeDef,
    generic_args: &[CheckedGenericArg],
    src: SourceId,
    span: Span,
) -> Result<HashMap<graphcal_compiler::hir::types::GenericParamId, u64>, SemanticError> {
    if type_def.generic_params().len() != generic_args.len() {
        return Err(SemanticError::internal_error(
            format!(
                "concrete application of `{}` has {} generic arguments, expected {}",
                type_def.name(),
                generic_args.len(),
                type_def.generic_params().len()
            ),
            src,
            graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(span),
        ));
    }
    type_def
        .generic_params()
        .iter()
        .zip(generic_args)
        .filter_map(|(param, arg)| match arg {
            CheckedGenericArg::Nat(form) => Some((param, form)),
            CheckedGenericArg::Dim(_)
            | CheckedGenericArg::Index(_)
            | CheckedGenericArg::Type(_) => None,
        })
        .map(|(param, value)| Ok((param.id().clone(), *value)))
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
#[cfg(any(test, feature = "test-internals"))]
pub(super) fn resolve_struct_field_constraints(
    tir: &CheckedTir,
    const_values: &RuntimeValueMap,
    src: SourceId,
    sources: &SourceRegistry,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, SemanticError> {
    graphcal_compiler::outcome::without_cancellation(|cancellation| {
        resolve_struct_field_constraints_with_cancellation(
            tir,
            const_values,
            src,
            sources,
            cancellation,
        )
    })
}

/// Evaluated constants whose field constraints are still being resolved.
/// This is deliberately not an executable/checked DAG artifact.
pub(super) struct DagConstScope<'a> {
    pub values: &'a RuntimeValueMap,
    pub source: SourceId,
}

/// The constant scope of every DAG of one checked TIR, at the DAG's position
/// in its registry.
pub(super) struct ConstScopes<'a> {
    by_position: Vec<DagConstScope<'a>>,
}

impl<'a> ConstScopes<'a> {
    /// The scope `scope` gives each DAG of `tir`, in position order.
    pub(super) fn new(
        tir: &'a CheckedTir,
        mut scope: impl FnMut(
            graphcal_compiler::tir::typed::dag_position::DagPosition,
            &'a CheckedDag,
        ) -> DagConstScope<'a>,
    ) -> Self {
        Self {
            by_position: tir
                .dag_registry()
                .positioned()
                .map(|(position, dag)| scope(position, dag))
                .collect(),
        }
    }

    /// The constant scope of the DAG at `position` of the TIR the scopes
    /// were made for.
    fn at(
        &self,
        position: graphcal_compiler::tir::typed::dag_position::DagPosition,
    ) -> &DagConstScope<'a> {
        &self.by_position[position.index()]
    }
}

struct FieldConstraintResolutionContext<'a> {
    tir: &'a CheckedTir,
    const_scopes: &'a ConstScopes<'a>,
    all_const_values: &'a RuntimeValueMap,
    fallback_src: SourceId,
    sources: &'a SourceRegistry,
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
    field: graphcal_compiler::tir::typed::NominalFieldSemantics<'_>,
) -> StructFieldConstraintKey {
    StructFieldConstraintKey::for_application(
        StructTypeRef::from_resolved(field.member().nominal().identity().clone()),
        application.generic_args.clone(),
        field.member().constructor().name(),
        field.field().name().clone(),
    )
}

fn resolve_application_field_constraints(
    application: &ConcreteNominalApplication,
    ctx: &FieldConstraintResolutionContext<'_>,
) -> Result<ApplicationFieldConstraints, Outcome<SemanticError>> {
    ctx.cancellation.checkpoint()?;
    let dag_id = application.identity.resolved().owner();
    let nominal = ctx
        .tir
        .nominal_type_body(application.identity.resolved())
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "semantic type metadata missing concrete application `{}`",
                    application.identity
                ),
                ctx.fallback_src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let type_def = nominal.definition();
    let constants = ctx.const_scopes.at(nominal.position());
    let owner_src = constants.source;
    let visible_const_values = visible_values_with_imports(constants.values, ctx.all_const_values);
    let nat_bindings = generic_nat_bindings(
        type_def,
        &application.generic_args,
        type_def.source(),
        type_def.span(),
    )?;
    let application_ctx = EvalSession::provisional_constants(
        ctx.tir,
        owner_src,
        ctx.sources,
        ctx.cancellation.clone(),
    );
    let mut constraints = Vec::new();
    for (constrained, bounds) in nominal.constrained_fields() {
        let field = constrained.field();
        let field_semantics = field.semantics();
        let display_name = DomainSubject::ConstructorField(Box::new((
            field.member().constructor().name().clone(),
            field.field().name().clone(),
        )));
        let first_bound = bounds.get().first();
        let bound_span = first_bound.span;
        let constraint_src = &first_bound.src;
        let target = resolve_constraint_target(
            field_semantics.resolved_type().element(),
            bound_span,
            *constraint_src,
        )?;
        let constraint = resolve_constraint_from_bounds(
            bounds,
            &display_name,
            target,
            &visible_const_values,
            BoundCheckingContext {
                evaluation: &application_ctx.with_src(*constraint_src),
                bindings: &nat_bindings,
            },
            *constraint_src,
        )?;
        constraints.push((
            application_field_constraint_key(application, field),
            constraint,
        ));
    }
    Ok((dag_id.clone(), constraints))
}

fn collect_field_constraint_applications(
    tir: &CheckedTir,
    src: SourceId,
) -> Result<HashSet<ConcreteNominalApplication>, SemanticError> {
    let mut applications = HashSet::new();
    // The entry DAG's own declarations and the imported values visible in it.
    let root = tir.root();
    for declared in root
        .value_decl_types()
        .map(|(_, annotation)| annotation.checked().declared())
        .chain(
            root.imported_bindings()
                .values()
                .map(graphcal_compiler::ir::imported_binding::ImportedBinding::declared_type),
        )
    {
        collect_concrete_nominal_applications(declared, tir, src, &mut applications)?;
    }
    for dag in tir.dag_registry().values() {
        for (definition, generic_args) in dag.concrete_constructor_applications() {
            applications.insert(ConcreteNominalApplication {
                identity: StructTypeRef::from_resolved(definition.clone()),
                generic_args,
            });
        }
        for nominal in dag.nominal_types() {
            if nominal.definition().generic_params().is_empty() {
                applications.insert(ConcreteNominalApplication {
                    identity: StructTypeRef::from_resolved(nominal.identity().clone()),
                    generic_args: Vec::new(),
                });
            }
        }
    }
    Ok(applications)
}

pub(super) fn resolve_struct_field_constraints_for_dags(
    tir: &CheckedTir,
    const_scopes: &ConstScopes<'_>,
    all_const_values: &RuntimeValueMap,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<DagFieldConstraints, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let context = FieldConstraintResolutionContext {
        tir,
        const_scopes,
        all_const_values,
        fallback_src: src,
        sources,
        cancellation,
    };
    let mut grouped = DagFieldConstraints::new();
    for application in collect_field_constraint_applications(tir, src)? {
        let (owner, entries) = resolve_application_field_constraints(&application, &context)?;
        grouped.entry(owner).or_default().extend(entries);
    }
    Ok(grouped)
}

#[cfg(any(test, feature = "test-internals"))]
pub(super) fn resolve_struct_field_constraints_with_cancellation(
    tir: &CheckedTir,
    const_values: &RuntimeValueMap,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, Outcome<SemanticError>> {
    let empty = RuntimeValueMap::new();
    let const_scopes = ConstScopes::new(tir, |_, dag| DagConstScope {
        values: if dag.dag_id() == tir.root_dag_id() {
            const_values
        } else {
            &empty
        },
        source: src,
    });
    let all_const_values = visible_values_with_imports(const_values, &empty);
    resolve_struct_field_constraints_for_dags(
        tir,
        &const_scopes,
        &all_const_values,
        src,
        sources,
        cancellation,
    )
    .map(|grouped| grouped.into_values().flatten().collect())
}

pub(super) fn check_dag_const_struct_field_constraints_at_compile_time(
    dag: &CheckedDag,
    const_values: &RuntimeValueMap,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: SourceId,
) -> Result<(), SemanticError> {
    for (entry, declared) in dag.declarations().filter_map(|entry| {
        entry
            .value()
            .filter(|value| value.category == ValueDeclCategory::Const)
            .map(|declared| (entry, declared))
    }) {
        let key = entry.identity().clone();
        let value = const_values.get(&key).ok_or_else(|| {
            SemanticError::internal_error(
                format!("checked constant `{key}` has no evaluated value"),
                src,
                DiagnosticAnchor::Source(declared.span),
            )
        })?;
        let owning_type =
            struct_type_ref_from_resolved_type(declared.annotation.checked().resolved());
        check_const_struct_field_constraints(
            value,
            &ValuePath::new(entry.name().clone()),
            declared.span,
            owning_type.as_ref(),
            field_constraints,
            src,
        )?;
    }
    Ok(())
}

fn struct_type_ref_from_resolved_type(resolved: &ResolvedDeclType) -> Option<StructTypeRef> {
    match resolved.element() {
        ResolvedValueType::Struct { name, .. } => Some(StructTypeRef::from_resolved(name.clone())),
        _ => None,
    }
}

fn find_struct_field_constraint<'a>(
    field_constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    generic_args: &[graphcal_compiler::semantic::checked_type::CheckedGenericArg],
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
    path: &ValuePath,
    decl_span: Span,
    owning_type: Option<&StructTypeRef>,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: SourceId,
) -> Result<(), SemanticError> {
    match value {
        RuntimeValue::Struct(value) => {
            let runtime_owning_type = StructTypeRef::from_resolved(value.type_name().clone());
            let effective_owning_type = owning_type.or(Some(&runtime_owning_type));
            for (field_name, field_value) in value.fields() {
                if let Some(constraint) = find_struct_field_constraint(
                    field_constraints,
                    effective_owning_type,
                    value.generic_args(),
                    value.constructor(),
                    field_name,
                ) && let Err(failure) =
                    crate::domain_check::check_domain_constraint(field_value, constraint)
                {
                    return Err(domain_check_error(
                        failure,
                        DomainSubject::Value(Box::new(
                            path.child(ValuePathStep::Field(field_name.clone())),
                        )),
                        field_value,
                        src,
                        decl_span,
                    ));
                }
                // Recurse for nested struct fields. The nested runtime value
                // carries its canonical owner when module-aware constructor
                // evaluation created it, so the recursive call can recover the
                // owner even without a field-declared type side channel.
                check_const_struct_field_constraints(
                    field_value,
                    &path.child(ValuePathStep::Field(field_name.clone())),
                    decl_span,
                    None,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        RuntimeValue::Indexed(entries) => {
            for (variant, entry) in entries.iter() {
                check_const_struct_field_constraints(
                    entry,
                    &path.child(ValuePathStep::Entry(variant.clone())),
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

/// The diagnostic for a constant `value` of `subject` that failed its domain
/// check at `span`: a violation is a domain error, a value of another family
/// than its constraint's a violated invariant.
fn domain_check_error(
    failure: Failure<crate::domain_check::DomainViolation>,
    subject: DomainSubject,
    value: &RuntimeValue,
    src: SourceId,
    span: Span,
) -> SemanticError {
    match failure {
        Failure::Error(violation) => SemanticError::located(
            src,
            span,
            DomainError::DomainViolation {
                name: subject,
                value: format_runtime_value(value),
                violation: Box::new(violation),
            },
        ),
        Failure::Invariant(invariant) => invariant.into_internal_error(src),
    }
}

/// Format a runtime value for inclusion in a `DomainViolation` error message.
fn format_runtime_value(rv: &RuntimeValue) -> String {
    match rv {
        RuntimeValue::Quantity(v) => graphcal_compiler::display::number::format_number(v.get()),
        RuntimeValue::Int(i) => format!("{i}"),
        RuntimeValue::Datetime(epoch) => epoch.to_string(),
        RuntimeValue::Indexed(entries) => {
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

/// Resolve the typed constraint family selected by a declaration or field type.
fn resolve_constraint_target(
    resolved: &ResolvedValueType,
    decl_span: Span,
    src: SourceId,
) -> Result<DomainFamily, SemanticError> {
    resolved.domain_family().map_err(|type_kind| {
        SemanticError::located(
            src,
            decl_span,
            DomainError::InvalidDomainTarget { type_kind },
        )
    })
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
///
/// For simple expressions (numbers, quantity literals, unary negation), the
/// original syntactic form is preserved. For complex expressions, the
/// pre-evaluated SI value is displayed as a fallback — no re-evaluation needed.
fn exact_domain_int_bound(
    value: i64,
    src: SourceId,
    span: graphcal_compiler::syntax::span::Span,
) -> Result<f64, SemanticError> {
    crate::eval_expr::numeric::exact_i64_to_f64(value).map_err(|_| {
        SemanticError::located(src, span, DomainError::InexactIntDomainBound { value })
    })
}

/// How a quantity bound is spelled: as written when it is a literal, by its
/// SI value otherwise.
fn quantity_bound_spelling(
    expr: &graphcal_compiler::hir::expr::Expr,
    si_value: f64,
) -> DomainBoundSpelling {
    use graphcal_compiler::hir::expr::ExprKind;
    match expr.kind() {
        ExprKind::Number(n) => DomainBoundSpelling::Number(*n),
        ExprKind::Integer(n) => DomainBoundSpelling::Integer(*n),
        ExprKind::QuantityLiteral { value, unit } => DomainBoundSpelling::Quantity {
            value: *value,
            unit: unit.clone(),
        },
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => DomainBoundSpelling::Negated(Box::new(quantity_bound_spelling(operand, -si_value))),
        // Fallback: display the already-evaluated SI value.
        _ => DomainBoundSpelling::Number(si_value),
    }
}

/// Local constants shadow the constants visible from every checked DAG.
fn visible_values_with_imports(
    local_const_values: &RuntimeValueMap,
    known_const_values: &RuntimeValueMap,
) -> RuntimeValueMap {
    let mut values = known_const_values.clone();
    values.extend(
        local_const_values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    values
}
