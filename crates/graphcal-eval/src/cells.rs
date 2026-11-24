//! Declaration results whose cells may have no value (#2405).
//!
//! Each element of an indexed value takes part in the computation DAG as its
//! own node, so one element can be unfinished or failed while the others
//! have values. A struct value is one node: it is never split into cells.
//!
//! A [`Cell`] tree is what evaluating a declaration's formula in its result
//! position gives. When every cell has a value the tree collapses to that
//! value ([`Cell::complete`]); otherwise the declaration keeps a
//! [`PartialValue`]. A partial value is never an operand: a read of the
//! declaration as a whole has no value, and only a read of one cell, or of a
//! slice in a result position, sees its cells.

use graphcal_compiler::diagnostic::Diagnostic;
use graphcal_compiler::node_unavailable::{NodeUnavailable, RuntimeUnavailable};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::semantic_error::{SemanticError, SemanticErrorKind};
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use crate::runtime_presentation::EvaluatedRuntimeValue;
use crate::runtime_value::IndexedValue;

/// The result of one cell, or of a level of cells along an axis, with values
/// of type `V`: evaluated values, or values whose presentations are resolved
/// for output.
#[derive(Debug, Clone)]
pub enum Cell<V = EvaluatedRuntimeValue> {
    /// The cell has a value.
    Value(V),
    /// The cell's evaluation reached a hole, or read a value that has none.
    Unavailable(RuntimeUnavailable),
    /// The cell's own evaluation failed.
    Failed(SemanticError),
    /// One cell per key of an axis.
    Axis(IndexedValue<Self>),
}

impl<V> Cell<V> {
    /// The same cells with every value mapped by `map`.
    ///
    /// # Errors
    ///
    /// Returns the first error of `map`, in axis order.
    pub fn try_map_values<W, E>(
        &self,
        map: &mut impl FnMut(&V) -> Result<W, E>,
    ) -> Result<Cell<W>, E> {
        Ok(match self {
            Self::Value(value) => Cell::Value(map(value)?),
            Self::Unavailable(reason) => Cell::Unavailable(reason.clone()),
            Self::Failed(error) => Cell::Failed(error.clone()),
            Self::Axis(cells) => Cell::Axis(cells.try_map_ref(|_, cell| cell.try_map_values(map))?),
        })
    }
}

impl Cell {
    /// The cell an evaluation that ended with `error` gives: unavailable for
    /// an unavailable input, failed for any other evaluation error.
    #[must_use]
    pub fn of_error(error: SemanticError) -> Self {
        match error {
            SemanticError::Located(Diagnostic {
                kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason }),
                ..
            }) => Self::Unavailable(reason),
            other => Self::Failed(other),
        }
    }

    /// Whether every cell of this tree has a value.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        match self {
            Self::Value(_) => true,
            Self::Unavailable(_) | Self::Failed(_) => false,
            Self::Axis(cells) => cells.values().iter().all(Self::is_complete),
        }
    }

    /// The value of this tree when every cell has one, else the tree back.
    ///
    /// # Errors
    ///
    /// Returns the tree when a cell has no value.
    pub fn complete(self) -> Result<EvaluatedRuntimeValue, Self> {
        if !self.is_complete() {
            return Err(self);
        }
        match self {
            Self::Value(value) => Ok(value),
            Self::Axis(cells) => cells
                .try_map(|_, cell| cell.complete())
                .map(EvaluatedRuntimeValue::from_indexed),
            incomplete @ (Self::Unavailable(_) | Self::Failed(_)) => Err(incomplete),
        }
    }

    /// Visit every cell without a value, in axis order.
    fn visit_missing<'a>(&'a self, visit: &mut impl FnMut(&'a Self)) {
        match self {
            Self::Value(_) => {}
            Self::Unavailable(_) | Self::Failed(_) => visit(self),
            Self::Axis(cells) => {
                for cell in cells.values() {
                    cell.visit_missing(visit);
                }
            }
        }
    }

    /// These cells as a reader of the declaration `source` sees them: a cell
    /// without a value there has none here because `source`'s cell has none.
    #[must_use]
    pub fn read_from(&self, source: &ResolvedDeclName) -> Self {
        match self {
            Self::Value(value) => Self::Value(value.clone()),
            Self::Unavailable(reason) => Self::Unavailable(reason.read_by(source)),
            Self::Failed(error) => Self::Unavailable(
                NodeUnavailable::EvalFailed {
                    message: error.to_string(),
                }
                .read_by(source),
            ),
            Self::Axis(cells) => Self::Axis(cells.clone().map(|cell| cell.read_from(source))),
        }
    }

    /// Why a value made of these cells has no value as a whole: every reason
    /// of its cells without a value, merged; `None` when every cell has one.
    #[must_use]
    pub fn reason(&self) -> Option<RuntimeUnavailable> {
        let mut reasons = Vec::new();
        self.visit_missing(&mut |missing| match missing {
            Self::Unavailable(reason) => reasons.push(reason.clone()),
            Self::Failed(error) => reasons.push(NodeUnavailable::EvalFailed {
                message: error.to_string(),
            }),
            Self::Value(_) | Self::Axis(_) => {}
        });
        reasons.into_iter().reduce(RuntimeUnavailable::merged)
    }

    /// The first failed cell's error in this tree, in axis order.
    fn first_failure(&self) -> Option<&SemanticError> {
        match self {
            Self::Failed(error) => Some(error),
            Self::Value(_) | Self::Unavailable(_) => None,
            Self::Axis(cells) => cells.values().iter().find_map(Self::first_failure),
        }
    }

    /// Apply `check` to every value of this tree, with the entry keys of its
    /// cell, outermost first; a value it rejects becomes a failed cell.
    #[must_use]
    pub fn check_values(
        self,
        check: &mut impl FnMut(&[IndexEntryKey], &EvaluatedRuntimeValue) -> Option<SemanticError>,
    ) -> Self {
        self.check_values_at(&mut Vec::new(), check)
    }

    fn check_values_at(
        self,
        path: &mut Vec<IndexEntryKey>,
        check: &mut impl FnMut(&[IndexEntryKey], &EvaluatedRuntimeValue) -> Option<SemanticError>,
    ) -> Self {
        match self {
            Self::Value(value) => match check(path, &value) {
                None => Self::Value(value),
                // A value that is itself indexed (a row a reference brought
                // in whole) is checked entry by entry, so one entry outside
                // the domain fails alone.
                Some(error) => value.into_entries().map_or_else(
                    |_| Self::Failed(error),
                    |entries| Self::Axis(entries.map(Self::Value)).check_values_at(path, check),
                ),
            },
            Self::Axis(cells) => {
                let Ok(cells) = cells.try_map(|key, cell| {
                    path.push(key.clone());
                    let checked = cell.check_values_at(path, check);
                    path.pop();
                    Ok::<_, std::convert::Infallible>(checked)
                });
                Self::Axis(cells)
            }
            missing @ (Self::Unavailable(_) | Self::Failed(_)) => missing,
        }
    }
}

