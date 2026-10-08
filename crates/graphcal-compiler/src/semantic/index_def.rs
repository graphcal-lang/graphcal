use std::fmt;
use std::num::NonZeroUsize;

use thiserror::Error;

use crate::dimension::Dimension;
use crate::semantic::unit_scale::PositiveFiniteScale;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::non_empty::NonEmptyUnique;

/// Largest concrete index that Graphcal will materialize eagerly.
///
/// Index variants and indexed values are currently allocated eagerly. Keeping
/// this bound in the semantic cardinality type prevents an otherwise valid
/// source literal from turning into an unbounded allocation request.
pub const MAX_INDEX_CARDINALITY: usize = 1_000_000;

/// Validated, non-empty cardinality for an eagerly materialized index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IndexCardinality(NonZeroUsize);

impl IndexCardinality {
    /// The singleton cardinality.
    pub const ONE: Self = Self(NonZeroUsize::MIN);

    /// Validate a source/runtime cardinality before any allocation.
    ///
    /// # Errors
    ///
    /// Returns an error for zero, values that do not fit the target, or values
    /// above Graphcal's practical eager-allocation limit.
    pub fn try_from_u64(value: u64) -> Result<Self, IndexCardinalityError> {
        if value == 0 {
            return Err(IndexCardinalityError::Empty);
        }
        let value =
            usize::try_from(value).map_err(|_| IndexCardinalityError::DoesNotFitUsize { value })?;
        if value > MAX_INDEX_CARDINALITY {
            return Err(IndexCardinalityError::TooLarge {
                value,
                maximum: MAX_INDEX_CARDINALITY,
            });
        }
        let value = NonZeroUsize::new(value).ok_or(IndexCardinalityError::Empty)?;
        Ok(Self(value))
    }

    /// Return the validated cardinality.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }

    /// Return the validated non-zero cardinality.
    #[must_use]
    pub const fn non_zero(self) -> NonZeroUsize {
        self.0
    }
}

/// Failure to validate a concrete index cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IndexCardinalityError {
    #[error("indexes must contain at least one element")]
    Empty,
    #[error("index cardinality {value} does not fit in usize on this target")]
    DoesNotFitUsize { value: u64 },
    #[error("index cardinality {value} exceeds the practical limit of {maximum} elements")]
    TooLarge { value: usize, maximum: usize },
}

impl IndexCardinalityError {
    /// Render this failure for a structural `Fin(N)` axis.
    #[must_use]
    pub fn describe_finite_index(self) -> String {
        match self {
            Self::Empty => {
                "Fin(0) is not allowed; indexes must contain at least one element".to_string()
            }
            Self::DoesNotFitUsize { value } => {
                format!("Fin cardinality {value} does not fit in usize on this target")
            }
            Self::TooLarge { value, maximum } => {
                format!("Fin cardinality {value} exceeds the practical limit of {maximum} elements")
            }
        }
    }
}

/// Direct coordinate-generation rule for a concrete coordinate index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoordinateSpacing {
    /// Exact increment supplied by `range(..., step: ...)`.
    Step { step: f64 },
    /// Exact count supplied by `linspace(..., points: ...)`.
    Linspace,
}

/// Display unit attached to a coordinate axis.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinateDisplayUnit {
    /// Display unit label (e.g., `"s"`) for formatting coordinate values.
    pub label: Option<String>,
    /// Scale factor from SI to display unit: `display_value = si_value / scale`.
    pub scale: PositiveFiniteScale,
}

impl CoordinateDisplayUnit {
    /// SI display: no unit label and a unit scale.
    pub const SI: Self = Self {
        label: None,
        scale: PositiveFiniteScale::ONE,
    };
}

/// A concrete coordinate index whose SI coordinates are finite and strictly
/// monotonic in binary64.
///
/// The invariant is established by [`Self::try_range`] and
/// [`Self::try_linspace`]; the fields are private so no consumer can observe a
/// coordinate axis that was not validated.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinateIndexData {
    start: f64,
    end: f64,
    spacing: CoordinateSpacing,
    cardinality: IndexCardinality,
    dimension: Dimension,
    display: CoordinateDisplayUnit,
}

/// Failure to construct a [`CoordinateIndexData`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum CoordinateIndexError {
    #[error("coordinate bounds and step must be finite")]
    NonFiniteParameter,
    #[error("step must be nonzero")]
    ZeroStep,
    #[error("step {step} moves away from endpoint {end}")]
    StepAwayFromEndpoint { step: f64, end: f64 },
    #[error("range interval count is not a finite non-negative value")]
    NonFiniteIntervalCount,
    #[error("step {step} does not land on endpoint {end}")]
    StepMissesEndpoint { step: f64, end: f64 },
    #[error(
        "range requires {points} coordinate points, which exceeds the practical limit of {}",
        MAX_INDEX_CARDINALITY
    )]
    RangeTooLarge { points: f64 },
    #[error("linspace requires at least one point")]
    EmptyLinspace,
    #[error(transparent)]
    LinspacePointCount(IndexCardinalityError),
    #[error("linspace with one point requires identical start and end values")]
    SingletonLinspaceWithDistinctEndpoints,
    #[error("linspace with two or more points requires distinct endpoints")]
    LinspaceWithIdenticalEndpoints,
    #[error(
        "coordinates at positions {previous} and {position} are duplicate or non-monotonic in binary64"
    )]
    NonMonotonicCoordinates { previous: usize, position: usize },
}

