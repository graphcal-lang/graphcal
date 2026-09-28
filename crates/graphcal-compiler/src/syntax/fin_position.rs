//! A position on a structural `Fin(N)` index axis.
//!
//! Table sugar over `Fin(N)` axes addresses cells by position (`#0`, `#1`, …)
//! instead of by variant name. A [`FinPosition`] keeps the axis cardinality
//! with the position and can only be built inside the axis, so a key that
//! points past its axis is unrepresentable.

use std::fmt;

/// Position `position` on the axis `Fin(cardinality)`; always
/// `position < cardinality`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FinPosition {
    cardinality: u64,
    position: u64,
}

/// A position outside its `Fin(cardinality)` axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("position #{position} is out of range for Fin({cardinality})")]
pub struct FinPositionOutOfRange {
    pub cardinality: u64,
    pub position: u64,
}

impl FinPosition {
    /// Position `position` on `Fin(cardinality)`.
    ///
    /// # Errors
    ///
    /// Returns [`FinPositionOutOfRange`] unless `position < cardinality`.
    pub const fn try_new(cardinality: u64, position: u64) -> Result<Self, FinPositionOutOfRange> {
        if position < cardinality {
            Ok(Self {
                cardinality,
                position,
            })
        } else {
            Err(FinPositionOutOfRange {
                cardinality,
                position,
            })
        }
    }

    /// Cardinality `N` of the `Fin(N)` axis.
    #[must_use]
    pub const fn cardinality(self) -> u64 {
        self.cardinality
    }

    /// Zero-based position on the axis.
    #[must_use]
    pub const fn position(self) -> u64 {
        self.position
    }
}

/// Renders the source spelling `#position`.
impl fmt::Display for FinPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_inside_the_axis_are_accepted() {
        let position = FinPosition::try_new(3, 2).expect("inside Fin(3)");
        assert_eq!(position.cardinality(), 3);
        assert_eq!(position.position(), 2);
        assert_eq!(position.to_string(), "#2");
    }

    #[test]
    fn positions_at_or_past_the_cardinality_are_rejected() {
        assert_eq!(
            FinPosition::try_new(3, 3),
            Err(FinPositionOutOfRange {
                cardinality: 3,
                position: 3
            })
        );
        assert!(FinPosition::try_new(0, 0).is_err());
    }
}
