use crate::runtime_value::{IndexAxis, IndexedValue, KeyValue, RuntimeValue};
use graphcal_compiler::builtin::{AggregationFn, KeyAggregation};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic::checked_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::tir::texpr::{
    CoordinateSearch, TConstruct, TConstructorArm, TExpr, TExternArg, TForBinding, TIndexArg,
    TKeyForm, TLabelArm, TParamBinding,
};
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::evaluation_unit::{BodyKind, ScopedTree};
use graphcal_compiler::tir::typed::scoped_node::{
    ConstRef, NodeKind, ScopedCall, ScopedIndexArg, ScopedMatchArms, ScopedNode, ScopedScan,
};

use crate::invariant::{Failure, Invariant};
use crate::presentation_evidence::{PendingLeaf, PendingQuantityDisplay};
use crate::runtime_presentation::{EvaluatedRuntimeValue, PendingPresentedMap, PresentedRef};
use graphcal_compiler::resolved_name::ResolvedDeclName;

use super::context::EvalSession;
use super::operations::read_shape;
use super::unit_scale::{checked_unit_scaled_value, resolve_unit_scale};
use crate::constant_pools::RuntimeValueMap;
use graphcal_compiler::tir::texpr::MapLayout;

pub type HirLocalValueMap<'a> = graphcal_compiler::hir::expr::LocalEnv<'a, EvaluatedRuntimeValue>;

fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::resolved_name::ResolvedIndexName,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

/// The value of declaration `key`, `value`, borrowed with its presentation:
/// the frame keeps a presented value only for a value with a presentation.
fn presented_ref<'a>(
    presentation_values: Option<&'a PendingPresentedMap>,
    key: &ResolvedDeclName,
    value: &'a RuntimeValue,
) -> PresentedRef<'a, PendingLeaf> {
    presentation_values
        .and_then(|presented| presented.get(key))
        .map_or_else(|| PresentedRef::plain(value), EvaluatedRuntimeValue::as_ref)
}

/// The diagnostic for a violated presentation invariant.
fn invariant_error(invariant: Invariant, span: Span, ctx: &EvalSession<'_>) -> SemanticError {
    ctx.failure_error(
        Failure::<std::convert::Infallible>::Invariant(invariant),
        span,
    )
}

/// The diagnostic for a value whose shape contradicts the checked type of
/// the node that produced it.
fn type_invariant(
    message: impl std::fmt::Display,
    span: Span,
    ctx: &EvalSession<'_>,
) -> SemanticError {
    invariant_error(Invariant::violated(message), span, ctx)
}

/// Evaluate the root tree of an evaluation unit in the scope it was handed
/// out with.
pub fn eval_root<T: std::borrow::Borrow<TExpr>>(
    root: &ScopedTree<'_, T>,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    eval_texpr(root.root(), values, &HirLocalValueMap::root(), session)
}

/// [`eval_root`] for the executable trees of unit-scale bodies, as the
/// kernel handed to unit-scale and presentation resolution.
pub(super) fn eval_executable(
    root: &ScopedTree<'_, &TExpr>,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    eval_root(root, values, session)
}

/// Evaluate the root tree of an evaluation unit in the scope it was handed
/// out with, preserving concrete presentation-call identities.
pub fn eval_root_with_presentation<T: std::borrow::Borrow<TExpr>>(
    root: &ScopedTree<'_, T>,
    values: &RuntimeValueMap,
    presentation_values: &PendingPresentedMap,
    session: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    eval_texpr_with_presentation(
        root.root(),
        values,
        presentation_values,
        &HirLocalValueMap::root(),
        session,
    )
}

/// Evaluate `subtree` in its scope with `locals` bound, for tests that forge
/// a local binding.
#[cfg(any(test, feature = "test-internals"))]
pub fn eval_subtree_for_test(
    subtree: ScopedNode<'_>,
    values: &RuntimeValueMap,
    locals: &HirLocalValueMap<'_>,
    session: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    eval_texpr(subtree, values, locals, session)
}

/// Evaluate a checked, executable expression tree.
///
/// Checking published the tree only once every type in it was concrete and
/// every static obligation discharged, so evaluation reads each node's checked
/// type and nominal facts from the node itself. Declaration, constructor,
/// index-variant, inline-DAG, local, and built-in references are canonical.
///
/// `expr` is the root of a declaration's (or another evaluated root's) tree:
/// the availability of every dependency of the whole tree, including
/// unselected branches, is determined once before any of it is evaluated.
fn eval_texpr(
    expr: ScopedNode<'_>,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    ctx.check_dependencies(expr)?;
    eval_value(expr, values, local_values, ctx)
}

/// Evaluate a subtree of a root whose dependency availability has already
/// been determined by [`eval_texpr`] or [`eval_texpr_with_presentation`].
fn eval_value(
    expr: ScopedNode<'_>,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    eval_texpr_evaluated(expr, values, None, local_values, ctx)
        .map(EvaluatedRuntimeValue::into_value)
}