/// A cell without a value, and why.
#[derive(Debug, Clone, Copy)]
pub enum MissingCell<'a> {
    /// Its evaluation reached a hole, or read a value that has none.
    Unavailable(&'a RuntimeUnavailable),
    /// Its own evaluation failed.
    Failed(&'a SemanticError),
}

impl MissingCell<'_> {
    /// Why the cell has no value, as a reason.
    #[must_use]
    pub fn reason(self) -> RuntimeUnavailable {
        match self {
            Self::Unavailable(reason) => reason.clone(),
            Self::Failed(error) => NodeUnavailable::EvalFailed {
                message: error.to_string(),
            },
        }
    }
}

/// The cells of a declaration's indexed value, some of which have no value.
#[derive(Debug, Clone)]
pub struct PartialValue {
    cells: IndexedValue<Cell>,
}

impl PartialValue {
    /// The cells of `cells`, when some of them have no value.
    ///
    /// # Errors
    ///
    /// Returns the cells back as a tree when every cell has a value.
    pub fn new(cells: IndexedValue<Cell>) -> Result<Self, Cell> {
        let tree = Cell::Axis(cells);
        if tree.is_complete() {
            return Err(tree);
        }
        match tree {
            Cell::Axis(cells) => Ok(Self { cells }),
            other => Err(other),
        }
    }

    /// The cells, one per key of the outermost axis.
    #[must_use]
    pub const fn cells(&self) -> &IndexedValue<Cell> {
        &self.cells
    }

    /// The first failed cell's error, in axis order.
    #[must_use]
    pub fn first_failure(&self) -> Option<&SemanticError> {
        self.cells.values().iter().find_map(Cell::first_failure)
    }

    /// Every failed cell's error, in axis order.
    #[must_use]
    pub fn failures(&self) -> Vec<&SemanticError> {
        let mut failures = Vec::new();
        for cell in self.cells.values() {
            cell.visit_missing(&mut |missing| {
                if let Cell::Failed(error) = missing {
                    failures.push(error);
                }
            });
        }
        failures
    }

    /// Why the declaration `key` has no value as a whole: every reason of
    /// its cells without a value, merged, as `key`'s own reason.
    ///
    /// A cell that failed by itself and another that is unfinished leave
    /// `key` blocked by the unfinished origins and failed by itself, so
    /// neither fact hides the other.
    #[must_use]
    pub fn reason(&self, key: &ResolvedDeclName) -> RuntimeUnavailable {
        // Every missing leaf's own reason, so a nested row with an unfinished
        // and a failed cell keeps both facts, as a flat row does.
        let mut reasons = Vec::new();
        for cell in self.cells.values() {
            cell.visit_missing(&mut |missing| match missing {
                Cell::Unavailable(reason) => reasons.push(reason.clone()),
                Cell::Failed(error) => reasons.push(NodeUnavailable::EvalFailed {
                    message: error.to_string(),
                }),
                Cell::Value(_) | Cell::Axis(_) => {}
            });
        }
        let unfinished = reasons
            .iter()
            .flat_map(|reason| reason.unfinished().iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        // The failed declarations the missing cells inherit, and `key`
        // itself, which failed in a cell of its own.
        let mut failed_deps = reasons
            .iter()
            .flat_map(|reason| reason.failed_dependencies().iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        failed_deps.insert(key.clone());
        let merged = reasons
            .into_iter()
            .reduce(RuntimeUnavailable::merged)
            .unwrap_or_else(|| NodeUnavailable::EvalFailed {
                message: "a partial value has no missing cell".to_owned(),
            });
        match (
            merged,
            NonEmpty::try_from_vec(unfinished.into_iter().collect()),
        ) {
            (NodeUnavailable::EvalFailed { .. }, Ok(unfinished)) => NodeUnavailable::Blocked {
                unfinished,
                failed_deps: failed_deps.into_iter().collect(),
            },
            (merged, _) => merged.of_declaration(key),
        }
    }
}
