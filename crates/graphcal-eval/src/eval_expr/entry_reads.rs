//! Reading entries of indexed values, and cells of declarations whose
//! indexed values have cells without a value (#2405): what one index
//! argument selects, and the cell a chain of them reaches.

use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::checked_type::IndexTypeRef;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::tir::texpr::TIndexArg;
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::scoped_node::ScopedIndexArg;

use crate::runtime_presentation::{EvaluatedRuntimeValue, PresentedRef};
use crate::runtime_value::{IndexedValue, KeyValue, RuntimeValue};

use super::context::EvalSession;
use super::operations::{Operands, read_shape};

/// The values of the local variables in scope.
pub type HirLocalValueMap<'a> = graphcal_compiler::hir::expr::LocalEnv<'a, EvaluatedRuntimeValue>;

/// A state the checker rules out, so that only a bug in the evaluator
/// reaches it: reported as an internal error, never as a runtime failure of
/// the program.
#[derive(Debug, thiserror::Error)]
pub(super) enum EvaluatorBug {
    #[error("constant `{0}` is not bound where it is read")]
    UnboundConstant(ResolvedDeclName),
    #[error("a local variable is not bound where it is read")]
    UnboundLocal,
    #[error("graph reference `@{0}` is not bound where it is read")]
    UnboundGraphRef(ResolvedDeclName),
    #[error("unfold steps along `{0}`, which is not coordinate-valued")]
    UnfoldWithoutCoordinates(IndexTypeRef),
}

impl EvaluatorBug {
    /// The internal error reporting this state at `span`.
    #[cold]
    pub(super) fn at(self, span: Span, ctx: &EvalSession<'_>) -> SemanticError {
        ctx.internal_error(self.to_string(), span)
    }
}

fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::resolved_name::ResolvedIndexName,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

/// The selector one index argument evaluates to.
pub(super) fn entry_selector<'t>(
    arg: ScopedIndexArg<'t>,
    operands: &Operands<'_, 't>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EntrySelector<'t>, Outcome<SemanticError>> {
    Ok(match arg {
        // A label selects only on its own axis, never by its leaf name.
        ScopedIndexArg::Variant(variant) => EntrySelector::Label(&variant.variant),
        ScopedIndexArg::Var(local) => {
            let bound = local_values
                .get(local.value)
                .ok_or_else(|| EvaluatorBug::UnboundLocal.at(local.span, ctx))?;
            EntrySelector::Key(read_shape(
                bound.value().into_owned(),
                "a key",
                local.span,
                ctx,
                |value| match value {
                    RuntimeValue::Key(key) => Ok(key),
                    other => Err(other),
                },
            )?)
        }
        ScopedIndexArg::Key(operand) => EntrySelector::Key(operands.key(operand)?),
        // A static integer position on a `Fin` axis (`@m[0, 1]`), proved
        // in range.
        ScopedIndexArg::Position(position) => {
            EntrySelector::Entry(IndexEntryKey::position(position.position))
        }
    })
}

/// The declaration `node` reads as a whole, with its cells, when `node` is
/// a reference to a declaration (a graph reference or a constant) whose
/// indexed value has cells without a value.
pub(super) fn partial_target<'s>(
    node: graphcal_compiler::tir::typed::scoped_node::ScopedNode<'_>,
    ctx: &EvalSession<'s>,
) -> Option<(Spanned<ResolvedDeclName>, &'s crate::cells::PartialValue)> {
    use graphcal_compiler::tir::typed::scoped_node::{ConstRef, NodeKind};
    let target = match node.kind() {
        NodeKind::GraphRef(target) => target,
        NodeKind::Const(Spanned {
            value: ConstRef::Decl(declaration),
            span,
        }) => Spanned::new(declaration, span),
        _ => return None,
    };
    let partial = ctx.partial?.get(&target.value)?;
    Some((target, partial))
}

/// What reading the cells of a partial value with some index arguments
/// selects.
pub(super) enum CellRead<'p> {
    /// A cell with a value, after `consumed` arguments; the remaining ones
    /// select inside the value.
    Value {
        value: &'p EvaluatedRuntimeValue,
        consumed: usize,
    },
    /// The cells of a slice: every argument selected an axis level.
    Cells(&'p IndexedValue<crate::cells::Cell>),
}