/// Evaluate one checked root tree while preserving concrete presentation-call
/// identities through value-preserving expression forms. Like [`eval_texpr`],
/// it determines the availability of the whole tree's dependencies once.
fn eval_texpr_with_presentation(
    expr: ScopedNode<'_>,
    values: &RuntimeValueMap,
    presentation_values: &PendingPresentedMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    ctx.check_dependencies(expr)?;
    eval_texpr_evaluated(expr, values, Some(presentation_values), local_values, ctx)
}

fn eval_texpr_evaluated(
    expr: ScopedNode<'_>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    ctx.cancellation.checkpoint()?;
    // Recursion choke point: evaluation recurses once per tree level
    // (unbounded for left-nested operator chains).
    graphcal_compiler::stack::with_stack_growth(|| {
        eval_texpr_inner(expr, values, presentation_values, local_values, ctx)
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive typed expression evaluation"
)]
fn eval_texpr_inner(
    expr: ScopedNode<'_>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let span = expr.span();
    let evaluate = |node| eval_value(node, values, local_values, ctx);
    let operands = super::operations::Operands::new(&evaluate, ctx);
    let plain = EvaluatedRuntimeValue::plain;
    match expr.kind() {
        NodeKind::Quantity(operation) => super::operations::quantity(&operation, span, &operands)
            .map(|value| plain(RuntimeValue::Quantity(value))),
        NodeKind::Int(operation) => super::operations::int(&operation, span, &operands)
            .map(|value| plain(RuntimeValue::Int(value))),
        NodeKind::Bool(operation) => super::operations::boolean(&operation, &operands)
            .map(|value| plain(RuntimeValue::Bool(value))),
        NodeKind::Complex(operation) => super::operations::complex(&operation, span, &operands)
            .map(|value| plain(RuntimeValue::Complex(value))),
        NodeKind::Datetime(operation) => super::operations::datetime(&operation, span, &operands)
            .map(|value| plain(RuntimeValue::Datetime(value))),
        NodeKind::KeyShift { key, addend, axis } => {
            super::operations::key_shift(axis, key, addend, span, &operands)
                .map(|value| plain(RuntimeValue::Key(value)))
        }
        NodeKind::QuantityLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, ctx, eval_executable)?;
            let value = checked_unit_scaled_value(value, scale, span, ctx)?;
            let display = super::presentation::scaled(unit.get(), scale, ctx);
            EvaluatedRuntimeValue::with_leaf(
                value,
                PendingLeaf::Quantity(PendingQuantityDisplay::Ready(display)),
            )
            .map_err(|invariant| invariant_error(invariant, span, ctx))
            .map_err(Outcome::Failed)
        }
        NodeKind::GraphRef(target) => {
            let value = resolve_graph_ref(&target, values, ctx)?;
            Ok(presented_ref(presentation_values, &target.value, value)
                .to_owned_with(clone_graph_ref_value))
        }
        NodeKind::Const(target) => match target.value {
            ConstRef::Decl(declaration) => {
                let value = values.get(&declaration).cloned().ok_or_else(|| {
                    ctx.eval_error(format!("undefined constant `{declaration}`"), target.span)
                })?;
                Ok(presentation_values
                    .and_then(|presented| presented.get(&declaration))
                    .map_or_else(|| EvaluatedRuntimeValue::plain(value), Clone::clone))
            }
            ConstRef::Constructor(construct) => {
                eval_constructor_call(construct, values, presentation_values, local_values, ctx)
            }
        },
        NodeKind::Local(local) => local_values
            .get(local.value)
            .cloned()
            .ok_or_else(|| ctx.eval_error("undefined local variable", local.span))
            .map_err(Outcome::Failed),
        NodeKind::DatetimeLiteral(literal) => Ok(plain(RuntimeValue::Datetime(
            super::operations::datetime_literal(literal),
        ))),
        NodeKind::Aggregate { function, arg } => {
            let indexed = operands.indexed(arg)?;
            match function {
                AggregationFn::Key(function) => eval_extremum_key(function, &indexed, span, ctx),
                AggregationFn::Value(function) => {
                    super::aggregations::aggregate_indexed_values(function, &indexed)
                        .map_err(|failure| ctx.failure_error(failure, span))
                }
            }
            .map(plain)
            .map_err(Outcome::Failed)
        }
        NodeKind::LinearAlgebra(call) => {
            super::linear_algebra::evaluate(&call, span, &operands, ctx).map(plain)
        }
        NodeKind::Extern {
            function,
            args,
            result,
        } => eval_extern_fn(span, function, args, result, values, local_values, ctx).map(plain),
        NodeKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            if operands.bool(condition)? {
                eval_texpr_evaluated(then_branch, values, presentation_values, local_values, ctx)
            } else {
                eval_texpr_evaluated(else_branch, values, presentation_values, local_values, ctx)
            }
        }
        NodeKind::Convert {
            expr: inner,
            target,
        } => {
            let value = eval_value(inner, values, local_values, ctx)?;
            EvaluatedRuntimeValue::with_leaf(
                value,
                PendingLeaf::Quantity(super::presentation::pending(target, expr.dag_id(), ctx)),
            )
            .map_err(|invariant| invariant_error(invariant, span, ctx))
            .map_err(Outcome::Failed)
        }
        NodeKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => {
            let value = eval_value(inner, values, local_values, ctx)?;
            EvaluatedRuntimeValue::with_leaf(value, PendingLeaf::Datetime(timezone.clone()))
                .map_err(|invariant| invariant_error(invariant, span, ctx))
                .map_err(Outcome::Failed)
        }
        NodeKind::Field { expr: inner, field } => {
            let inner_val =
                eval_texpr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            // The checker proved the operand's nominal application, and a
            // struct value carries exactly its constructor's declared fields.
            read_shape(
                inner_val,
                format_args!("a struct with field `{}`", field.value),
                field.span,
                ctx,
                |value| {
                    value.into_fields().and_then(|fields| {
                        fields
                            .into_field(&field.value)
                            .map_err(EvaluatedRuntimeValue::from_struct)
                    })
                },
            )
        }
        NodeKind::Construct(construct) => {
            eval_constructor_call(construct, values, presentation_values, local_values, ctx)
        }
        NodeKind::Map { entries, layout } => eval_map_literal(
            entries,
            layout,
            span,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        NodeKind::For { bindings, body } => {
            let (first, remaining) = bindings.split_first();
            eval_for_comp_bindings(
                first,
                remaining,
                body,
                values,
                presentation_values,
                local_values,
                ctx,
            )
        }
        NodeKind::Index { expr: inner, args } => eval_index_access(
            span,
            inner,
            args,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        NodeKind::Scan(scan_node) => {
            eval_scan(scan_node, values, presentation_values, local_values, ctx)
        }
        NodeKind::Unfold {
            recurrence,
            init,
            body,
            axis,
        } => eval_unfold(
            ScopedUnfold {
                recurrence,
                init,
                body,
                axis,
            },
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        NodeKind::Key { form, arg, axis } => eval_key_form(form, axis, arg, span, &operands)
            .map(|key| EvaluatedRuntimeValue::plain(RuntimeValue::Key(key))),
        NodeKind::Match { scrutinee, arms } => match arms {
            ScopedMatchArms::Labels(arms) => {
                let arm = match_label(span, &operands.key(scrutinee)?, arms, ctx)?;
                eval_texpr_evaluated(
                    arm.map(|arm| &arm.body),
                    values,
                    presentation_values,
                    local_values,
                    ctx,
                )
            }
            ScopedMatchArms::Constructors(arms) => eval_constructor_match(
                span,
                scrutinee,
                arms,
                values,
                presentation_values,
                local_values,
                ctx,
            ),
        },
        NodeKind::Variant(key) => Ok(plain(RuntimeValue::Key(key.clone()))),
        NodeKind::DagCall { call, args, output } => eval_dag_call(
            call,
            args,
            output,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
    }
}

fn resolve_graph_ref<'a>(
    target: &Spanned<ResolvedDeclName>,
    values: &'a RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<&'a RuntimeValue, SemanticError> {
    values.get(&target.value).ok_or_else(|| {
        ctx.eval_error(
            format!("undefined graph reference `@{}`", target.value),
            target.span,
        )
    })
}

fn clone_graph_ref_value(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_cloned_runtime_nodes(value);
    value.clone()
}

fn clone_index_access_result(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_cloned_runtime_nodes(value);
    value.clone()
}

#[cfg(test)]
fn record_cloned_runtime_nodes(value: &RuntimeValue) {
    CLONED_RUNTIME_NODES.with(|count| {
        count.set(
            count
                .get()
                .saturating_add(runtime_value_tree_node_count(value)),
        );
    });
}

#[cfg(test)]
std::thread_local! {
    static CLONED_RUNTIME_NODES: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(any(test, feature = "test-internals"))]
fn runtime_value_tree_node_count(value: &RuntimeValue) -> usize {
    match value {
        RuntimeValue::Struct(value) => value
            .fields()
            .map(|(_, field)| runtime_value_tree_node_count(field))
            .fold(1, usize::saturating_add),
        RuntimeValue::Indexed(entries) => entries
            .values()
            .iter()
            .map(runtime_value_tree_node_count)
            .fold(1, usize::saturating_add),
        _ => 1,
    }
}

/// Test-only: reset the count of runtime value nodes cloned by graph
/// references and index accesses.
#[cfg(test)]
pub fn reset_cloned_runtime_node_count() {
    CLONED_RUNTIME_NODES.with(|count| count.set(0));
}

/// Test-only: take the count of cloned runtime value nodes.
#[cfg(test)]
#[must_use]
pub fn take_cloned_runtime_node_count() -> usize {
    CLONED_RUNTIME_NODES.with(|count| count.replace(0))
}

/// Evaluate a key introduction form.
///
/// `key` positions are proven in bounds by the checker; `fin_key` performs
/// its runtime range check here; the coordinate searches scan the axis's
/// coordinates with the documented policies.
fn eval_key_form<'t>(
    form: &TKeyForm,
    axis: &IndexAxis,
    arg: ScopedNode<'t>,
    span: Span,
    operands: &super::operations::Operands<'_, 't>,
) -> Result<KeyValue, Outcome<SemanticError>> {
    let ctx = operands.ctx();
    match form {
        // The key was proved on its axis when the tree was discharged.
        TKeyForm::Static { key, .. } => Ok(key.clone()),
        TKeyForm::Fin => {
            let position = operands.int(arg)?;
            usize::try_from(position)
                .ok()
                .and_then(|position| KeyValue::at(axis.clone(), position))
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!("fin_key: {position} out of bounds for {}", axis.index()),
                        span,
                    )
                })
                .map_err(Outcome::Failed)
        }
        TKeyForm::Search(search) => {
            let quantity = operands.quantity(arg)?.get();
            coordinate_search(*search, axis, quantity, span, ctx).map_err(Outcome::Failed)
        }
    }
}

