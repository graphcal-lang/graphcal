use std::collections::HashMap;
use std::num::NonZeroUsize;

use thiserror::Error;

use crate::dimension::Dimension;
use crate::syntax::index_name::{IndexName, IndexVariantName};

#[derive(Debug, Clone)]
pub struct RangeIndexData {
    pub start: f64,
    pub end: f64,
    pub step: f64,
    /// Validated number of inclusive range steps.
    pub(crate) step_count: NonZeroUsize,
    pub dimension: Dimension,
    /// Display unit label (e.g., `"s"`) for formatting step values.
    pub display_label: Option<String>,
    /// Scale factor from SI to display unit: `display_value = si_value / scale`.
    pub display_scale: f64,
}

impl RangeIndexData {
    /// Returns the SI value at step `i`.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "range step indices are small enough for exact f64 representation"
    )]
    pub fn step_value(&self, i: usize) -> f64 {
        (i as f64).mul_add(self.step, self.start)
    }

    /// Returns the number of steps in this range.
    #[must_use]
    const fn step_count(&self) -> usize {
        self.step_count.get()
    }
}

/// The kind of an index: either named variants or a numeric range.
#[derive(Debug, Clone)]
pub enum IndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named { variants: Vec<IndexVariantName> },
    /// A numeric range, e.g. `index T = linspace(0.0 s, 100.0 s, step: 0.1 s);`
    Range(RangeIndexData),
    /// Required named index (no variants): must be bound via parameterized import.
    RequiredNamed,
    /// Required range index with dimension constraint: must be bound via parameterized import.
    RequiredRange { dimension: Dimension },
    /// A Nat-parameterized range: `range(N)` with elements `{0, 1, ..., N-1}`.
    ///
    /// Created synthetically for integer literals in index position (e.g., `D[3]`).
    NatRange {
        /// The non-zero size of the range (number of elements). Stored as
        /// `usize` because it bounds in-memory variant tables; AST-level Nat
        /// literals are converted at the registry boundary.
        size: NonZeroUsize,
    },
}

/// A declared index with its ordered variants.
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: IndexName,
    pub kind: IndexKind,
}

impl IndexDef {
    /// Returns the ordered variant names for this index.
    ///
    /// For named indexes, returns the declared variants.
    /// For range indexes, generates synthetic names like `"#0"`, `"#1"`, etc.
    /// For nat range indexes, generates synthetic names like `"#0"`, `"#1"`, etc.
    /// For required indexes, returns an empty vec (no variants until bound).
    #[must_use]
    pub fn variants(&self) -> Vec<IndexVariantName> {
        match &self.kind {
            IndexKind::Named { variants } => variants.clone(),
            IndexKind::Range(data) => {
                let count = data.step_count();
                (0..count).map(IndexVariantName::range_step).collect()
            }
            IndexKind::NatRange { size } => {
                (0..size.get()).map(IndexVariantName::range_step).collect()
            }
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. } => vec![],
        }
    }

    /// Returns the number of steps/variants in this index.
    ///
    /// Returns 0 for required indexes (no variants until bound).
    #[must_use]
    pub const fn step_count(&self) -> usize {
        match &self.kind {
            IndexKind::Named { variants } => variants.len(),
            IndexKind::Range(data) => data.step_count(),
            IndexKind::NatRange { size } => size.get(),
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. } => 0,
        }
    }

    /// Returns the range data if this is a concrete range index.
    #[must_use]
    pub const fn range_data(&self) -> Option<&RangeIndexData> {
        match &self.kind {
            IndexKind::Range(data) => Some(data),
            _ => None,
        }
    }

    /// Returns true if this is a range index (concrete or required, not nat range).
    #[must_use]
    pub const fn is_range(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Range(_) | IndexKind::RequiredRange { .. }
        )
    }

    /// Returns true if this is a named index (concrete or required).
    #[must_use]
    pub const fn is_named(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Named { .. } | IndexKind::RequiredNamed
        )
    }

    /// Returns true if this is a nat range index.
    #[must_use]
    pub(crate) const fn is_nat_range(&self) -> bool {
        matches!(self.kind, IndexKind::NatRange { .. })
    }

    /// Returns the nat range size, if this is a nat range index.
    #[must_use]
    pub(crate) const fn nat_range_size(&self) -> Option<u64> {
        match &self.kind {
            IndexKind::NatRange { size } => Some(size.get() as u64),
            _ => None,
        }
    }

    /// Returns true if this is a required index (must be bound via parameterized import).
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Nat range helpers
// ---------------------------------------------------------------------------

/// Error returned when an AST/runtime Nat range size cannot become a concrete index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NatRangeIndexError {
    /// Empty Nat ranges are deliberately not representable.
    #[error("range(0) is not allowed; indexes must contain at least one element")]
    Empty,
    /// The source-level `u64` size does not fit in this target's in-memory index size.
    #[error("nat range size {size} does not fit in usize on this target")]
    DoesNotFitUsize { size: u64 },
}

/// Typed identity for a concrete compiler-generated Nat range index.
///
/// The core carries this non-zero size directly; display names are derived only
/// for diagnostics and compatibility with APIs that still need an [`IndexName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NatRangeIndex {
    size: NonZeroUsize,
}

impl NatRangeIndex {
    /// Create an identity for a non-empty Nat range index.
    #[must_use]
    pub(crate) const fn new(size: NonZeroUsize) -> Self {
        Self { size }
    }

    /// Try to create an identity from an AST/runtime `u64` size.
    ///
    /// # Errors
    ///
    /// Returns an error when `size` is zero or cannot fit in `usize` on this target.
    pub fn try_from_u64(size: u64) -> Result<Self, NatRangeIndexError> {
        if size == 0 {
            return Err(NatRangeIndexError::Empty);
        }
        let size =
            usize::try_from(size).map_err(|_| NatRangeIndexError::DoesNotFitUsize { size })?;
        let size = NonZeroUsize::new(size).ok_or(NatRangeIndexError::Empty)?;
        Ok(Self::new(size))
    }

    /// Return the non-zero in-memory size.
    #[must_use]
    pub const fn size(self) -> NonZeroUsize {
        self.size
    }

    /// Return the size as a `u64` for Nat-expression comparisons and display.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Graphcal currently supports targets where usize fits in u64"
    )]
    pub(crate) fn size_u64(self) -> u64 {
        u64::try_from(self.size.get()).expect("usize fits in u64 on supported targets")
    }

    /// Render this identity for diagnostics as source-level `range(N)` syntax.
    #[must_use]
    pub fn display_name(self) -> IndexName {
        IndexName::expect_valid(format!("range({})", self.size_u64()))
    }
}

/// Index registry: maps declared index names and typed Nat-range identities to `IndexDef`.
#[derive(Debug, Clone)]
pub struct IndexRegistry {
    pub(crate) indexes: HashMap<IndexName, IndexDef>,
    pub(crate) nat_ranges: HashMap<NatRangeIndex, IndexDef>,
}

impl IndexRegistry {
    /// Look up a declared index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Look up a compiler-generated Nat range index by typed identity.
    #[must_use]
    pub fn get_nat_range(&self, index: NatRangeIndex) -> Option<&IndexDef> {
        self.nat_ranges.get(&index)
    }

    /// Iterate over all index definitions.
    pub fn all_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        self.indexes.values().chain(self.nat_ranges.values())
    }
}
