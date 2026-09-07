//! Assertion expectation records and semantic index-key selection.
//!
//! The index parameter preserves the distinction between source paths and
//! resolved identities. Collection and attribute parsing are separate clients.

use crate::dag_id::DagId;
use crate::registry::declared_type::IndexTypeRef;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::span::Span;

/// One axis segment in a per-variant assertion expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedFailKeyPart<I = IndexTypeRef> {
    /// The index is a source path before resolution and a semantic reference after it.
    Named {
        index: I,
        variant: IndexVariantName,
        span: Span,
    },
    /// A finite axis position, validated against its assertion axis during checking.
    FinitePosition { position: u64, span: Span },
}

impl<I> ExpectedFailKeyPart<I> {
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } | Self::FinitePosition { span, .. } => *span,
        }
    }

    #[must_use]
    pub(crate) fn entry_key(&self) -> IndexEntryKey {
        match self {
            Self::Named { variant, .. } => IndexEntryKey::named(variant.clone()),
            Self::FinitePosition { position, .. } => IndexEntryKey::position(*position),
        }
    }
}

impl ExpectedFailKeyPart<IndexTypeRef> {
    #[must_use]
    pub fn with_owner(
        owner: DagId,
        index: IndexName,
        variant: IndexVariantName,
        span: Span,
    ) -> Self {
        Self::Named {
            index: IndexTypeRef::with_owner(owner, index),
            variant,
            span,
        }
    }

    #[must_use]
    pub(crate) fn resolved(resolved: ResolvedIndexVariant, span: Span) -> Self {
        let (index, variant) = resolved.into_parts();
        Self::Named {
            index: IndexTypeRef::from_resolved(index),
            variant,
            span,
        }
    }

    #[must_use]
    pub(crate) const fn named_index(&self) -> Option<&IndexTypeRef> {
        match self {
            Self::Named { index, .. } => Some(index),
            Self::FinitePosition { .. } => None,
        }
    }

    /// Match a key within the already checked assertion axis.
    ///
    /// Named selectors require canonical index identity. A finite selector
    /// matches its position; its tuple's axis was validated during checking.
    #[must_use]
    pub fn matches_entry(&self, index: &IndexTypeRef, key: &IndexEntryKey) -> bool {
        match (self, key) {
            (
                Self::Named {
                    index: expected,
                    variant: expected_variant,
                    ..
                },
                IndexEntryKey::Named(actual),
            ) => expected.matches_ref(index) && actual == expected_variant,
            (Self::FinitePosition { position, .. }, IndexEntryKey::Position(actual)) => {
                matches!(index, IndexTypeRef::Finite(_)) && actual == position
            }
            (Self::Named { .. }, IndexEntryKey::Position(_))
            | (Self::FinitePosition { .. }, IndexEntryKey::Named(_)) => false,
        }
    }

    /// Render an expectation segment at the diagnostic boundary.
    #[must_use]
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Named { index, variant, .. } => format!("{}#{variant}", index.display_name()),
            Self::FinitePosition { position, .. } => format!("#{position}"),
        }
    }
}

/// One expected-fail key, in the assertion's axis order.
pub type ExpectedFailKey<I = IndexTypeRef> = Vec<ExpectedFailKeyPart<I>>;

/// Whether the whole scalar assertion or selected indexed entries should fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedFail<I = IndexTypeRef> {
    All,
    Variants(Vec<ExpectedFailKey<I>>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::index::FiniteIndex;
    use std::path::Path;

    fn axis(file: &str) -> IndexTypeRef {
        IndexTypeRef::with_owner(
            DagId::from_virtual_relative_path(Path::new(file)).unwrap(),
            IndexName::expect_valid("Mode"),
        )
    }

    #[test]
    fn named_selection_requires_identity_not_equal_spelling() {
        let index = axis("first.gcl");
        let variant = IndexVariantName::expect_valid("Boost");
        let selector = ExpectedFailKeyPart::Named {
            index: index.clone(),
            variant: variant.clone(),
            span: Span::new(0, 0),
        };
        let key = IndexEntryKey::named(variant);
        assert!(selector.matches_entry(&index, &key));
        let alias = IndexTypeRef::with_display_leaf(
            IndexName::expect_valid("Alias"),
            index.declared_resolved().unwrap().clone(),
        );
        assert!(selector.matches_entry(&alias, &key));
        assert!(!selector.matches_entry(&axis("second.gcl"), &key));
        assert!(!selector.matches_entry(&index, &IndexEntryKey::position(1)));
        assert!(!selector.matches_entry(
            &index,
            &IndexEntryKey::named(IndexVariantName::expect_valid("Coast"))
        ));
    }

    #[test]
    fn finite_selection_requires_a_finite_axis_and_matching_position() {
        let index = IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(3).unwrap());
        let selector = ExpectedFailKeyPart::FinitePosition {
            position: 1,
            span: Span::new(0, 0),
        };
        assert!(selector.matches_entry(&index, &IndexEntryKey::position(1)));
        assert!(!selector.matches_entry(&index, &IndexEntryKey::position(2)));
        assert!(!selector.matches_entry(&axis("named.gcl"), &IndexEntryKey::position(1)));
        assert!(!selector.matches_entry(
            &index,
            &IndexEntryKey::named(IndexVariantName::expect_valid("Boost"))
        ));
    }
}