/// The key a coordinate search of `axis` for `quantity` selects.
fn coordinate_search(
    search: CoordinateSearch,
    axis: &IndexAxis,
    quantity: f64,
    span: Span,
    ctx: &EvalSession<'_>,
) -> Result<KeyValue, SemanticError> {
    let keys = KeyValue::all(axis);
    let mut best: Option<(&KeyValue, f64)> = None;
    // A coordinate axis has one finite coordinate per key.
    for (key, coordinate) in keys.iter().zip(axis.coordinates()) {
        let coordinate = coordinate.get();
        let candidate = match search {
            CoordinateSearch::Floor => coordinate <= quantity,
            CoordinateSearch::Ceil => coordinate >= quantity,
            CoordinateSearch::Nearest => true,
        };
        if !candidate {
            continue;
        }
        let better = match (&best, search) {
            (None, _) => true,
            // Nearest: strictly closer wins, so midpoint ties keep
            // the earlier position (toward the axis start).
            (Some((_, incumbent)), CoordinateSearch::Nearest) => {
                (coordinate - quantity).abs() < (incumbent - quantity).abs()
            }
            // Floor: the greatest coordinate at or below the target.
            (Some((_, incumbent)), CoordinateSearch::Floor) => coordinate > *incumbent,
            // Ceil: the smallest coordinate at or above the target.
            (Some((_, incumbent)), CoordinateSearch::Ceil) => coordinate < *incumbent,
        };
        if better {
            best = Some((key, coordinate));
        }
    }
    best.map(|(key, _)| key.clone()).ok_or_else(|| {
        ctx.eval_error(
            format!(
                "{}: no coordinate of `{}` is {} the target",
                search.kind().as_str(),
                axis.index(),
                if search == CoordinateSearch::Floor {
                    "at or below"
                } else {
                    "at or above"
                },
            ),
            span,
        )
    })
}

