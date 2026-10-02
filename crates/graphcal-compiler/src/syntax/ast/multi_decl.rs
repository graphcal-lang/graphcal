//! Multi-declaration surface form (issue #481).
//!
//! A multi-decl is a single surface form — e.g.,
//!
//! ```text
//! param a: T[I], const node b: U[I, J] = table[I, (_, J)] { : _, X, …; … };
//! ```
//!
//! — represented in the raw AST as `DeclKind::Sugar(RawDeclSugar::Multi(_))`.
//! The desugar pass expands it into N parallel ordinary declarations;
//! consumers that want the surface form (formatter, surface-aware LSP
//! features) read it directly.
//!
//! Multi-decl is a raw-only construct, so nothing here carries a phase
//! parameter. Its correlated collections are built only through
//! [`MultiDeclBuilder`], which lays each slice header out against the slots
//! it was created with: every slice's column layout is aligned with the slots
//! by construction, and every row is as wide as its header.

use crate::syntax::ast::common::Visibility;
use crate::syntax::ast::value::{Expr, MapEntryKey, TableIndexSpec, TypeExpr};
use crate::syntax::decl_name::DeclName;
use crate::syntax::format_equivalent::FormatEquivalent;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::AtLeastTwo;
use crate::syntax::span::{Span, Spanned};

/// Value-declaration kind of a multi-decl slot, with the visibility the kind
/// admits: `param` declares an input port and carries none, while `node` /
/// `const node` are either private or `pub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FormatEquivalent)]
pub enum SlotKind {
    Param,
    Node(Visibility),
    ConstNode(Visibility),
}

/// The surface form of a multi-decl: parallel declaration slots sharing a
/// single `table[…] {…}` initializer.
///
/// Built by [`MultiDeclBuilder`]; the read-only accessors keep a
/// parser-produced value from being mutated into a shape that desugaring
/// cannot expand.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct MultiDecl {
    slots: AtLeastTwo<MultiDeclSlot>,
    shared_axes: MultiDeclSharedAxes,
    slices: Vec<MultiDeclSlice>,
    /// Full surface span: from the first slot's kind keyword through the
    /// closing `;`.
    #[fe(skip)]
    pub(crate) span: Span,
    /// Span of the `table[…] {…}` sub-expression.
    #[fe(skip)]
    pub(crate) table_expr_span: Span,
}

impl MultiDecl {
    #[must_use]
    pub const fn slots(&self) -> &AtLeastTwo<MultiDeclSlot> {
        &self.slots
    }

    #[must_use]
    pub const fn shared_axes(&self) -> &MultiDeclSharedAxes {
        &self.shared_axes
    }

    #[must_use]
    pub fn slices(&self) -> &[MultiDeclSlice] {
        &self.slices
    }
}

/// One slot in a multi-decl: kind keyword (with its visibility), name, type
/// annotation, and the slot's entry in the `(…)` slot tuple.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct MultiDeclSlot {
    pub kind: SlotKind,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    /// The slot's entry in the slot tuple `(…)`.
    pub axis: MultiSlotAxis,
    /// Span from kind keyword through end of the type annotation.
    #[fe(skip)]
    pub(crate) header_span: Span,
}

/// Per-slot entry in the slot tuple `(…)`.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum MultiSlotAxis {
    /// `_` — 1-D slot, typed `T[SharedAxis]`.
    Underscore,
    /// Named axis path — 2-D slot, typed `T[SharedAxis, ExtraAxis]`.
    Axis(Spanned<NamePath>),
}

/// Where one slot's columns live within a slice's header row.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum MultiSlotColumnSpan {
    /// 1-D slot: one column at `col_idx`.
    Single(usize),
    /// 2-D slot: columns `start..end`, one per variant of `extra_axis`.
    Range {
        start: usize,
        end: usize,
        extra_axis: Spanned<NamePath>,
    },
}

