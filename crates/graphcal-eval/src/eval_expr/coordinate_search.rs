//! Binary search of a coordinate axis for the key forms `floor_key`,
//! `ceil_key` and `nearest_key`.
//!
//! The coordinates of an axis are finite and strictly monotonic (ascending or
//! descending), so the coordinates at or below a target form one contiguous
//! run of positions and those at or above it another. Each search inspects
//! only the boundaries of those runs.

use std::ops::Range;

use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::tir::texpr::CoordinateSearch;

/// The position `search` selects among strictly monotonic `coordinates` for
/// `quantity`; `None` when no coordinate qualifies.
///
/// - `Floor`: the greatest coordinate at or below `quantity`.
/// - `Ceil`: the smallest coordinate at or above `quantity`.
/// - `Nearest`: the coordinate whose binary64 distance `|c - quantity|` is
///   smallest, ties toward the axis start.
pub(super) fn search_position(
    search: CoordinateSearch,
    coordinates: &[FiniteQuantity],
    quantity: f64,
) -> Option<usize> {
    let at = |position: usize| coordinates[position].get();
    let distance = |position: usize| (at(position) - quantity).abs();
    let ascending = coordinates.len() < 2 || at(0) < at(1);
    let boundary = |predicate: &dyn Fn(f64) -> bool| {
        coordinates.partition_point(|coordinate| predicate(coordinate.get()))
    };
    // The runs of positions whose coordinates are at or below, and at or
    // above, `quantity`; and within each run the position closest to it.
    let (below, above): (Range<usize>, Range<usize>) = if ascending {
        (
            0..boundary(&|coordinate| coordinate <= quantity),
            boundary(&|coordinate| coordinate < quantity)..coordinates.len(),
        )
    } else {
        (
            boundary(&|coordinate| coordinate > quantity)..coordinates.len(),
            0..boundary(&|coordinate| coordinate >= quantity),
        )
    };
    let nonempty = |run: &Range<usize>| (!run.is_empty()).then_some(());
    let floor = nonempty(&below).map(|()| {
        if ascending {
            below.end.saturating_sub(1)
        } else {
            below.start
        }
    });
    let ceil = nonempty(&above).map(|()| {
        if ascending {
            above.start
        } else {
            above.end.saturating_sub(1)
        }
    });
    match search {
        CoordinateSearch::Floor => floor,
        CoordinateSearch::Ceil => ceil,
        CoordinateSearch::Nearest => {
            // Rounding can give several coordinates of a run the same
            // distance as its closest one; they sit next to it, on the side
            // toward the run's start when the run approaches `quantity` with
            // rising positions.
            let earliest = |run: &Range<usize>, closest: usize| {
                if closest.saturating_add(1) == run.end {
                    let minimum = distance(closest);
                    run.start
                        .saturating_add(coordinates[run.clone()].partition_point(|coordinate| {
                            (coordinate.get() - quantity).abs() > minimum
                        }))
                } else {
                    closest
                }
            };
            let candidates = [
                floor.map(|closest| earliest(&below, closest)),
                ceil.map(|closest| earliest(&above, closest)),
            ];
            candidates
                .into_iter()
                .flatten()
                .min_by(|lhs, rhs| distance(*lhs).total_cmp(&distance(*rhs)).then(lhs.cmp(rhs)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The linear scan the binary search replaced.
    fn linear(search: CoordinateSearch, coordinates: &[f64], quantity: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (position, &coordinate) in coordinates.iter().enumerate() {
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
                (Some((_, incumbent)), CoordinateSearch::Nearest) => {
                    (coordinate - quantity).abs() < (incumbent - quantity).abs()
                }
                (Some((_, incumbent)), CoordinateSearch::Floor) => coordinate > *incumbent,
                (Some((_, incumbent)), CoordinateSearch::Ceil) => coordinate < *incumbent,
            };
            if better {
                best = Some((position, coordinate));
            }
        }
        best.map(|(position, _)| position)
    }

    fn finite(values: &[f64]) -> Vec<FiniteQuantity> {
        values
            .iter()
            .map(|value| FiniteQuantity::try_new(*value).unwrap())
            .collect()
    }

    const SEARCHES: [CoordinateSearch; 3] = [
        CoordinateSearch::Floor,
        CoordinateSearch::Ceil,
        CoordinateSearch::Nearest,
    ];

    fn assert_matches_linear(coordinates: &[f64], quantity: f64) {
        for search in SEARCHES {
            assert_eq!(
                search_position(search, &finite(coordinates), quantity),
                linear(search, coordinates, quantity),
                "{search:?} of {quantity:e} in {coordinates:?}"
            );
        }
    }

    #[test]
    fn searches_both_directions_and_both_ends() {
        let ascending = [0.0, 1.0, 2.0, 3.0];
        let descending = [3.0, 2.0, 1.0, 0.0];
        for quantity in [-1.0, 0.0, 0.5, 1.5, 2.0, 3.0, 4.0] {
            assert_matches_linear(&ascending, quantity);
            assert_matches_linear(&descending, quantity);
        }
        assert_eq!(
            search_position(CoordinateSearch::Nearest, &finite(&ascending), 1.5),
            Some(1),
            "a midpoint tie goes toward the axis start"
        );
        assert_eq!(
            search_position(CoordinateSearch::Nearest, &finite(&descending), 1.5),
            Some(1),
            "a midpoint tie goes toward the axis start"
        );
        assert_eq!(
            search_position(CoordinateSearch::Floor, &finite(&ascending), -1.0),
            None
        );
        assert_eq!(
            search_position(CoordinateSearch::Ceil, &finite(&descending), 4.0),
            None
        );
        assert_matches_linear(&[5.0], 5.0);
        assert_matches_linear(&[5.0], -5.0);
    }

    #[test]
    fn rounded_distance_ties_go_toward_the_axis_start() {
        // Far from the axis every distance rounds to the same binary64 value.
        for quantity in [1.0e20, -1.0e20] {
            assert_matches_linear(&[1.0, 2.0, 3.0, 4.0], quantity);
            assert_matches_linear(&[4.0, 3.0, 2.0, 1.0], quantity);
        }
    }

    proptest! {
        #[test]
        fn matches_the_linear_scan(
            start in -1.0e3_f64..1.0e3,
            steps in proptest::collection::vec(1.0e-3_f64..1.0e2, 0..24),
            descending in any::<bool>(),
            quantity in prop_oneof![-1.0e4_f64..1.0e4, Just(1.0e300), Just(-1.0e300)],
        ) {
            let mut coordinates = steps.iter().scan(start, |coordinate, step| {
                *coordinate += step;
                Some(*coordinate)
            }).collect::<Vec<_>>();
            coordinates.insert(0, start);
            coordinates.dedup();
            if descending {
                coordinates.reverse();
            }
            for search in SEARCHES {
                prop_assert_eq!(
                    search_position(search, &finite(&coordinates), quantity),
                    linear(search, &coordinates, quantity)
                );
            }
        }

        #[test]
        fn matches_the_linear_scan_at_every_coordinate(
            coordinates in proptest::collection::btree_set(-1_000_000_i32..1_000_000, 1..24),
            descending in any::<bool>(),
        ) {
            let mut coordinates = coordinates.into_iter().map(f64::from).collect::<Vec<_>>();
            if descending {
                coordinates.reverse();
            }
            for quantity in coordinates.clone() {
                for search in SEARCHES {
                    prop_assert_eq!(
                        search_position(search, &finite(&coordinates), quantity),
                        linear(search, &coordinates, quantity)
                    );
                }
            }
        }
    }
}