/// Evaluate `argmin`/`argmax`: the key of the extremum entry on the reduced
/// axis.
fn eval_extremum_key(
    kind: KeyAggregation,
    indexed: &IndexedValue<RuntimeValue>,
    span: Span,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, SemanticError> {
    super::aggregations::extremum_key(kind, indexed)
        .map(RuntimeValue::Key)
        .map_err(|error| ctx.eval_error(error.to_string(), span))
}

/// The expression of one plugin-call argument, in the argument's scope.
fn argument_node(arg: Scoped<'_, TExternArg>) -> Scoped<'_, TExpr> {
    arg.map(|arg| &arg.value)
}

/// Evaluate an extern (plugin) function call through the embedder-injected
/// host function registry.
///
/// Arguments and the result cross the host ABI through
/// [`HostArguments`](crate::host_abi::marshal::HostArguments), which reads each
/// argument at the ABI kind its checked node carries and rebuilds the result
/// over the typed axes the arguments bound. A closure error or an invalid result
/// becomes a per-node evaluation failure naming the plugin alias and function;
/// dependents report `DependencyFailed` through the ordinary per-node fault
/// isolation.
fn eval_extern_fn(
    span: Span,
    ext: &graphcal_compiler::hir::expr::ExternFnRef,
    args: Scoped<'_, [TExternArg]>,
    result: &graphcal_compiler::function_signature::ResultKind<
        graphcal_compiler::extern_struct_result::ExternStructResult,
    >,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    use crate::host_abi::marshal::{EncodeError, HostArguments};
    use crate::invariant::Failure;

    let Some(registry) = ctx.host_fns() else {
        return Err(ctx.eval_error(
            format!("extern function `{ext}` cannot be evaluated in this context (no host function registry)"),
            span,
        ).into());
    };
    let key = ext.key();
    let Some(host_fn) = registry.get(&key) else {
        return Err(ctx
            .eval_error(
                format!(
                    "extern function `{}` (plugin \"{}\") is not provided by the host",
                    ext.name, ext.plugin
                ),
                span,
            )
            .into());
    };
    let invariant =
        |invariant, span| ctx.internal_error(format!("extern function `{ext}`: {invariant}"), span);

    let evaluate = |node| eval_value(node, values, local_values, ctx);
    let operands = super::operations::Operands::new(&evaluate, ctx);
    let arguments = HostArguments::encode(
        result,
        args.iter().map(|arg| (&arg.get().kind, argument_node(arg))),
        &operands,
    )
    .map_err(|error| match error {
        EncodeError::Value(error) => error,
        EncodeError::Argument { position, failure } => {
            let span = args
                .nth(position)
                .map_or(span, |arg| argument_node(arg).span());
            match failure {
                Failure::Error(error) => ctx.eval_error(error.describe(ext), span),
                Failure::Invariant(error) => invariant(error, span),
            }
            .into()
        }
    })?;

    let result = host_fn(arguments.values()).map_err(|err| {
        ctx.eval_error(
            format!(
                "extern function `{}` (plugin \"{}\") failed: {}",
                ext, ext.plugin, err.message
            ),
            span,
        )
    })?;

    arguments
        .decode(&result)
        .map_err(|failure| match failure {
            Failure::Error(error) => ctx.eval_error(error.describe(ext), span),
            Failure::Invariant(error) => invariant(error, span),
        })
        .map_err(Outcome::Failed)
}

/// Evaluate a constructor call: each field in written order, checked
/// against its field constraint, at its declared place.
fn eval_constructor_call(
    construct: Scoped<'_, TConstruct>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::ConstructorFactConsumption);
    let application = construct.application();
    let constructor_name = application.constructor.name();
    let owning_type = StructTypeRef::from_resolved(application.definition().clone());
    construct
        .apply(|scoped_init| {
            let field_init = scoped_init.get();
            let evaluated = eval_texpr_evaluated(
                scoped_init.map(|init| &init.value),
                values,
                presentation_values,
                local_values,
                ctx,
            )?;
            if application.constructor.constrains(&field_init.name)
                && let Some(field_constraints) = ctx.struct_field_constraints()
            {
                let key =
                    graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                        owning_type.clone(),
                        application.generic_args().to_vec(),
                        constructor_name.clone(),
                        field_init.name.clone(),
                    );
                let constraint = field_constraints.get(&key).ok_or_else(|| {
                    ctx.internal_error(
                        format!(
                            "required field constraint `{constructor_name}.{}` is missing",
                            field_init.name
                        ),
                        field_init.value.span(),
                    )
                })?;
                if let Err(violation) =
                    crate::domain_check::check_domain_constraint(&evaluated.value(), constraint)
                {
                    return Err(ctx
                        .eval_error(
                            format!(
                                "field `{constructor_name}.{}` {}",
                                field_init.name, violation.message
                            ),
                            field_init.value.span(),
                        )
                        .into());
                }
            }
            Ok::<_, Outcome<SemanticError>>(evaluated)
        })
        .map(EvaluatedRuntimeValue::from_struct)
}

/// Evaluate a map literal: each entry in its layout's evaluation order,
/// placed at its cell.
fn eval_map_literal(
    entries: Scoped<'_, [graphcal_compiler::tir::texpr::TMapEntry]>,
    layout: &MapLayout,
    span: Span,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let mut cells = std::iter::repeat_with(|| None)
        .take(layout.cells())
        .collect::<Vec<_>>();
    for placement in layout.placements() {
        let entry = entries
            .nth(placement.entry)
            .map(|entry| entry.map(|entry| &entry.value));
        if let Some(entry) = entry {
            cells[placement.cell] = Some(eval_texpr_evaluated(
                entry,
                values,
                presentation_values,
                local_values,
                ctx,
            )?);
        }
    }
    // The layout fills every cell, row-major, exactly once.
    let mut cells = cells.into_iter();
    let (outer, inner) = layout.axes().split_first();
    nest_cells(outer, inner, &mut cells)
        .ok_or_else(|| type_invariant("a map literal left a cell unfilled", span, ctx))
        .map_err(Outcome::Failed)
}

/// The nested indexed value over `axis` and `inner` whose leaves are the next
/// row-major `cells`; `None` when a cell is unfilled.
fn nest_cells(
    axis: &IndexAxis,
    inner: &[IndexAxis],
    cells: &mut impl Iterator<Item = Option<EvaluatedRuntimeValue>>,
) -> Option<EvaluatedRuntimeValue> {
    IndexedValue::try_from_axis(axis.clone(), |_| {
        match inner.split_first() {
            None => cells.next().flatten(),
            Some((next, rest)) => nest_cells(next, rest, cells),
        }
        .ok_or(())
    })
    .ok()
    .map(EvaluatedRuntimeValue::from_indexed)
}

/// Evaluate a comprehension over `binding`'s axis and, inside each of its
/// keys, over the `remaining` bindings' axes.
fn eval_for_comp_bindings(
    binding: &TForBinding,
    remaining: &[TForBinding],
    body: ScopedNode<'_>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let TForBinding { binding, axis } = binding;
    let mut inner_locals = local_values.child(Vec::new());
    let entries = IndexedValue::try_from_axis(axis.clone(), |key| {
        let binding_value = RuntimeValue::Key(key.clone());
        inner_locals.bind(
            binding.local.id,
            EvaluatedRuntimeValue::plain(binding_value),
        );
        if let Some((next, remaining)) = remaining.split_first() {
            eval_for_comp_bindings(
                next,
                remaining,
                body,
                values,
                presentation_values,
                &inner_locals,
                ctx,
            )
        } else {
            eval_texpr_evaluated(body, values, presentation_values, &inner_locals, ctx)
        }
    })?;
    Ok(EvaluatedRuntimeValue::from_indexed(entries))
}

#[expect(
    clippy::single_match_else,
    reason = "single pattern dispatch keeps borrowed graph references and every index-argument category explicit"
)]
fn eval_index_access(
    span: Span,
    inner: ScopedNode<'_>,
    args: Scoped<'_, [TIndexArg]>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let evaluated;
    let base = match inner.kind() {
        NodeKind::GraphRef(target) => {
            // This replaces the checkpoint normally performed by
            // `eval_value(inner, ...)` while retaining a reference to the
            // stored value instead of deep-cloning it before traversal.
            ctx.cancellation.checkpoint()?;
            let value = resolve_graph_ref(&target, values, ctx)?;
            presented_ref(presentation_values, &target.value, value)
        }
        _ => {
            evaluated =
                eval_texpr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            evaluated.as_ref()
        }
    };
    let evaluate = |node| eval_value(node, values, local_values, ctx);
    let operands = super::operations::Operands::new(&evaluate, ctx);
    let mut current = base;
    for arg in args.iter() {
        let indexed = read_shape(current, "an indexed value", span, ctx, |current| {
            current.entries().ok_or(current)
        })?;
        let (entry, entry_key) = match arg.view() {
            // A label selects only on its own axis, never by its leaf name.
            ScopedIndexArg::Variant(variant) => {
                let entry_key = IndexEntryKey::named(variant.variant.variant().clone());
                let entry = index_ref_matches_resolved(indexed.index(), variant.variant.index())
                    .then(|| indexed.get(&entry_key))
                    .flatten();
                (entry, entry_key)
            }
            ScopedIndexArg::Var(local) => {
                let bound = local_values
                    .get(local.value)
                    .ok_or_else(|| ctx.eval_error("undefined local variable", local.span))?;
                let key = read_shape(
                    bound.value().into_owned(),
                    "a key",
                    local.span,
                    ctx,
                    |value| match value {
                        RuntimeValue::Key(key) => Ok(key),
                        other => Err(other),
                    },
                )?;
                (indexed.get_key(&key), key.entry_key().clone())
            }
            ScopedIndexArg::Key(operand) => {
                let key = operands.key(operand)?;
                (indexed.get_key(&key), key.entry_key().clone())
            }
            // A static integer position on a `Fin` axis (`@m[0, 1]`), proved
            // in range.
            ScopedIndexArg::Position(position) => {
                let entry_key = IndexEntryKey::position(position.position);
                (indexed.get(&entry_key), entry_key)
            }
        };
        current = entry.ok_or_else(|| {
            type_invariant(
                format_args!("checked index entry `{entry_key}` is missing"),
                span,
                ctx,
            )
        })?;
    }
    Ok(current.to_owned_with(clone_index_access_result))
}

