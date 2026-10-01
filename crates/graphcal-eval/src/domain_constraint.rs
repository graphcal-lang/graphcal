//! Evaluated domain-bound data, independent of runtime-value interpretation.

use graphcal_compiler::semantic::time_scale::TimeScale;

/// One evaluated inclusive domain bound plus its user-facing diagnostic text.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBound<T> {
    value: T,
    display: String,
}

impl<T> ResolvedDomainBound<T> {
    pub const fn new(value: T, display: String) -> Self {
        Self { value, display }
    }

    pub const fn value(&self) -> &T {
        &self.value
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    fn map<U>(self, map: impl FnOnce(T) -> U) -> ResolvedDomainBound<U> {
        ResolvedDomainBound {
            value: map(self.value),
            display: self.display,
        }
    }
}

/// Evaluated inclusive lower and upper bounds for one constraint family.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBounds<T> {
    min: Option<ResolvedDomainBound<T>>,
    max: Option<ResolvedDomainBound<T>>,
}

impl<T> ResolvedDomainBounds<T> {
    pub const fn new(
        min: Option<ResolvedDomainBound<T>>,
        max: Option<ResolvedDomainBound<T>>,
    ) -> Self {
        Self { min, max }
    }

    pub const fn min(&self) -> Option<&ResolvedDomainBound<T>> {
        self.min.as_ref()
    }

    pub const fn max(&self) -> Option<&ResolvedDomainBound<T>> {
        self.max.as_ref()
    }

    /// Transform both bound values, keeping their diagnostic text.
    #[must_use]
    pub fn map<U>(self, map: impl Fn(T) -> U) -> ResolvedDomainBounds<U> {
        ResolvedDomainBounds {
            min: self.min.map(|bound| bound.map(&map)),
            max: self.max.map(|bound| bound.map(&map)),
        }
    }
}

/// A canonical physical instant admitted only from an epoch in the target scale.
///
/// Storing TAI duration makes ordering independent of hifitime's display scale.
/// The private carrier and validating constructor prevent cross-scale bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DomainInstant(hifitime::Duration);

impl DomainInstant {
    #[must_use]
    pub const fn duration(self) -> hifitime::Duration {
        self.0
    }

    pub(crate) fn from_epoch(
        epoch: hifitime::Epoch,
        expected_scale: TimeScale,
    ) -> Result<Self, DomainInstantError> {
        if epoch.time_scale != expected_scale.to_hifitime() {
            return Err(DomainInstantError::ScaleMismatch {
                expected: expected_scale,
                actual: epoch.time_scale,
            });
        }
        Ok(Self(epoch.to_tai_duration()))
    }
}

/// Failure to canonicalize a datetime value for a same-scale domain constraint.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DomainInstantError {
    #[error("expected time scale {expected}, got {actual:?}")]
    ScaleMismatch {
        expected: TimeScale,
        actual: hifitime::TimeScale,
    },
}

#[derive(Debug, Clone)]
enum ResolvedDomainConstraintKind {
    Quantity(ResolvedDomainBounds<f64>),
    Int(ResolvedDomainBounds<i64>),
    Datetime {
        scale: TimeScale,
        bounds: ResolvedDomainBounds<DomainInstant>,
    },
}

/// An evaluated constraint whose bound representation matches its value family.
///
/// The private carrier prevents mixing quantity, integer, and datetime bounds.
/// Integers remain `i64`; datetime instants are canonicalized only after checking
/// their declared scale. Value checking is a separate interpreter operation.
#[derive(Debug, Clone)]
pub struct ResolvedDomainConstraint {
    kind: ResolvedDomainConstraintKind,
}

/// Read-only family-preserving view for validation and interface projection.
#[derive(Clone, Copy)]
pub enum ResolvedDomainConstraintRef<'constraint> {
    Quantity(&'constraint ResolvedDomainBounds<f64>),
    Int(&'constraint ResolvedDomainBounds<i64>),
    Datetime {
        scale: TimeScale,
        bounds: &'constraint ResolvedDomainBounds<DomainInstant>,
    },
}

impl ResolvedDomainConstraint {
    #[must_use]
    pub const fn quantity(bounds: ResolvedDomainBounds<f64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Quantity(bounds),
        }
    }

    #[must_use]
    pub const fn int(bounds: ResolvedDomainBounds<i64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Int(bounds),
        }
    }

    #[must_use]
    pub const fn as_ref(&self) -> ResolvedDomainConstraintRef<'_> {
        match &self.kind {
            ResolvedDomainConstraintKind::Quantity(bounds) => {
                ResolvedDomainConstraintRef::Quantity(bounds)
            }
            ResolvedDomainConstraintKind::Int(bounds) => ResolvedDomainConstraintRef::Int(bounds),
            ResolvedDomainConstraintKind::Datetime { scale, bounds } => {
                ResolvedDomainConstraintRef::Datetime {
                    scale: *scale,
                    bounds,
                }
            }
        }
    }

    /// Datetime bounds of a `Datetime<scale>` value, each instant admitted
    /// from an epoch in `scale`.
    #[must_use]
    pub const fn datetime(scale: TimeScale, bounds: ResolvedDomainBounds<DomainInstant>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Datetime { scale, bounds },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_instant_admits_only_an_epoch_in_its_scale() {
        let tt_epoch =
            hifitime::Epoch::maybe_from_gregorian(2024, 1, 1, 0, 0, 0, 0, hifitime::TimeScale::TT)
                .unwrap();
        let error = DomainInstant::from_epoch(tt_epoch, TimeScale::UTC).unwrap_err();
        assert!(matches!(error, DomainInstantError::ScaleMismatch { .. }));
        let instant = DomainInstant::from_epoch(tt_epoch, TimeScale::TT).unwrap();
        assert_eq!(instant.duration(), tt_epoch.to_tai_duration());
    }

    #[test]
    fn mapped_bounds_keep_their_diagnostic_text() {
        let bounds = ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(1_i64, "one".to_owned())),
            Some(ResolvedDomainBound::new(2_i64, "two".to_owned())),
        )
        .map(|value| value * 10);
        let (min, max) = (bounds.min().unwrap(), bounds.max().unwrap());
        assert_eq!((*min.value(), min.display()), (10, "one"));
        assert_eq!((*max.value(), max.display()), (20, "two"));
    }

    #[test]
    fn integer_view_preserves_exact_bounds_and_diagnostic_spelling() {
        let constraint = ResolvedDomainConstraint::int(ResolvedDomainBounds::new(
            None,
            Some(ResolvedDomainBound::new(
                i64::MAX,
                "maximum integer".to_owned(),
            )),
        ));
        match constraint.as_ref() {
            ResolvedDomainConstraintRef::Int(bounds) => {
                assert!(bounds.min().is_none());
                let max = bounds.max().unwrap();
                assert_eq!(*max.value(), i64::MAX);
                assert_eq!(max.display(), "maximum integer");
            }
            _ => panic!("integer bounds changed family"),
        }
    }
}