/// One slice of a multi-decl body: optional slice-label prefix + header + rows.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct MultiDeclSlice {
    prefix_keys: Vec<MapEntryKey>,
    header_cells: Vec<MultiHeaderCell>,
    /// One entry per slot, in slot order.
    column_layout: Vec<MultiSlotColumnSpan>,
    rows: Vec<MultiDataRow>,
}

impl MultiDeclSlice {
    #[must_use]
    pub fn prefix_keys(&self) -> &[MapEntryKey] {
        &self.prefix_keys
    }

    #[must_use]
    pub fn header_cells(&self) -> &[MultiHeaderCell] {
        &self.header_cells
    }

    /// Column span of each slot, aligned with [`MultiDecl::slots`].
    #[must_use]
    pub fn column_layout(&self) -> &[MultiSlotColumnSpan] {
        &self.column_layout
    }

    #[must_use]
    pub fn rows(&self) -> &[MultiDataRow] {
        &self.rows
    }
}

/// One cell of a multi-decl header row. A variant label is bare: the slot
/// whose columns it falls in supplies its axis.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum MultiHeaderCell {
    Underscore {
        #[fe(skip)]
        span: Span,
    },
    Variant {
        variant: Spanned<IndexVariantName>,
        #[fe(skip)]
        span: Span,
    },
}

impl MultiHeaderCell {
    /// Returns the span of this cell.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Underscore { span } | Self::Variant { span, .. } => *span,
        }
    }
}

/// One data row of a multi-decl body: row-axis key + value per header column.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct MultiDataRow {
    row_key: MapEntryKey,
    values: Vec<Expr>,
}

impl MultiDataRow {
    /// Key of this row on the multi-decl's row axis.
    #[must_use]
    pub const fn row_key(&self) -> &MapEntryKey {
        &self.row_key
    }

    #[must_use]
    pub fn values(&self) -> &[Expr] {
        &self.values
    }
}

/// Shared axes in a multi-declaration table prefix.
///
/// The final axis has a distinct semantic role: it is the row axis. Any axes
/// before it are slice axes. This is intentionally not modeled as a generic
/// `NonEmpty<TableIndexSpec>` because the tail element is special.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct MultiDeclSharedAxes {
    slice_axes: Vec<TableIndexSpec>,
    row_axis: TableIndexSpec,
}

impl MultiDeclSharedAxes {
    /// Construct shared axes from zero or more slice axes and the always-present row axis.
    #[must_use]
    const fn new(slice_axes: Vec<TableIndexSpec>, row_axis: TableIndexSpec) -> Self {
        Self {
            slice_axes,
            row_axis,
        }
    }

    /// Convert a parser-order vector into semantic slice/row axes.
    ///
    /// # Errors
    ///
    /// Returns [`crate::syntax::non_empty::EmptyVecError`] when `axes` is empty.
    pub(crate) fn try_from_vec(
        mut axes: Vec<TableIndexSpec>,
    ) -> Result<Self, crate::syntax::non_empty::EmptyVecError> {
        let row_axis = axes.pop().ok_or(crate::syntax::non_empty::EmptyVecError)?;
        Ok(Self::new(axes, row_axis))
    }

    /// Slice axes preceding the row axis.
    #[must_use]
    pub(crate) fn slice_axes(&self) -> &[TableIndexSpec] {
        &self.slice_axes
    }

    /// The row axis.
    #[must_use]
    pub const fn row_axis(&self) -> &TableIndexSpec {
        &self.row_axis
    }

    /// Number of shared axes. Always at least 1.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn len(&self) -> usize {
        self.slice_axes.len() + 1
    }

    /// Returns `false`; provided for sequence-like callers.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Iterate over axes in source order: slice axes first, then row axis.
    pub fn iter(&self) -> impl Iterator<Item = &TableIndexSpec> {
        self.slice_axes
            .iter()
            .chain(std::iter::once(&self.row_axis))
    }
}

