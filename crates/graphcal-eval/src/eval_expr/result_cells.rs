//! Evaluation of a declaration's formula in its result position, cell by
//! cell (#2405).
//!
//! Each element of an indexed value is its own node of the computation DAG,
//! so the cells of a value built in a result position are evaluated
//! independently: a cell that reaches a hole, reads a value that has none,
//! or fails, leaves the other cells' values intact. Only these forms build
//! cells: a `for` comprehension (one cell per key), a map or table literal
//! (one cell per entry), a reference to a declaration or to a slice of it
//! (its cells as they are), and the arms of an `if` or a label `match`.
//! Every other expression is evaluated as a whole, as everywhere else.

use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::tir::texpr::{MapLayout, TForBinding};
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::scoped_node::{NodeKind, ScopedMatchArms, ScopedNode};

use crate::cells::Cell;
use crate::runtime_presentation::EvaluatedRuntimeValue;
use crate::runtime_value::{IndexedValue, RuntimeValue};

use super::context::EvalSession;
use super::entry_reads::{CellRead, HirLocalValueMap, partial_target, read_cells};

type Evaluated<T> = Result<T, Outcome<SemanticError>>;

/// How the expressions that are not cell structure are evaluated, as a
/// whole: to a plain value (an operand), or with its presentation (a cell).
pub(super) struct Whole<'w> {
    pub(super) value: &'w dyn Fn(ScopedNode<'_>, &HirLocalValueMap<'_>) -> Evaluated<RuntimeValue>,
    pub(super) presented:
        &'w dyn Fn(ScopedNode<'_>, &HirLocalValueMap<'_>) -> Evaluated<EvaluatedRuntimeValue>,
    /// An inline DAG call, to the cells of its output, or of a slice of it,
    /// or a `scan` or `unfold`, to one cell per step.
    pub(super) stepped: &'w dyn Fn(ScopedNode<'_>, &HirLocalValueMap<'_>) -> Evaluated<Cell>,
}

/// Evaluate `expr`, a formula in its result position, to its cells.
///
/// # Errors
///
/// Returns why the whole value has none, when no cell structure applies.
pub(super) fn eval_result(
    expr: ScopedNode<'_>,
    local_values: &HirLocalValueMap<'_>,
    whole: &Whole<'_>,
    ctx: &EvalSession<'_>,
) -> Evaluated<Cell> {
    ctx.cancellation.checkpoint()?;
    graphcal_compiler::stack::with_stack_growth(|| {
        eval_result_inner(expr, local_values, whole, ctx)
    })
}

fn eval_result_inner(
    expr: ScopedNode<'_>,
    local_values: &HirLocalValueMap<'_>,
    whole: &Whole<'_>,
    ctx: &EvalSession<'_>,
) -> Evaluated<Cell> {
    let evaluate = |node| (whole.value)(node, local_values);
    let operands = super::operations::Operands::new(&evaluate, ctx);
    let result = |node| eval_result(node, local_values, whole, ctx);
    match expr.kind() {
        NodeKind::For { bindings, body } => {
            let (first, remaining) = bindings.split_first();
            for_cells(first, remaining, body, local_values, whole, ctx)
        }
        NodeKind::Map { entries, layout } => map_cells(entries, layout, &result),
        NodeKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            if operands.bool(condition)? {
                result(then_branch)
            } else {
                result(else_branch)
            }
        }
        NodeKind::Match {
            scrutinee,
            arms: ScopedMatchArms::Labels { arms, dispatch },
        } => {
            let arm = operands.key_arm(scrutinee, dispatch)?;
            result(arms.map(|arms| &arms[arm].body))
        }
        // A reference to a declaration or constant with cells keeps them.
        _ if let Some((target, partial)) = partial_target(expr, ctx) => {
            Ok(Cell::Axis(partial.cells().clone()).read_from(&target.value))
        }
        NodeKind::Index { expr: inner, args }
            if let Some((target, partial)) = partial_target(inner, ctx) =>
        {
            match read_cells(&target, partial.cells(), args, &operands, local_values, ctx)? {
                CellRead::Cells(cells) => Ok(Cell::Axis(cells.clone()).read_from(&target.value)),
                CellRead::Value { .. } => (whole.presented)(expr, local_values).map(Cell::Value),
            }
        }
        // A slice of a call output keeps its cells; the call runs once.
        NodeKind::Index { expr: inner, .. } if matches!(inner.kind(), NodeKind::DagCall { .. }) => {
            (whole.stepped)(expr, local_values)
        }
        // A call output keeps its cells; a scan or unfold has one per step.
        NodeKind::DagCall { .. } | NodeKind::Scan(_) | NodeKind::Unfold { .. } => {
            (whole.stepped)(expr, local_values)
        }
        _ => (whole.presented)(expr, local_values).map(Cell::Value),
    }
}

/// The cell an evaluation of one element gives: its value, or why it has
/// none. Cancellation and violated invariants stop the whole evaluation.
pub(super) fn cell(result: Evaluated<Cell>) -> Evaluated<Cell> {
    match result {
        Ok(cell) => Ok(cell),
        Err(Outcome::Failed(error)) if !matches!(error, SemanticError::Internal(_)) => {
            Ok(Cell::of_error(error))
        }
        Err(stop) => Err(stop),
    }
}

/// One cell per entry of a map or table literal, placed by its layout.
fn map_cells<'t>(
    entries: Scoped<'t, [graphcal_compiler::tir::texpr::TMapEntry]>,
    layout: &MapLayout,
    result: &impl Fn(ScopedNode<'t>) -> Evaluated<Cell>,
) -> Evaluated<Cell> {
    // The layout belongs to this node, so its entry positions are positions
    // of `entries`.
    layout.fill(
        |entry| cell(result(entries.map(|entries| &entries[entry].value))),
        |cells| Cell::Axis(IndexedValue::from_axis_cells(cells)),
    )
}

/// One cell per key of `binding`'s axis, and inside each, per key of the
/// `remaining` bindings' axes.
fn for_cells(
    binding: &TForBinding,
    remaining: &[TForBinding],
    body: ScopedNode<'_>,
    local_values: &HirLocalValueMap<'_>,
    whole: &Whole<'_>,
    ctx: &EvalSession<'_>,
) -> Evaluated<Cell> {
    let TForBinding { binding, axis } = binding;
    let mut inner_locals = local_values.child(Vec::new());
    IndexedValue::try_from_axis(axis.clone(), |key| {
        inner_locals.bind(
            binding.local.id,
            EvaluatedRuntimeValue::plain(RuntimeValue::Key(key.clone())),
        );
        cell(match remaining.split_first() {
            Some((next, remaining)) => for_cells(next, remaining, body, &inner_locals, whole, ctx),
            None => eval_result(body, &inner_locals, whole, ctx),
        })
    })
    .map(Cell::Axis)
}