/// Select the cell of `cells`, the cells of `target`, that
/// `args` name. A cell without a value makes the read unavailable for its
/// reason; a read of the whole value is not a cell read.
pub(super) fn read_cells<'p, 't>(
    target: &Spanned<ResolvedDeclName>,
    cells: &'p IndexedValue<crate::cells::Cell>,
    args: Scoped<'t, [TIndexArg]>,
    operands: &Operands<'_, 't>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalSession<'_>,
) -> Result<CellRead<'p>, Outcome<SemanticError>> {
    let mut level = cells;
    for (position, arg) in args.iter().enumerate() {
        let selector = entry_selector(arg.view(), operands, local_values, ctx)?;
        let cell = selector.select_cell(level).ok_or_else(|| {
            ctx.internal_error(
                format!("the cells of `{}` have no entry {selector}", target.value),
                target.span,
            )
        })?;
        match cell {
            crate::cells::Cell::Axis(cells) => level = cells,
            crate::cells::Cell::Value(value) => {
                return Ok(CellRead::Value {
                    value,
                    consumed: position.saturating_add(1),
                });
            }
            crate::cells::Cell::Unavailable(reason) => {
                return Err(unavailable_dependency(target, reason, ctx).into());
            }
            crate::cells::Cell::Failed(error) => {
                let reason = graphcal_compiler::node_unavailable::NodeUnavailable::EvalFailed {
                    message: error.to_string(),
                };
                return Err(unavailable_dependency(target, &reason, ctx).into());
            }
        }
    }
    Ok(CellRead::Cells(level))
}

/// A read of `cells`, a slice of `target`'s cells some of which have no
/// value, where the whole slice is needed.
pub(super) fn unavailable_cells(
    target: &Spanned<ResolvedDeclName>,
    cells: &IndexedValue<crate::cells::Cell>,
    ctx: &EvalSession<'_>,
) -> SemanticError {
    crate::cells::Cell::Axis(cells.clone())
        .reason()
        .map_or_else(
            || {
                ctx.internal_error(
                    format!(
                        "a slice of `{}` without missing cells is unavailable",
                        target.value
                    ),
                    target.span,
                )
            },
            |reason| unavailable_dependency(target, &reason, ctx),
        )
}

/// A read at `target` of a value that has no value for `reason`.
fn unavailable_dependency(
    target: &Spanned<ResolvedDeclName>,
    reason: &graphcal_compiler::node_unavailable::RuntimeUnavailable,
    ctx: &EvalSession<'_>,
) -> SemanticError {
    SemanticError::located(
        ctx.src,
        target.span,
        EvaluationError::Unavailable {
            reason: reason.read_by(&target.value),
        },
    )
}

/// How one index argument selects an entry of an indexed value.
pub(super) enum EntrySelector<'t> {
    /// A qualified label: an entry of its own axis only.
    Label(&'t graphcal_compiler::resolved_name::ResolvedIndexVariant),
    /// A key of the value's axis, or a narrower `Fin` key widened onto it.
    Key(KeyValue),
    /// A static position on a `Fin` axis.
    Entry(IndexEntryKey),
}

impl EntrySelector<'_> {
    /// The cell of `cells` this selector names, if its axis has one.
    pub(super) fn select_cell<'a, T>(&self, cells: &'a IndexedValue<T>) -> Option<&'a T> {
        match self {
            Self::Label(variant) => index_ref_matches_resolved(cells.index(), variant.index())
                .then(|| cells.get(&IndexEntryKey::named(variant.variant().clone())))
                .flatten(),
            Self::Key(key) => cells.get_key(key),
            Self::Entry(entry) => cells.get(entry),
        }
    }

    /// The entry of `indexed` this selector names, if its axis has one.
    pub(super) fn select<'a, L>(
        &self,
        indexed: crate::runtime_presentation::EntriesRef<'a, L>,
    ) -> Option<PresentedRef<'a, L>> {
        match self {
            Self::Label(variant) => index_ref_matches_resolved(indexed.index(), variant.index())
                .then(|| indexed.get(&IndexEntryKey::named(variant.variant().clone())))
                .flatten(),
            Self::Key(key) => indexed.get_key(key),
            Self::Entry(entry) => indexed.get(entry),
        }
    }
}

impl std::fmt::Display for EntrySelector<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Label(variant) => write!(f, "`{variant}`"),
            Self::Key(key) => write!(f, "`{}`", key.entry_key()),
            Self::Entry(entry) => write!(f, "`{entry}`"),
        }
    }
}
