//! Evaluated domain-bound data, independent of runtime-value interpretation.

use graphcal_compiler::registry::time_scale::TimeScale;

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

    fn try_map<U, E>(
        self,
        map: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<ResolvedDomainBound<U>, E> {
        Ok(ResolvedDomainBound {
            value: map(self.value)?,
            display: self.display,
        })
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
}

/// A canonical physical instant admitted only from an epoch in the target scale.
///
/// Storing TAI duration makes ordering independent of hifitime's display scale.
/// The private carrier and validating constructor prevent cross-scale bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DomainInstant(hifitime::Duration);

impl DomainInstant {
    pub(crate) const fn duration(self) -> hifitime::Duration {
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
    pub const fn quantity(bounds: ResolvedDomainBounds<f64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Quantity(bounds),
        }
    }

    pub const fn int(bounds: ResolvedDomainBounds<i64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Int(bounds),
        }
    }

    pub(crate) const fn as_ref(&self) -> ResolvedDomainConstraintRef<'_> {
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

    pub fn datetime(
        scale: TimeScale,
        bounds: ResolvedDomainBounds<hifitime::Epoch>,
    ) -> Result<Self, DomainInstantError> {
        let min = bounds
            .min
            .map(|bound| bound.try_map(|epoch| DomainInstant::from_epoch(epoch, scale)))
            .transpose()?;
        let max = bounds
            .max
            .map(|bound| bound.try_map(|epoch| DomainInstant::from_epoch(epoch, scale)))
            .transpose()?;
        Ok(Self {
            kind: ResolvedDomainConstraintKind::Datetime {
                scale,
                bounds: ResolvedDomainBounds::new(min, max),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_constraint_constructor_rejects_a_cross_scale_epoch() {
        let tt_epoch =
            hifitime::Epoch::maybe_from_gregorian(2024, 1, 1, 0, 0, 0, 0, hifitime::TimeScale::TT)
                .unwrap();
        let bounds = ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(tt_epoch, tt_epoch.to_string())),
            None,
        );
        let error = ResolvedDomainConstraint::datetime(TimeScale::UTC, bounds).unwrap_err();
        assert!(matches!(error, DomainInstantError::ScaleMismatch { .. }));
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