impl CoordinateIndexError {
    /// Actionable advice for this failure, rendered at the diagnostic boundary.
    #[must_use]
    pub fn help(&self) -> String {
        match self {
            Self::NonFiniteParameter | Self::NonFiniteIntervalCount => {
                "choose finite bounds and a finite nonzero step with matching direction".to_string()
            }
            Self::ZeroStep => "use an explicitly positive or negative quantity step".to_string(),
            Self::StepAwayFromEndpoint { .. } => "use a positive step for an ascending range and a negative step for a descending range".to_string(),
            Self::StepMissesEndpoint { .. } => "change the endpoint or use `linspace(start, end, points: N)` when the point count is authoritative".to_string(),
            Self::RangeTooLarge { .. } => {
                format!("reduce the index to at most {MAX_INDEX_CARDINALITY} points")
            }
            Self::EmptyLinspace => "use `points: 1` or greater".to_string(),
            Self::LinspacePointCount(_) => {
                format!("use a point count from 1 through {MAX_INDEX_CARDINALITY}")
            }
            Self::SingletonLinspaceWithDistinctEndpoints => {
                "set end equal to start or request at least two points".to_string()
            }
            Self::LinspaceWithIdenticalEndpoints => {
                "use `points: 1` for a singleton or choose distinct endpoints".to_string()
            }
            Self::NonMonotonicCoordinates { .. } => {
                "reduce the cardinality or increase the spacing between adjacent coordinates"
                    .to_string()
            }
        }
    }

    /// Whether this failure is about the requested `linspace` point count
    /// rather than the axis as a whole.
    #[must_use]
    pub const fn concerns_point_count(&self) -> bool {
        matches!(self, Self::EmptyLinspace | Self::LinspacePointCount(_))
    }
}

/// One centralized binary64 endpoint comparison for coordinate construction.
fn coordinate_values_equal(actual: f64, expected: f64, scale_hint: f64) -> bool {
    let scale = actual.abs().max(expected.abs()).max(scale_hint.abs());
    // Keep the tolerance relative even for coordinates near zero. A unit-scale
    // floor would accept steps that miss tiny endpoints by a large fraction.
    // The subnormal floor covers a small number of binary64 ULPs at zero.
    let tolerance = (scale * (32.0 * f64::EPSILON)).max(f64::from_bits(32));
    (actual - expected).abs() <= tolerance
}

/// Exact numeric endpoint equality after finite-value validation.
fn coordinate_endpoints_equal(start: f64, end: f64) -> bool {
    matches!(start.partial_cmp(&end), Some(std::cmp::Ordering::Equal))
}

/// Number of points of `range(start, end, step: step)` for finite arguments.
fn range_cardinality(
    start: f64,
    end: f64,
    step: f64,
) -> Result<IndexCardinality, CoordinateIndexError> {
    if step == 0.0 {
        return Err(CoordinateIndexError::ZeroStep);
    }
    if (start < end && step < 0.0) || (start > end && step > 0.0) {
        return Err(CoordinateIndexError::StepAwayFromEndpoint { step, end });
    }
    if coordinate_endpoints_equal(start, end) {
        return Ok(IndexCardinality::ONE);
    }

    let raw_intervals = (end - start) / step;
    if !raw_intervals.is_finite() || raw_intervals < 0.0 {
        return Err(CoordinateIndexError::NonFiniteIntervalCount);
    }
    let intervals = raw_intervals.round();
    let reconstructed_end = intervals.mul_add(step, start);
    if intervals < 1.0 || !coordinate_values_equal(reconstructed_end, end, intervals * step) {
        return Err(CoordinateIndexError::StepMissesEndpoint { step, end });
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "the practical limit is far below f64's exact integer range"
    )]
    if intervals >= MAX_INDEX_CARDINALITY as f64 {
        return Err(CoordinateIndexError::RangeTooLarge {
            points: intervals + 1.0,
        });
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the rounded interval count is finite, at least one, and below the practical limit"
    )]
    let intervals = intervals as usize;
    Ok(IndexCardinality(
        NonZeroUsize::MIN.saturating_add(intervals),
    ))
}

impl CoordinateIndexData {
    /// Build the axis of `range(start, end, step: step)` from SI values.
    ///
    /// The last coordinate is exactly `end`; `step` must land on it within a
    /// small relative binary64 tolerance.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite arguments, a zero or misdirected step,
    /// a step that misses the endpoint, too many points, or coordinates that
    /// are not strictly monotonic in binary64.
    pub fn try_range(
        start: f64,
        end: f64,
        step: f64,
        dimension: Dimension,
        display: CoordinateDisplayUnit,
    ) -> Result<Self, CoordinateIndexError> {
        if !(start.is_finite() && end.is_finite() && step.is_finite()) {
            return Err(CoordinateIndexError::NonFiniteParameter);
        }
        let cardinality = range_cardinality(start, end, step)?;
        Self {
            start,
            end,
            spacing: CoordinateSpacing::Step { step },
            cardinality,
            dimension,
            display,
        }
        .validated()
    }