impl std::ops::Index<usize> for MultiDeclSharedAxes {
    type Output = TableIndexSpec;

    #[expect(
        clippy::panic,
        reason = "Index implementations conventionally panic on out-of-bounds access"
    )]
    fn index(&self, index: usize) -> &Self::Output {
        match index.cmp(&self.slice_axes.len()) {
            std::cmp::Ordering::Less => &self.slice_axes[index],
            std::cmp::Ordering::Equal => &self.row_axis,
            std::cmp::Ordering::Greater => {
                panic!("multi-decl shared axis index out of bounds")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// A slice header row that does not fit the slots' tuple entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiDeclLayoutError {
    /// A 1-D slot's cell is a variant label instead of `_`.
    VariantCellForScalarSlot { span: Span, slot_name: DeclName },
    /// The header has fewer cells than the slots need, or cells left over.
    HeaderArity {
        slot_count: usize,
        header_count: usize,
    },
    /// An extra-axis slot has no variant cells.
    NotEnoughCells { slot_name: DeclName, span: Span },
}

/// A data row whose width differs from its slice header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiDeclRowWidthError {
    pub header_count: usize,
    pub value_count: usize,
}

/// Builds a [`MultiDecl`] slice by slice, laying every slice header out
/// against the slots the builder owns.
#[derive(Debug)]
pub struct MultiDeclBuilder {
    slots: AtLeastTwo<MultiDeclSlot>,
    shared_axes: MultiDeclSharedAxes,
    slices: Vec<MultiDeclSlice>,
}

impl MultiDeclBuilder {
    #[must_use]
    pub const fn new(slots: AtLeastTwo<MultiDeclSlot>, shared_axes: MultiDeclSharedAxes) -> Self {
        Self {
            slots,
            shared_axes,
            slices: Vec::new(),
        }
    }

    #[must_use]
    pub const fn shared_axes(&self) -> &MultiDeclSharedAxes {
        &self.shared_axes
    }

    /// Start a slice by mapping its header cells to the slots.
    ///
    /// For each slot tuple entry:
    /// - `_` consumes exactly one header cell, which must be `_`.
    /// - An axis consumes all contiguous variant cells until the next `_` (or
    ///   the end of the row); the slot supplies their axis.
    ///
    /// The last rule assumes **at most one extra-axis slot** (v2); adjacent
    /// extra-axis slots would need disambiguation by axis lookup.
    ///
    /// # Errors
    ///
    /// Returns a [`MultiDeclLayoutError`] when the header does not fit the
    /// slots.
    pub fn begin_slice(
        &mut self,
        prefix_keys: Vec<MapEntryKey>,
        header_cells: Vec<MultiHeaderCell>,
    ) -> Result<MultiSliceBuilder<'_>, MultiDeclLayoutError> {
        let column_layout = layout_columns(&self.slots, &header_cells)?;
        Ok(MultiSliceBuilder {
            decl: self,
            slice: MultiDeclSlice {
                prefix_keys,
                header_cells,
                column_layout,
                rows: Vec::new(),
            },
        })
    }

    #[must_use]
    pub const fn slice_count(&self) -> usize {
        self.slices.len()
    }

    #[must_use]
    pub fn finish(self, span: Span, table_expr_span: Span) -> MultiDecl {
        MultiDecl {
            slots: self.slots,
            shared_axes: self.shared_axes,
            slices: self.slices,
            span,
            table_expr_span,
        }
    }
}

/// A slice under construction; see [`MultiDeclBuilder::begin_slice`].
#[derive(Debug)]
pub struct MultiSliceBuilder<'a> {
    decl: &'a mut MultiDeclBuilder,
    slice: MultiDeclSlice,
}

