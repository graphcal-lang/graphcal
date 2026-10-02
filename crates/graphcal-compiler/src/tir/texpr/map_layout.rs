//! The placement of a map literal's entries on its concrete axes.
//!
//! Checking proved that the entries of a map literal cover every combination
//! of its axes' keys exactly once. A concrete tree records that proof as a
//! [`MapLayout`]: the cell each entry fills and the order entries are
//! evaluated in, so evaluation places values without looking keys up.

use thiserror::Error;

use crate::hir::expr::MapEntryKey;
use crate::semantic::index_axis::IndexAxis;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::non_empty::NonEmpty;

/// The entries of a map literal placed on its axes.
///
/// Built only by [`MapLayout::try_new`], which admits entries that name one
/// key of each axis and cover every combination of keys exactly once.
#[derive(Debug, Clone)]
pub struct MapLayout {
    axes: NonEmpty<IndexAxis>,
    /// Each entry, in evaluation order: its position among the written
    /// entries and the row-major cell (the last axis varies fastest) it
    /// fills.
    placements: Vec<Placement>,
}

/// The values of one axis level of a filled map literal: exactly one per key
/// of `axis`, in axis order.
///
/// Built only by [`MapLayout::fill`], whose layout covers every cell exactly
/// once.
#[derive(Debug, Clone)]
pub struct AxisCells<T> {
    axis: IndexAxis,
    cells: NonEmpty<T>,
}

impl<T> AxisCells<T> {
    /// The axis and its values, one per key in axis order.
    #[must_use]
    pub fn into_parts(self) -> (IndexAxis, NonEmpty<T>) {
        (self.axis, self.cells)
    }
}

/// Where one map-literal entry goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The entry's position among the written entries.
    pub entry: usize,
    /// The row-major cell the entry's value fills.
    pub cell: usize,
}

/// Why map-literal entries cannot be placed on their axes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MapLayoutError {
    #[error("a map literal selects on no axis")]
    NoAxis,
    #[error("a map-literal entry names {keys} keys for {axes} axes")]
    Arity { keys: usize, axes: usize },
    #[error("a map-literal key names no entry of the axis `{axis}`")]
    KeyOutsideAxis {
        axis: Box<crate::semantic::checked_type::IndexTypeRef>,
    },
    #[error("map-literal entries fill {filled} of {cells} cells")]
    Coverage { filled: usize, cells: usize },
}