fn eval_scan(
    ScopedScan {
        source,
        init,
        acc,
        val,
        body,
    }: ScopedScan<'_>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let source_entries = read_shape(
        eval_texpr_evaluated(source, values, presentation_values, local_values, ctx)?,
        "an indexed value",
        source.span(),
        ctx,
        EvaluatedRuntimeValue::into_entries,
    )?;
    let evaluated_init =
        eval_texpr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let initial = evaluated_init.clone();
    let mut accumulated = evaluated_init;
    let mut scan_locals = local_values.child(Vec::new());
    let result_entries = source_entries.try_map(|_, item| {
        scan_locals.bind(acc.id, accumulated.clone());
        scan_locals.bind(val.id, item);
        accumulated = eval_texpr_evaluated(body, values, presentation_values, &scan_locals, ctx)?
            .with_default_presentation(&initial)
            .map_err(|invariant| invariant_error(invariant, body.span(), ctx))?;
        Ok::<_, Outcome<SemanticError>>(accumulated.clone())
    })?;
    Ok(EvaluatedRuntimeValue::from_indexed(result_entries))
}

/// An unfold node with its operands and the axis it unfolds along.
struct ScopedUnfold<'t> {
    recurrence: &'t graphcal_compiler::hir::expr::UnfoldRecurrence,
    init: ScopedNode<'t>,
    body: ScopedNode<'t>,
    axis: &'t IndexAxis,
}