    /// Build the axis of `linspace(start, end, points: points)` from SI values.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite endpoints, an invalid point count,
    /// endpoints inconsistent with the point count, or coordinates that are
    /// not strictly monotonic in binary64.
    pub fn try_linspace(
        start: f64,
        end: f64,
        points: u64,
        dimension: Dimension,
        display: CoordinateDisplayUnit,
    ) -> Result<Self, CoordinateIndexError> {
        if !(start.is_finite() && end.is_finite()) {
            return Err(CoordinateIndexError::NonFiniteParameter);
        }
        let cardinality = IndexCardinality::try_from_u64(points).map_err(|error| match error {
            IndexCardinalityError::Empty => CoordinateIndexError::EmptyLinspace,
            error => CoordinateIndexError::LinspacePointCount(error),
        })?;
        match (cardinality.get(), coordinate_endpoints_equal(start, end)) {
            (1, false) => {
                return Err(CoordinateIndexError::SingletonLinspaceWithDistinctEndpoints);
            }
            (2.., true) => return Err(CoordinateIndexError::LinspaceWithIdenticalEndpoints),
            _ => {}
        }
        Self {
            start,
            end,
            spacing: CoordinateSpacing::Linspace,
            cardinality,
            dimension,
            display,
        }
        .validated()
    }

    /// Check that every generated coordinate is strictly monotonic.
    ///
    /// Callers only build multi-point axes with finite, distinct endpoints, so
    /// every generated coordinate lies between them and is finite; the
    /// comparison also rejects any NaN.
    fn validated(self) -> Result<Self, CoordinateIndexError> {
        let ascending = self.end > self.start;
        let first = self.coordinate_value(0);
        (1..self.cardinality.get()).try_fold(first, |previous, position| {
            let current = self.coordinate_value(position);
            let monotonic = if ascending {
                current > previous
            } else {
                current < previous
            };
            if !monotonic {
                return Err(CoordinateIndexError::NonMonotonicCoordinates {
                    previous: position - 1,
                    position,
                });
            }
            Ok(current)
        })?;
        Ok(self)
    }

    /// SI value of the first coordinate.
    #[must_use]
    pub const fn start(&self) -> f64 {
        self.start
    }

    /// SI value of the last coordinate.
    #[must_use]
    pub const fn end(&self) -> f64 {
        self.end
    }

    /// The generation rule the axis was declared with.
    #[must_use]
    pub const fn spacing(&self) -> CoordinateSpacing {
        self.spacing
    }

    /// Dimension shared by every coordinate.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// Display unit used when formatting coordinates.
    #[must_use]
    pub const fn display(&self) -> &CoordinateDisplayUnit {
        &self.display
    }

    /// Returns the SI coordinate at position `i`, computed directly rather than
    /// by cumulative addition.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "validated index cardinalities are far below f64's exact integer range"
    )]
    pub fn coordinate_value(&self, i: usize) -> f64 {
        match self.spacing {
            CoordinateSpacing::Step { .. } if i == self.cardinality.get() - 1 => self.end,
            CoordinateSpacing::Step { step } => (i as f64).mul_add(step, self.start),
            CoordinateSpacing::Linspace => match (i, self.cardinality.get()) {
                (0, _) => self.start,
                (position, count) if position + 1 == count => self.end,
                (position, count) => {
                    let t = position as f64 / (count - 1) as f64;
                    (1.0 - t).mul_add(self.start, t * self.end)
                }
            },
        }
    }

    /// Returns the number of coordinates in this index.
    #[must_use]
    pub const fn cardinality(&self) -> usize {
        self.cardinality.get()
    }
}

const fn index_position_key(position: usize) -> IndexEntryKey {
    IndexEntryKey::position(position as u64)
}

/// Coarse semantic category used when checking whether one index may bind another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexCategory {
    Named,
    Coordinate,
    Finite,
}

impl fmt::Display for IndexCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Named => "named",
            Self::Coordinate => "coordinate",
            Self::Finite => "finite",
        })
    }
}

/// Semantic category accepted by an index binding contract.
///
/// `Discrete` is the capability exposed by an unconstrained required index:
/// callers may supply either a label-bearing named axis or a structural
/// `Fin(N)` axis. Concrete named declarations remain nominal and therefore use
/// the narrower `Named` requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexBindingCategory {
    /// Only a concrete or forwarded named axis is accepted.
    Named,
    /// A named axis or structural finite axis is accepted.
    Discrete,
    /// Only a dimension-compatible coordinate axis is accepted.
    Coordinate,
}