impl MapLayout {
    /// Place entries whose keys are `entries`, in written order, on `axes`.
    ///
    /// Entries are evaluated grouped by their keys on every axis but the
    /// last, in axis order, and in written order within a group.
    ///
    /// # Errors
    ///
    /// Returns a [`MapLayoutError`] when an entry does not name one key of
    /// each axis or the entries do not cover every cell exactly once.
    pub fn try_new(
        axes: Vec<IndexAxis>,
        entries: &[&NonEmpty<MapEntryKey>],
    ) -> Result<Self, MapLayoutError> {
        let axes = NonEmpty::try_from_vec(axes).map_err(|_| MapLayoutError::NoAxis)?;
        let cells = axes.iter().map(IndexAxis::len).product::<usize>();
        let mut filled = vec![false; cells];
        let mut keyed = entries
            .iter()
            .enumerate()
            .map(|(entry, keys)| {
                if keys.len() != axes.len() {
                    return Err(MapLayoutError::Arity {
                        keys: keys.len(),
                        axes: axes.len(),
                    });
                }
                let positions = axes
                    .iter()
                    .zip(keys.iter())
                    .map(|(axis, key)| position(axis, key))
                    .collect::<Result<Vec<_>, _>>()?;
                let cell = axes
                    .iter()
                    .zip(&positions)
                    .fold(0, |cell, (axis, position)| cell * axis.len() + position);
                Ok((positions, Placement { entry, cell }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let unique = keyed
            .iter()
            .filter(|(_, placement)| !std::mem::replace(&mut filled[placement.cell], true))
            .count();
        if unique != keyed.len() || unique != cells {
            return Err(MapLayoutError::Coverage {
                filled: unique,
                cells,
            });
        }
        // A stable sort keeps written order within a group.
        let groups = axes.len() - 1;
        keyed.sort_by(|(lhs, _), (rhs, _)| lhs[..groups].cmp(&rhs[..groups]));
        Ok(Self {
            axes,
            placements: keyed.into_iter().map(|(_, placement)| placement).collect(),
        })
    }

    /// The axes the entries select on, outermost first.
    #[must_use]
    pub const fn axes(&self) -> &NonEmpty<IndexAxis> {
        &self.axes
    }

    /// Every entry's placement, in evaluation order; together they fill
    /// every cell exactly once.
    #[must_use]
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    /// The number of cells: one per combination of axis keys.
    #[must_use]
    pub const fn cells(&self) -> usize {
        self.placements.len()
    }

    /// The nested value of the map literal: each entry's value, computed by
    /// `entry` from its written position in evaluation order, placed in its
    /// cell, and nested axis by axis (innermost first) by `nest`.
    ///
    /// # Errors
    ///
    /// Returns the first error `entry` returns; later entries are not
    /// computed.
    pub fn fill<T, E>(
        &self,
        mut entry: impl FnMut(usize) -> Result<T, E>,
        mut nest: impl FnMut(AxisCells<T>) -> T,
    ) -> Result<T, E> {
        let mut placed = Vec::with_capacity(self.placements.len());
        for placement in &self.placements {
            placed.push((placement.cell, entry(placement.entry)?));
        }
        // The placements fill every cell exactly once, so sorting by cell
        // leaves one value per cell in row-major order.
        placed.sort_by_key(|(cell, _)| *cell);
        let mut level = placed
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        for axis in self.axes.iter().rev() {
            // Each level holds a whole number of runs of this axis's length.
            let mut values = level.into_iter();
            let mut nested = Vec::new();
            while let Some(first) = values.next() {
                let rest = values.by_ref().take(axis.len() - 1).collect();
                nested.push(nest(AxisCells {
                    axis: axis.clone(),
                    cells: NonEmpty::new(first, rest),
                }));
            }
            level = nested;
        }
        // The outermost axis nests every cell into one value.
        Ok(level.swap_remove(0))
    }
}

/// The position on `axis` of the entry `key` names.
fn position(axis: &IndexAxis, key: &MapEntryKey) -> Result<usize, MapLayoutError> {
    let entry = match key {
        MapEntryKey::IndexVariant(variant) => (axis.index().declared_resolved()
            == Some(variant.variant.index()))
        .then(|| IndexEntryKey::named(variant.variant.variant().clone())),
        MapEntryKey::FinitePosition { position, .. } => {
            Some(IndexEntryKey::position(position.value))
        }
    };
    entry
        .and_then(|entry| axis.position(&entry))
        .ok_or_else(|| MapLayoutError::KeyOutsideAxis {
            axis: Box::new(axis.index().clone()),
        })
}

#[cfg(test)]
mod tests {
    use crate::dag_id::DagId;
    use crate::hir::expr::MapEntryKey;
    use crate::semantic::index_axis::IndexAxis;
    use crate::semantic::index_def::FiniteIndex;
    use crate::syntax::non_empty::NonEmpty;
    use crate::syntax::span::{Span, Spanned};

    use super::{AxisCells, MapLayout, MapLayoutError, Placement};

    fn fin(size: u64) -> IndexAxis {
        IndexAxis::finite(FiniteIndex::try_from_u64(size).unwrap()).unwrap()
    }

    fn at(size: u64, position: u64) -> MapEntryKey {
        MapEntryKey::FinitePosition {
            size,
            position: Spanned::new(position, Span::new(0, 1)),
        }
    }

    fn keys(positions: &[(u64, u64)]) -> NonEmpty<MapEntryKey> {
        NonEmpty::try_from_vec(
            positions
                .iter()
                .map(|(size, position)| at(*size, *position))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn entries_are_placed_row_major_and_evaluated_by_group() {
        let entries = [
            keys(&[(2, 1), (2, 0)]),
            keys(&[(2, 0), (2, 1)]),
            keys(&[(2, 1), (2, 1)]),
            keys(&[(2, 0), (2, 0)]),
        ];
        let layout =
            MapLayout::try_new(vec![fin(2), fin(2)], &entries.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(layout.axes().len(), 2);
        assert_eq!(layout.cells(), 4);
        // Grouped by the outer key; written order within a group.
        assert_eq!(
            layout.placements(),
            [
                Placement { entry: 1, cell: 1 },
                Placement { entry: 3, cell: 0 },
                Placement { entry: 0, cell: 2 },
                Placement { entry: 2, cell: 3 },
            ]
        );
        let single = [keys(&[(3, 2)]), keys(&[(3, 0)]), keys(&[(3, 1)])];
        let layout = MapLayout::try_new(vec![fin(3)], &single.iter().collect::<Vec<_>>()).unwrap();
        // One axis: written order.
        assert_eq!(
            layout.placements(),
            [
                Placement { entry: 0, cell: 2 },
                Placement { entry: 1, cell: 0 },
                Placement { entry: 2, cell: 1 },
            ]
        );
    }

    /// A filled cell or axis level, rendered for comparison.
    fn rendered(level: AxisCells<String>) -> String {
        let (axis, cells) = level.into_parts();
        format!("{}[{}]", axis.len(), cells.as_slice().join(","))
    }

    #[test]
    fn filling_nests_cells_row_major_and_evaluates_by_group() {
        let entries = [
            keys(&[(2, 1), (3, 0)]),
            keys(&[(2, 0), (3, 2)]),
            keys(&[(2, 1), (3, 2)]),
            keys(&[(2, 0), (3, 0)]),
            keys(&[(2, 1), (3, 1)]),
            keys(&[(2, 0), (3, 1)]),
        ];
        let layout =
            MapLayout::try_new(vec![fin(2), fin(3)], &entries.iter().collect::<Vec<_>>()).unwrap();
        let mut order = Vec::new();
        let filled = layout
            .fill(
                |entry| {
                    order.push(entry);
                    Ok::<_, ()>(format!("e{entry}"))
                },
                rendered,
            )
            .unwrap();
        assert_eq!(filled, "2[3[e3,e5,e1],3[e0,e4,e2]]");
        assert_eq!(order, [1, 3, 5, 0, 2, 4]);
        let single = [keys(&[(1, 0)])];
        let layout = MapLayout::try_new(vec![fin(1)], &single.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(
            layout.fill(|entry| Ok::<_, ()>(format!("e{entry}")), rendered),
            Ok("1[e0]".to_owned())
        );
    }

    #[test]
    fn filling_stops_at_the_first_failed_entry() {
        let entries = [keys(&[(2, 1)]), keys(&[(2, 0)])];
        let layout = MapLayout::try_new(vec![fin(2)], &entries.iter().collect::<Vec<_>>()).unwrap();
        let mut computed = Vec::new();
        let result = layout.fill(
            |entry| {
                computed.push(entry);
                if entry == 0 {
                    Err(entry)
                } else {
                    Ok(String::new())
                }
            },
            rendered,
        );
        assert_eq!(result, Err(0));
        assert_eq!(computed, [0]);
    }

    #[test]
    fn layouts_admit_only_entries_covering_every_cell_once() {
        let one = keys(&[(2, 0)]);
        let other = keys(&[(2, 1)]);
        let outside = keys(&[(2, 2)]);
        let pair = keys(&[(2, 0), (2, 0)]);
        assert_eq!(
            MapLayout::try_new(Vec::new(), &[&one]).unwrap_err(),
            MapLayoutError::NoAxis
        );
        assert_eq!(
            MapLayout::try_new(vec![fin(2)], &[&pair]).unwrap_err(),
            MapLayoutError::Arity { keys: 2, axes: 1 }
        );
        assert!(matches!(
            MapLayout::try_new(vec![fin(2)], &[&one, &outside]).unwrap_err(),
            MapLayoutError::KeyOutsideAxis { .. }
        ));
        assert_eq!(
            MapLayout::try_new(vec![fin(2)], &[&one]).unwrap_err(),
            MapLayoutError::Coverage {
                filled: 1,
                cells: 2
            }
        );
        assert_eq!(
            MapLayout::try_new(vec![fin(2)], &[&one, &one, &other]).unwrap_err(),
            MapLayoutError::Coverage {
                filled: 2,
                cells: 2
            }
        );
        let named = IndexAxis::named_for_test(DagId::root_in_package("map", "main"), "P", &["A"]);
        assert!(matches!(
            MapLayout::try_new(vec![named], &[&one]).unwrap_err(),
            MapLayoutError::KeyOutsideAxis { .. }
        ));
    }
}