fn eval_unfold(
    ScopedUnfold {
        recurrence,
        init,
        body,
        axis: index_axis,
    }: ScopedUnfold<'_>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    if index_axis.coordinate_data().is_none() {
        return Err(ctx
            .eval_error(
                format!(
                    "unfold requires a coordinate index, but `{}` is not coordinate-valued",
                    index_axis.index()
                ),
                recurrence.axis.span,
            )
            .into());
    }
    let evaluated_init =
        eval_texpr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let mut previous_state = evaluated_init.clone();

    let mut unfold_locals = local_values.child(Vec::new());
    let result_entries = IndexedValue::try_from_axis(index_axis.clone(), |key| {
        let Some(previous_key) = key
            .position()
            .checked_sub(1)
            .and_then(|previous| KeyValue::at(index_axis.clone(), previous))
        else {
            return Ok(evaluated_init.clone());
        };
        unfold_locals.bind(recurrence.previous_state.id, previous_state.clone());
        unfold_locals.bind(
            recurrence.previous_index.id,
            EvaluatedRuntimeValue::plain(RuntimeValue::Key(previous_key)),
        );
        unfold_locals.bind(
            recurrence.current_index.id,
            EvaluatedRuntimeValue::plain(RuntimeValue::Key(key.clone())),
        );
        previous_state =
            eval_texpr_evaluated(body, values, presentation_values, &unfold_locals, ctx)?
                .with_default_presentation(&evaluated_init)
                .map_err(|invariant| invariant_error(invariant, body.span(), ctx))?;
        Ok::<_, Outcome<SemanticError>>(previous_state.clone())
    })?;
    Ok(EvaluatedRuntimeValue::from_indexed(result_entries))
}