impl fmt::Display for IndexBindingCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Named => "named",
            Self::Discrete => "named or finite",
            Self::Coordinate => "coordinate",
        })
    }
}

/// A required or overridable declared index's typed binding contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBindingContract {
    /// A concrete named declaration can only retain another named identity.
    Named,
    /// An unconstrained required index accepts named and structural finite axes.
    Discrete,
    /// A coordinate declaration requires an axis with the same dimension.
    Coordinate { dimension: Dimension },
}

impl IndexBindingContract {
    /// Return the semantic index category required by this contract.
    #[must_use]
    pub const fn category(&self) -> IndexBindingCategory {
        match self {
            Self::Named => IndexBindingCategory::Named,
            Self::Discrete => IndexBindingCategory::Discrete,
            Self::Coordinate { .. } => IndexBindingCategory::Coordinate,
        }
    }

    /// Check a candidate index without erasing its category or coordinate dimension.
    ///
    /// Both concrete and required declared indexes are valid candidates. Allowing a
    /// compatible required candidate lets an outer generic DAG forward its own input
    /// to an inner DAG; the outer include chain must eventually provide a concrete axis.
    pub fn validate(&self, candidate: &IndexDef) -> Result<(), IndexBindingContractError> {
        let found = candidate.category();
        let kind_matches = matches!(
            (self.category(), found),
            (IndexBindingCategory::Named, IndexCategory::Named)
                | (
                    IndexBindingCategory::Discrete,
                    IndexCategory::Named | IndexCategory::Finite
                )
                | (IndexBindingCategory::Coordinate, IndexCategory::Coordinate)
        );
        if !kind_matches {
            return Err(IndexBindingContractError::KindMismatch {
                expected: self.category(),
                found,
            });
        }

        match (self, candidate.coordinate_dimension()) {
            (
                Self::Coordinate {
                    dimension: expected,
                },
                Some(found),
            ) if expected != found => Err(IndexBindingContractError::DimensionMismatch {
                expected: expected.clone(),
                found: found.clone(),
            }),
            _ => Ok(()),
        }
    }
}

/// Failure to satisfy a typed index binding contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexBindingContractError {
    #[error("index categories do not match")]
    KindMismatch {
        expected: IndexBindingCategory,
        found: IndexCategory,
    },
    #[error("coordinate-index dimensions do not match")]
    DimensionMismatch {
        expected: Dimension,
        found: Dimension,
    },
}

/// A concrete index: its elements are known.
#[derive(Debug, Clone, PartialEq)]
pub enum ConcreteIndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named {
        variants: NonEmptyUnique<IndexVariantName>,
    },
    /// A coordinate index created by `range(..., step: ...)` or
    /// `linspace(..., points: ...)`.
    Coordinate(CoordinateIndexData),
    /// A structural finite index `Fin(N)` with labels `0` through `N - 1`.
    Finite { index: FiniteIndex },
}

impl ConcreteIndexKind {
    /// Number of elements of this concrete index.
    #[must_use]
    pub const fn cardinality(&self) -> IndexCardinality {
        match self {
            Self::Named { variants } => IndexCardinality(variants.len()),
            Self::Coordinate(data) => data.cardinality,
            Self::Finite { index } => index.cardinality(),
        }
    }

    /// Typed entry keys in index order.
    ///
    /// Named indexes carry declared labels; coordinate and finite indexes carry
    /// numeric positions.
    #[must_use]
    pub fn entry_keys(&self) -> Vec<IndexEntryKey> {
        match self {
            Self::Named { variants } => {
                variants.iter().cloned().map(IndexEntryKey::named).collect()
            }
            Self::Coordinate(_) | Self::Finite { .. } => (0..self.cardinality().get())
                .map(index_position_key)
                .collect(),
        }
    }

    const fn category(&self) -> IndexCategory {
        match self {
            Self::Named { .. } => IndexCategory::Named,
            Self::Coordinate(_) => IndexCategory::Coordinate,
            Self::Finite { .. } => IndexCategory::Finite,
        }
    }
}

/// A required index: its elements are supplied by a parameterized include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredIndexKind {
    /// Required named index: accepts named and structural finite axes.
    Named,
    /// Required coordinate index with a dimension constraint.
    Coordinate { dimension: Dimension },
}

impl RequiredIndexKind {
    const fn category(&self) -> IndexCategory {
        match self {
            Self::Named => IndexCategory::Named,
            Self::Coordinate { .. } => IndexCategory::Coordinate,
        }
    }
}

/// Closed semantic categories of indexes, split by whether the elements are
/// already known.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexKind {
    Concrete(ConcreteIndexKind),
    Required(RequiredIndexKind),
}

/// A concrete or required index with its ordered elements.
///
/// `name` is the definition's registry identity: a declared name, or the
/// structural identity of a compiler-generated `Fin(N)` axis.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexDef {
    pub name: IndexBindingTarget,
    pub kind: IndexKind,
}