impl MultiSliceBuilder<'_> {
    /// Number of header columns every row must fill.
    #[must_use]
    pub const fn header_count(&self) -> usize {
        self.slice.header_cells.len()
    }

    #[must_use]
    pub const fn row_count(&self) -> usize {
        self.slice.rows.len()
    }

    /// Append a data row.
    ///
    /// # Errors
    ///
    /// Returns [`MultiDeclRowWidthError`] when the row is not exactly as wide
    /// as the header.
    pub fn push_row(
        &mut self,
        row_key: MapEntryKey,
        values: Vec<Expr>,
    ) -> Result<(), MultiDeclRowWidthError> {
        let header_count = self.header_count();
        if values.len() != header_count {
            return Err(MultiDeclRowWidthError {
                header_count,
                value_count: values.len(),
            });
        }
        self.slice.rows.push(MultiDataRow { row_key, values });
        Ok(())
    }

    /// Add the slice to its multi-decl.
    pub fn finish(self) {
        self.decl.slices.push(self.slice);
    }
}

fn layout_columns(
    slots: &AtLeastTwo<MultiDeclSlot>,
    header_cells: &[MultiHeaderCell],
) -> Result<Vec<MultiSlotColumnSpan>, MultiDeclLayoutError> {
    let arity_error = || MultiDeclLayoutError::HeaderArity {
        slot_count: slots.len(),
        header_count: header_cells.len(),
    };
    let mut layout = Vec::with_capacity(slots.len());
    let mut cursor = 0usize;

    for slot in slots {
        match &slot.axis {
            MultiSlotAxis::Underscore => {
                match header_cells.get(cursor) {
                    None => return Err(arity_error()),
                    Some(MultiHeaderCell::Underscore { .. }) => {}
                    Some(MultiHeaderCell::Variant { span, .. }) => {
                        return Err(MultiDeclLayoutError::VariantCellForScalarSlot {
                            span: *span,
                            slot_name: slot.name.value.clone(),
                        });
                    }
                }
                layout.push(MultiSlotColumnSpan::Single(cursor));
                cursor += 1;
            }
            MultiSlotAxis::Axis(extra_axis) => {
                let start = cursor;
                cursor += header_cells[start..]
                    .iter()
                    .take_while(|cell| matches!(cell, MultiHeaderCell::Variant { .. }))
                    .count();
                if cursor == start {
                    return Err(MultiDeclLayoutError::NotEnoughCells {
                        slot_name: slot.name.value.clone(),
                        span: extra_axis.span,
                    });
                }
                layout.push(MultiSlotColumnSpan::Range {
                    start,
                    end: cursor,
                    extra_axis: extra_axis.clone(),
                });
            }
        }
    }

    if cursor != header_cells.len() {
        return Err(arity_error());
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{ExprKind, TypeExprKind};
    use crate::syntax::names::NameAtom;

    fn span(offset: usize) -> Span {
        Span::new(offset, 1)
    }

    fn slot(name: &str, axis: MultiSlotAxis) -> MultiDeclSlot {
        MultiDeclSlot {
            kind: SlotKind::Param,
            name: Spanned::new(DeclName::expect_valid(name), span(0)),
            type_ann: TypeExpr::unindexed(crate::syntax::ast::ElementTypeExpr {
                kind: TypeExprKind::Dimensionless,
                constraints: vec![],
                span: span(0),
            }),
            axis,
            header_span: span(0),
        }
    }

    fn extra_axis() -> MultiSlotAxis {
        MultiSlotAxis::Axis(Spanned::new(
            NamePath::local(NameAtom::parse("Mode").unwrap()),
            span(7),
        ))
    }

    fn underscore(offset: usize) -> MultiHeaderCell {
        MultiHeaderCell::Underscore { span: span(offset) }
    }

    fn variant(label: &str, offset: usize) -> MultiHeaderCell {
        MultiHeaderCell::Variant {
            variant: Spanned::new(IndexVariantName::expect_valid(label), span(offset)),
            span: span(offset),
        }
    }

    fn builder(first: MultiSlotAxis, second: MultiSlotAxis) -> MultiDeclBuilder {
        let axes = MultiDeclSharedAxes::try_from_vec(vec![TableIndexSpec::Finite {
            cardinality: 1,
            span: span(0),
        }])
        .unwrap();
        MultiDeclBuilder::new(AtLeastTwo::new(slot("a", first), slot("b", second)), axes)
    }

    fn number(value: f64) -> Expr {
        Expr::new(ExprKind::Number(value), span(0))
    }

    #[test]
    fn layout_assigns_one_column_per_1d_slot_and_a_run_per_extra_axis_slot() {
        let mut builder = builder(MultiSlotAxis::Underscore, extra_axis());
        let slice = builder
            .begin_slice(
                vec![],
                vec![underscore(1), variant("Safe", 2), variant("Nominal", 3)],
            )
            .unwrap();
        assert_eq!(slice.header_count(), 3);
        slice.finish();
        let multi = builder.finish(span(0), span(0));
        let layout = multi.slices()[0].column_layout();
        assert!(matches!(layout[0], MultiSlotColumnSpan::Single(0)));
        assert!(matches!(
            layout[1],
            MultiSlotColumnSpan::Range {
                start: 1,
                end: 3,
                ..
            }
        ));
    }

    #[test]
    fn layout_rejects_a_variant_cell_for_a_1d_slot() {
        let mut builder = builder(MultiSlotAxis::Underscore, MultiSlotAxis::Underscore);
        let error = builder
            .begin_slice(vec![], vec![variant("Safe", 4), underscore(5)])
            .unwrap_err();
        assert_eq!(
            error,
            MultiDeclLayoutError::VariantCellForScalarSlot {
                span: span(4),
                slot_name: DeclName::expect_valid("a"),
            }
        );
    }

    #[test]
    fn layout_rejects_an_extra_axis_slot_without_variant_cells() {
        let mut builder = builder(extra_axis(), MultiSlotAxis::Underscore);
        let error = builder
            .begin_slice(vec![], vec![underscore(1)])
            .unwrap_err();
        assert_eq!(
            error,
            MultiDeclLayoutError::NotEnoughCells {
                slot_name: DeclName::expect_valid("a"),
                span: span(7),
            }
        );
    }

    #[test]
    fn layout_rejects_missing_and_leftover_cells() {
        let mut builder = builder(MultiSlotAxis::Underscore, MultiSlotAxis::Underscore);
        let arity = MultiDeclLayoutError::HeaderArity {
            slot_count: 2,
            header_count: 1,
        };
        assert_eq!(
            builder
                .begin_slice(vec![], vec![underscore(1)])
                .unwrap_err(),
            arity
        );
        assert_eq!(
            builder
                .begin_slice(vec![], vec![underscore(1), underscore(2), underscore(3)])
                .unwrap_err(),
            MultiDeclLayoutError::HeaderArity {
                slot_count: 2,
                header_count: 3,
            }
        );
    }

    #[test]
    fn rows_must_be_exactly_as_wide_as_the_header() {
        let mut builder = builder(MultiSlotAxis::Underscore, MultiSlotAxis::Underscore);
        let mut slice = builder
            .begin_slice(vec![], vec![underscore(1), underscore(2)])
            .unwrap();
        let label = MapEntryKey::Finite {
            axis_span: span(0),
            position: Spanned::new(
                crate::syntax::fin_position::FinPosition::try_new(1, 0).expect("inside Fin(1)"),
                span(0),
            ),
        };
        assert_eq!(
            slice.push_row(label.clone(), vec![number(1.0)]),
            Err(MultiDeclRowWidthError {
                header_count: 2,
                value_count: 1,
            })
        );
        assert_eq!(slice.row_count(), 0);
        slice
            .push_row(label, vec![number(1.0), number(2.0)])
            .unwrap();
        assert_eq!(slice.row_count(), 1);
        slice.finish();
        assert_eq!(builder.slice_count(), 1);
    }
}