/// The arm matching the label of `key`; the checker proved the arms
/// exhaustive over its axis.
fn match_label<'t>(
    span: Span,
    key: &KeyValue,
    arms: Scoped<'t, [TLabelArm]>,
    ctx: &EvalSession<'_>,
) -> Result<Scoped<'t, TLabelArm>, SemanticError> {
    arms.iter()
        .find(|arm| {
            let label = &arm.get().label.variant;
            index_ref_matches_resolved(key.index(), label.index())
                && matches!(key.entry_key(), IndexEntryKey::Named(name) if name == label.variant())
        })
        .ok_or_else(|| {
            type_invariant(
                format_args!("no match arm for label `{}`", key.entry_key()),
                span,
                ctx,
            )
        })
}

/// Evaluate the arm matching the constructor of the union value
/// `scrutinee` evaluates to, with its field bindings; the checker proved the
/// arms exhaustive over the union.
fn eval_constructor_match(
    span: Span,
    scrutinee: ScopedNode<'_>,
    arms: Scoped<'_, [TConstructorArm]>,
    values: &RuntimeValueMap,
    presentation_values: Option<&PendingPresentedMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let union = read_shape(
        eval_texpr_evaluated(scrutinee, values, presentation_values, local_values, ctx)?,
        "a union value",
        scrutinee.span(),
        ctx,
        EvaluatedRuntimeValue::into_fields,
    )?;
    let arm = arms
        .iter()
        .find(|arm| {
            let target = &arm.get().target;
            *union.constructor() == target.constructor && *union.type_name() == target.runtime_type
        })
        .ok_or_else(|| {
            type_invariant(
                format_args!("no match arm for variant `{}`", union.type_name()),
                span,
                ctx,
            )
        })?;
    let mut arm_locals = local_values.child(Vec::new());
    for binding in &arm.get().bindings {
        match binding {
            graphcal_compiler::hir::expr::PatternBinding::Bind { field, local } => {
                let value = union.field(&field.value).cloned().ok_or_else(|| {
                    type_invariant(
                        format_args!("matched union value has no field `{}`", field.value),
                        field.span,
                        ctx,
                    )
                })?;
                arm_locals.bind(local.id, value);
            }
            graphcal_compiler::hir::expr::PatternBinding::Wildcard { .. } => {}
        }
    }
    eval_texpr_evaluated(
        arm.map(|arm| &arm.body),
        values,
        presentation_values,
        &arm_locals,
        ctx,
    )
}