impl IndexDef {
    /// Derive a structural definition from an already validated cardinality.
    #[must_use]
    pub const fn finite(index: FiniteIndex) -> Self {
        Self {
            name: IndexBindingTarget::Finite(index),
            kind: IndexKind::Concrete(ConcreteIndexKind::Finite { index }),
        }
    }

    /// Return this definition's coarse semantic category.
    #[must_use]
    pub const fn category(&self) -> IndexCategory {
        match &self.kind {
            IndexKind::Concrete(kind) => kind.category(),
            IndexKind::Required(kind) => kind.category(),
        }
    }

    /// Returns the concrete kind, or `None` for a required index.
    #[must_use]
    pub const fn concrete(&self) -> Option<&ConcreteIndexKind> {
        match &self.kind {
            IndexKind::Concrete(kind) => Some(kind),
            IndexKind::Required(_) => None,
        }
    }

    /// Returns typed entry keys in index order.
    ///
    /// Required indexes have no entries until bound.
    #[must_use]
    pub fn entry_keys(&self) -> Vec<IndexEntryKey> {
        self.concrete()
            .map_or_else(Vec::new, ConcreteIndexKind::entry_keys)
    }

    /// Returns the validated cardinality when this index is concrete.
    ///
    /// Required indexes have no cardinality until a caller binds them.
    #[must_use]
    pub fn concrete_cardinality(&self) -> Option<IndexCardinality> {
        self.concrete().map(ConcreteIndexKind::cardinality)
    }

    /// Returns coordinate data if this is a concrete coordinate index.
    #[must_use]
    pub const fn coordinate_data(&self) -> Option<&CoordinateIndexData> {
        match &self.kind {
            IndexKind::Concrete(ConcreteIndexKind::Coordinate(data)) => Some(data),
            _ => None,
        }
    }

    /// Returns the coordinate dimension of a concrete or required coordinate index.
    #[must_use]
    pub const fn coordinate_dimension(&self) -> Option<&Dimension> {
        match &self.kind {
            IndexKind::Concrete(ConcreteIndexKind::Coordinate(data)) => Some(&data.dimension),
            IndexKind::Required(RequiredIndexKind::Coordinate { dimension }) => Some(dimension),
            _ => None,
        }
    }

    /// Returns true if this is a coordinate index (concrete or required).
    #[must_use]
    pub const fn is_coordinate(&self) -> bool {
        matches!(self.category(), IndexCategory::Coordinate)
    }

    /// Returns the cardinality of a concrete structural `Fin(N)` index.
    #[must_use]
    pub const fn finite_index_size(&self) -> Option<u64> {
        match &self.kind {
            IndexKind::Concrete(ConcreteIndexKind::Finite { index }) => Some(index.size_u64()),
            _ => None,
        }
    }

    /// Returns true if this is a required index (must be bound via parameterized include).
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.kind, IndexKind::Required(_))
    }
}

// ---------------------------------------------------------------------------
// Finite structural indexes
// ---------------------------------------------------------------------------

/// Typed identity for a concrete compiler-generated structural `Fin(N)` index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiniteIndex {
    cardinality: IndexCardinality,
}

impl FiniteIndex {
    /// Create an identity from an already validated cardinality.
    #[must_use]
    pub(crate) const fn new(cardinality: IndexCardinality) -> Self {
        Self { cardinality }
    }

    /// Try to create an identity from an AST/runtime Nat cardinality.
    ///
    /// # Errors
    ///
    /// Returns an error for zero, an unrepresentable value, or a value above
    /// the practical eager-allocation limit.
    /// Use [`IndexCardinalityError::describe_finite_index`] to render the
    /// failure for a structural axis.
    pub fn try_from_u64(size: u64) -> Result<Self, IndexCardinalityError> {
        IndexCardinality::try_from_u64(size).map(Self::new)
    }

    /// Return the validated cardinality.
    #[must_use]
    pub const fn cardinality(self) -> IndexCardinality {
        self.cardinality
    }

    /// Return the cardinality as a `u64` for Nat-expression comparisons and display.
    #[must_use]
    pub(crate) const fn size_u64(self) -> u64 {
        self.cardinality.get() as u64
    }
}

/// Renders the source spelling `Fin(N)`.
impl fmt::Display for FiniteIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Fin({})", self.cardinality.get())
    }
}

/// Resolved importer-side argument supplied to an include index port.
///
/// Declared axes retain their typed declaration names. Structural axes retain
/// their validated finite identity instead of fabricating a recoverable name
/// such as `"Fin(3)"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexBindingTarget {
    /// A declared axis in the including DAG's registry.
    Declared(IndexName),
    /// A validated structural finite identity.
    Finite(FiniteIndex),
}

impl IndexBindingTarget {
    /// Return the declared target name, when the argument names a declaration.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(name) => Some(name),
            Self::Finite(_) => None,
        }
    }
}