fn eval_dag_call(
    call: ScopedCall<'_>,
    args: Scoped<'_, [TParamBinding]>,
    output: &Spanned<ResolvedDeclName>,
    caller_values: &RuntimeValueMap,
    caller_presentations: Option<&PendingPresentedMap>,
    caller_locals: &HirLocalValueMap,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>> {
    let target = call.target();
    let plan = ctx.execution_plan()?;
    let callable = plan.call(call);

    let mut frame = crate::execution_frame::ExecutionFrame::new(
        plan,
        callable,
        crate::execution_frame::FailurePolicy::Propagate,
    );
    for scoped_binding in args.iter() {
        let binding = scoped_binding.get();
        let evaluated = eval_texpr_evaluated(
            scoped_binding.map(|binding| &binding.value),
            caller_values,
            caller_presentations,
            caller_locals,
            ctx,
        )?;
        frame.bind_argument(&binding.target, evaluated, ctx.src, binding.value.span())?;
    }

    let evaluated = frame.run(&ctx.cancellation, |entry, frame| {
        let session = ctx.for_declaration(&entry).with_unavailable(frame.errors());
        eval_root_with_presentation(
            entry.body(),
            frame.values(),
            frame.presentations(),
            &session,
        )
    });
    if let Some(calls) = ctx.unfinished_calls {
        calls
            .borrow_mut()
            .extend(frame.unfinished_origins().cloned());
    }
    evaluated?;
    let crate::execution_frame::FrameOutcome {
        values: dag_values,
        presented: mut dag_presented,
        errors,
    } = frame.finish();

    check_inline_plan_asserts(
        callable,
        &dag_values,
        target,
        output.span,
        &ctx.clone().with_unavailable(&errors),
    )?;

    let output_key = &output.value;
    let output_value = dag_values.get(output_key).ok_or_else(|| {
        if let Some(reason) = errors.get(output_key) {
            return SemanticError::located(ctx.src, output.span, EvaluationError::Unavailable { reason: reason.clone() });
        }
        ctx.internal_error(
            format!(
                "dag `{}` has no projected value `{}` after evaluation (should have been caught by dim-check)",
                target,
                output.value.as_str()
            ),
            output.span,
        )
    })?;
    // The frame keeps a presented value only for a value with a presentation.
    let output_presented = dag_presented
        .remove(output_key)
        .unwrap_or_else(|| EvaluatedRuntimeValue::plain(output_value.clone()));
    let presented = super::presentation::resolve_frame(
        output_presented,
        &dag_values,
        ctx,
        callable,
        eval_executable,
    )?;
    #[cfg(any(test, feature = "test-internals"))]
    record_call_retention(&dag_values, &presented);
    Ok(presented)
}

#[cfg(any(test, feature = "test-internals"))]
fn record_call_retention(values: &RuntimeValueMap, presentation: &EvaluatedRuntimeValue) {
    use crate::pipeline_metrics::{Event, record_many};
    record_many(
        Event::CallFrameValueNodes,
        u64::try_from(
            values
                .values()
                .map(runtime_value_tree_node_count)
                .sum::<usize>(),
        )
        .unwrap_or(u64::MAX),
    );
    record_many(
        Event::CallOutputEvidenceNodes,
        u64::try_from(presentation.retained_nodes()).unwrap_or(u64::MAX),
    );
}

fn check_inline_plan_asserts(
    callable: &crate::execution_plan::CallablePlan<'_>,
    values: &RuntimeValueMap,
    target: &graphcal_compiler::dag_id::DagId,
    span: Span,
    ctx: &EvalSession<'_>,
) -> Result<(), Outcome<SemanticError>> {
    callable.execution_dags().iter().try_for_each(|scope| {
        check_inline_dag_asserts(
            *scope,
            values,
            &ctx.with_src(scope.source()),
            target,
            span,
            ctx,
        )
    })
}

/// Check the asserts of an inline-instantiated dag body (#812).
///
/// An inline call site is a fresh instantiation (sugar for a synthetic
/// include), so the dag's asserts are checked here too — instantiating
/// inline must not silently skip the dag's invariants. Unlike the include
/// path, an expression has no reporting surface, so a FAIL or ERROR fails
/// the calling expression (fault-isolated to the calling declaration).
/// `#[expected_fail]` inversion applies as usual.
fn check_inline_dag_asserts(
    scope: crate::checked_program::SealedDag<'_>,
    dag_values: &RuntimeValueMap,
    dag_ctx: &EvalSession<'_>,
    target: &graphcal_compiler::dag_id::DagId,
    call_span: Span,
    ctx: &EvalSession<'_>,
) -> Result<(), Outcome<SemanticError>> {
    for unit in ctx.tir.declaration_bodies(scope.position()) {
        let BodyKind::Assert(body) = unit.kind() else {
            continue;
        };
        let key = unit.identity().clone();
        let name = key.leaf();
        let ef = unit.expected_fail();
        let context = dag_ctx.for_decl(&key);
        let result = crate::assertion_eval::evaluate_assert_with_expected_fail(
            body.map(|entry| &*entry.body),
            ef,
            &mut |expr| eval_root(&context.executable(expr)?, dag_values, &context),
        )?;
        match result {
            crate::eval::types::AssertResult::Pass => {}
            crate::eval::types::AssertResult::Fail { message } => {
                return Err(ctx
                    .eval_error(
                        format!(
                            "assertion `{name}` failed in inline call of dag `{}` ({message})",
                            target.leaf()
                        ),
                        call_span,
                    )
                    .into());
            }
            crate::eval::types::AssertResult::Blocked { reason } => {
                return Err(SemanticError::located(
                    ctx.src,
                    call_span,
                    EvaluationError::Unavailable { reason },
                )
                .into());
            }
            crate::eval::types::AssertResult::Error { message } => {
                return Err(ctx
                    .eval_error(
                        format!(
                            "assertion `{name}` errored in inline call of dag `{}` ({message})",
                            target.leaf()
                        ),
                        call_span,
                    )
                    .into());
            }
        }
    }
    Ok(())
}