impl fmt::Display for IndexBindingTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declared(name) => name.fmt(formatter),
            Self::Finite(index) => index.fmt(formatter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::non_empty::NonEmpty;

    fn time() -> Dimension {
        Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Time,
        ))
    }

    fn named_kind(labels: &[&str]) -> IndexKind {
        let labels = NonEmpty::try_from_vec(
            labels
                .iter()
                .map(|label| IndexVariantName::expect_valid(*label))
                .collect(),
        )
        .unwrap();
        IndexKind::Concrete(ConcreteIndexKind::Named {
            variants: NonEmptyUnique::try_from_non_empty(labels).unwrap(),
        })
    }

    fn range(start: f64, end: f64, step: f64) -> Result<CoordinateIndexData, CoordinateIndexError> {
        CoordinateIndexData::try_range(
            start,
            end,
            step,
            Dimension::dimensionless(),
            CoordinateDisplayUnit::SI,
        )
    }

    fn linspace(
        start: f64,
        end: f64,
        points: u64,
    ) -> Result<CoordinateIndexData, CoordinateIndexError> {
        CoordinateIndexData::try_linspace(
            start,
            end,
            points,
            Dimension::dimensionless(),
            CoordinateDisplayUnit::SI,
        )
    }

    fn coordinates(data: &CoordinateIndexData) -> Vec<f64> {
        (0..data.cardinality())
            .map(|position| data.coordinate_value(position))
            .collect()
    }

    #[test]
    fn range_generates_exact_endpoints_in_both_directions() {
        let ascending = range(0.0, 2.0, 1.0).unwrap();
        assert_eq!(coordinates(&ascending), vec![0.0, 1.0, 2.0]);
        assert_eq!(ascending.spacing(), CoordinateSpacing::Step { step: 1.0 });
        assert_eq!(ascending.start(), 0.0);
        assert_eq!(ascending.end(), 2.0);

        let descending = range(2.0, 0.0, -1.0).unwrap();
        assert_eq!(coordinates(&descending), vec![2.0, 1.0, 0.0]);

        // The reconstructed endpoint is within tolerance; the last coordinate
        // is the declared endpoint itself.
        let decimal = range(0.0, 0.3, 0.1).unwrap();
        assert_eq!(decimal.cardinality(), 4);
        assert_eq!(decimal.coordinate_value(3), 0.3);
    }

    #[test]
    fn range_with_identical_endpoints_is_a_singleton() {
        let singleton = range(1.5, 1.5, 0.5).unwrap();
        assert_eq!(coordinates(&singleton), vec![1.5]);
        assert_eq!(range(1.5, 1.5, -0.5).unwrap().cardinality(), 1);
    }

    #[test]
    fn range_keeps_dimension_and_display_unit() {
        let display = CoordinateDisplayUnit {
            label: Some("min".to_string()),
            scale: PositiveFiniteScale::new(60.0).unwrap(),
        };
        let data =
            CoordinateIndexData::try_range(0.0, 120.0, 60.0, time(), display.clone()).unwrap();
        assert_eq!(data.dimension(), &time());
        assert_eq!(data.display(), &display);
    }

    #[test]
    fn range_rejects_invalid_steps() {
        assert_eq!(range(0.0, 1.0, 0.0), Err(CoordinateIndexError::ZeroStep));
        assert_eq!(range(1.0, 1.0, 0.0), Err(CoordinateIndexError::ZeroStep));
        assert_eq!(
            range(0.0, 1.0, -1.0),
            Err(CoordinateIndexError::StepAwayFromEndpoint {
                step: -1.0,
                end: 1.0
            })
        );
        assert_eq!(
            range(1.0, 0.0, 1.0),
            Err(CoordinateIndexError::StepAwayFromEndpoint {
                step: 1.0,
                end: 0.0
            })
        );
        assert_eq!(
            range(0.0, 1.0, 0.3),
            Err(CoordinateIndexError::StepMissesEndpoint {
                step: 0.3,
                end: 1.0
            })
        );
        assert_eq!(
            range(0.0, 1.0, 3.0),
            Err(CoordinateIndexError::StepMissesEndpoint {
                step: 3.0,
                end: 1.0
            })
        );
    }

    #[test]
    fn coordinate_constructors_reject_non_finite_parameters() {
        for (start, end, step) in [
            (f64::NAN, 1.0, 1.0),
            (0.0, f64::INFINITY, 1.0),
            (0.0, 1.0, f64::NEG_INFINITY),
        ] {
            assert_eq!(
                range(start, end, step),
                Err(CoordinateIndexError::NonFiniteParameter)
            );
        }
        assert_eq!(
            linspace(f64::NAN, 1.0, 2),
            Err(CoordinateIndexError::NonFiniteParameter)
        );
        assert_eq!(
            linspace(0.0, f64::INFINITY, 2),
            Err(CoordinateIndexError::NonFiniteParameter)
        );
    }

    #[test]
    fn range_rejects_overflowing_interval_count() {
        assert_eq!(
            range(-1e308, 1e308, 1e-300),
            Err(CoordinateIndexError::NonFiniteIntervalCount)
        );
    }

    #[test]
    fn range_cardinality_is_bounded_by_the_practical_limit() {
        let last = 999_999.0;
        assert_eq!(
            range(0.0, last, 1.0).unwrap().cardinality(),
            MAX_INDEX_CARDINALITY
        );
        let error = range(0.0, last + 1.0, 1.0).unwrap_err();
        assert_eq!(
            error,
            CoordinateIndexError::RangeTooLarge {
                points: 1_000_001.0
            }
        );
        assert_eq!(
            error.to_string(),
            "range requires 1000001 coordinate points, which exceeds the practical limit of 1000000"
        );
        assert_eq!(error.help(), "reduce the index to at most 1000000 points");
    }

    #[test]
    fn range_rejects_coordinates_that_collapse_in_binary64() {
        // Above 2^53 only even integers are representable, so `1e16 + 1`
        // rounds onto a neighbour.
        assert!(matches!(
            range(1e16, 1e16 + 4.0, 1.0),
            Err(CoordinateIndexError::NonMonotonicCoordinates { .. })
        ));
        assert!(matches!(
            range(1e16 + 4.0, 1e16, -1.0),
            Err(CoordinateIndexError::NonMonotonicCoordinates { .. })
        ));
    }

    #[test]
    fn linspace_generates_exact_endpoints_in_both_directions() {
        let ascending = linspace(0.0, 1.0, 5).unwrap();
        assert_eq!(coordinates(&ascending), vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(ascending.spacing(), CoordinateSpacing::Linspace);

        let descending = linspace(1.0, 0.0, 3).unwrap();
        assert_eq!(coordinates(&descending), vec![1.0, 0.5, 0.0]);

        assert_eq!(coordinates(&linspace(2.0, 2.0, 1).unwrap()), vec![2.0]);
    }

    #[test]
    fn linspace_rejects_invalid_point_counts() {
        let empty = linspace(0.0, 1.0, 0).unwrap_err();
        assert_eq!(empty, CoordinateIndexError::EmptyLinspace);
        assert!(empty.concerns_point_count());
        assert_eq!(empty.help(), "use `points: 1` or greater");

        let too_large = linspace(0.0, 1.0, 1_000_001).unwrap_err();
        assert_eq!(
            too_large,
            CoordinateIndexError::LinspacePointCount(IndexCardinalityError::TooLarge {
                value: 1_000_001,
                maximum: MAX_INDEX_CARDINALITY,
            })
        );
        assert!(too_large.concerns_point_count());
        assert_eq!(
            too_large.to_string(),
            "index cardinality 1000001 exceeds the practical limit of 1000000 elements"
        );
        assert_eq!(too_large.help(), "use a point count from 1 through 1000000");
    }

    #[test]
    fn linspace_endpoints_must_agree_with_the_point_count() {
        let singleton = linspace(0.0, 1.0, 1).unwrap_err();
        assert_eq!(
            singleton,
            CoordinateIndexError::SingletonLinspaceWithDistinctEndpoints
        );
        assert!(!singleton.concerns_point_count());
        assert_eq!(
            linspace(1.0, 1.0, 2),
            Err(CoordinateIndexError::LinspaceWithIdenticalEndpoints)
        );
    }

    #[test]
    fn linspace_rejects_coordinates_that_collapse_in_binary64() {
        let error = linspace(0.0, f64::from_bits(1), 3).unwrap_err();
        assert!(matches!(
            error,
            CoordinateIndexError::NonMonotonicCoordinates { .. }
        ));
        assert!(!error.concerns_point_count());
        assert_eq!(
            error.help(),
            "reduce the cardinality or increase the spacing between adjacent coordinates"
        );
    }

    #[test]
    fn finite_index_renders_its_source_spelling() {
        let index = FiniteIndex::try_from_u64(3).unwrap();
        assert_eq!(index.to_string(), "Fin(3)");
        assert_eq!(IndexBindingTarget::Finite(index).to_string(), "Fin(3)");
        assert_eq!(
            FiniteIndex::try_from_u64(0),
            Err(IndexCardinalityError::Empty)
        );
    }

    #[test]
    fn cardinality_errors_render_for_finite_indexes() {
        assert_eq!(
            IndexCardinalityError::Empty.describe_finite_index(),
            "Fin(0) is not allowed; indexes must contain at least one element"
        );
        assert_eq!(
            IndexCardinalityError::DoesNotFitUsize { value: 7 }.describe_finite_index(),
            "Fin cardinality 7 does not fit in usize on this target"
        );
        assert_eq!(
            IndexCardinalityError::TooLarge {
                value: 1_000_001,
                maximum: MAX_INDEX_CARDINALITY,
            }
            .describe_finite_index(),
            "Fin cardinality 1000001 exceeds the practical limit of 1000000 elements"
        );
    }

    #[test]
    fn required_indexes_have_no_concrete_elements() {
        for kind in [
            IndexKind::Required(RequiredIndexKind::Named),
            IndexKind::Required(RequiredIndexKind::Coordinate { dimension: time() }),
        ] {
            let definition = IndexDef {
                name: IndexBindingTarget::Declared(IndexName::expect_valid("Axis")),
                kind,
            };
            assert!(definition.is_required());
            assert_eq!(definition.concrete(), None);
            assert_eq!(definition.concrete_cardinality(), None);
            assert_eq!(definition.entry_keys(), vec![]);
            assert_eq!(definition.finite_index_size(), None);
        }
    }

    #[test]
    fn index_def_queries_follow_the_kind() {
        let named = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("Phase")),
            kind: named_kind(&["Burn", "Coast"]),
        };
        assert!(!named.is_required());
        assert!(!named.is_coordinate());
        assert_eq!(named.category(), IndexCategory::Named);
        assert_eq!(
            named.concrete_cardinality(),
            Some(IndexCardinality::try_from_u64(2).unwrap())
        );
        assert_eq!(
            named.entry_keys(),
            vec![
                IndexEntryKey::named(IndexVariantName::expect_valid("Burn")),
                IndexEntryKey::named(IndexVariantName::expect_valid("Coast")),
            ]
        );
        assert_eq!(named.coordinate_dimension(), None);

        let required_coordinate = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("Step")),
            kind: IndexKind::Required(RequiredIndexKind::Coordinate { dimension: time() }),
        };
        assert!(required_coordinate.is_coordinate());
        assert_eq!(required_coordinate.category(), IndexCategory::Coordinate);
        assert_eq!(required_coordinate.coordinate_data(), None);
        assert_eq!(required_coordinate.coordinate_dimension(), Some(&time()));

        let data = CoordinateIndexData::try_range(0.0, 2.0, 1.0, time(), CoordinateDisplayUnit::SI)
            .unwrap();
        let coordinate = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("Clock")),
            kind: IndexKind::Concrete(ConcreteIndexKind::Coordinate(data.clone())),
        };
        assert!(coordinate.is_coordinate());
        assert_eq!(coordinate.category(), IndexCategory::Coordinate);
        assert_eq!(coordinate.coordinate_data(), Some(&data));
        assert_eq!(coordinate.coordinate_dimension(), Some(&time()));
        assert_eq!(coordinate.finite_index_size(), None);

        let finite = IndexDef::finite(FiniteIndex::try_from_u64(4).unwrap());
        assert_eq!(finite.category(), IndexCategory::Finite);
        assert_eq!(finite.finite_index_size(), Some(4));
        assert!(!finite.is_coordinate());
        assert_eq!(IndexCardinality::ONE.get(), 1);
    }

    #[test]
    fn binding_contract_accepts_compatible_required_forwarding() {
        let contract = IndexBindingContract::Coordinate { dimension: time() };
        let candidate = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("OuterStep")),
            kind: IndexKind::Required(RequiredIndexKind::Coordinate { dimension: time() }),
        };

        assert_eq!(contract.validate(&candidate), Ok(()));
    }

    #[test]
    fn discrete_binding_contract_accepts_named_and_finite_axes() {
        let contract = IndexBindingContract::Discrete;
        let named = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("Phase")),
            kind: named_kind(&["Only"]),
        };
        let finite = IndexDef::finite(FiniteIndex::try_from_u64(3).unwrap());

        assert_eq!(contract.validate(&named), Ok(()));
        assert_eq!(contract.validate(&finite), Ok(()));
        assert_eq!(
            IndexBindingContract::Named.validate(&finite),
            Err(IndexBindingContractError::KindMismatch {
                expected: IndexBindingCategory::Named,
                found: IndexCategory::Finite,
            })
        );
    }

    #[test]
    fn binding_contract_preserves_kind_and_dimension_errors() {
        let length = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ));
        let contract = IndexBindingContract::Coordinate { dimension: time() };
        let named = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("Phase")),
            kind: named_kind(&["Only"]),
        };
        assert_eq!(
            contract.validate(&named),
            Err(IndexBindingContractError::KindMismatch {
                expected: IndexBindingCategory::Coordinate,
                found: IndexCategory::Named,
            })
        );

        let coordinate = IndexDef {
            name: IndexBindingTarget::Declared(IndexName::expect_valid("DistanceStep")),
            kind: IndexKind::Required(RequiredIndexKind::Coordinate {
                dimension: length.clone(),
            }),
        };
        assert_eq!(
            contract.validate(&coordinate),
            Err(IndexBindingContractError::DimensionMismatch {
                expected: time(),
                found: length,
            })
        );
    }

    #[test]
    fn finite_and_coordinate_entry_keys_are_typed_positions() {
        for kind in [
            ConcreteIndexKind::Finite {
                index: FiniteIndex::try_from_u64(3).unwrap(),
            },
            ConcreteIndexKind::Coordinate(range(0.0, 2.0, 1.0).unwrap()),
        ] {
            let definition = IndexDef {
                name: IndexBindingTarget::Declared(IndexName::expect_valid("Axis")),
                kind: IndexKind::Concrete(kind),
            };
            assert_eq!(
                definition.entry_keys(),
                vec![
                    IndexEntryKey::Position(0),
                    IndexEntryKey::Position(1),
                    IndexEntryKey::Position(2),
                ]
            );
        }
    }
}
