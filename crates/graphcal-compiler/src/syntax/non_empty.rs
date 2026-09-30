//! Small semantic helper for homogeneous non-empty sequences.
//!
//! Use these only when the domain means “one or more homogeneous items” and
//! neither end of the sequence has a distinct semantic role. If the head or
//! tail is special (for example `qualifier + member`), prefer a domain-specific
//! type instead.

use std::num::NonZeroUsize;
use std::ops::{Index, IndexMut};

use thiserror::Error;

/// A homogeneous sequence with at least one element.
///
/// The invariant is enforced at construction boundaries while the storage stays
/// vector-backed. That keeps recursive AST shapes such as `NonEmpty<Expr<P>>`
/// safely indirect, just like `Vec<Expr<P>>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NonEmpty<T> {
    items: Vec<T>,
}

/// Error returned when converting an empty `Vec` into [`NonEmpty`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("expected at least one item, got an empty vector")]
pub struct EmptyVecError;

/// A homogeneous sequence with at least two elements.
///
/// This represents n-ary operations whose syntax and semantics require two or
/// more operands while keeping long chains in contiguous, stack-safe storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AtLeastTwo<T> {
    items: Vec<T>,
}

impl<T> AtLeastTwo<T> {
    /// Construct a sequence from its first two items.
    #[must_use]
    pub fn new(first: T, second: T) -> Self {
        Self {
            items: vec![first, second],
        }
    }

    /// Iterate over all elements in source order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Mutably iterate over all elements in source order.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.items.iter_mut()
    }

    /// Number of elements. Always at least 2.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `false`; provided for API compatibility with sequence-like code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Append an item to the end of the sequence.
    pub(crate) fn push(&mut self, item: T) {
        self.items.push(item);
    }

    /// Map borrowed items while preserving the two-or-more invariant.
    #[must_use]
    pub fn map_ref<U>(&self, f: impl FnMut(&T) -> U) -> AtLeastTwo<U> {
        AtLeastTwo {
            items: self.items.iter().map(f).collect(),
        }
    }

    /// Fallibly map borrowed items while preserving the two-or-more invariant.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `f`.
    pub fn try_map_ref<U, E>(&self, f: impl FnMut(&T) -> Result<U, E>) -> Result<AtLeastTwo<U>, E> {
        Ok(AtLeastTwo {
            items: self.items.iter().map(f).collect::<Result<_, _>>()?,
        })
    }
}

impl<T> AtLeastTwo<T> {
    /// Borrow the elements as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// The first element.
    #[must_use]
    pub fn first(&self) -> &T {
        &self.items[0]
    }

    /// Map owned items while preserving the two-or-more invariant.
    #[must_use]
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> AtLeastTwo<U> {
        AtLeastTwo {
            items: self.items.into_iter().map(f).collect(),
        }
    }

    /// Pair every item with the element at the same position of `other`.
    ///
    /// # Errors
    ///
    /// Returns `other` unchanged when its length differs from `self`'s.
    pub fn zip_exact<U>(self, other: Vec<U>) -> Result<AtLeastTwo<(T, U)>, Vec<U>> {
        if other.len() != self.items.len() {
            return Err(other);
        }
        Ok(AtLeastTwo {
            items: self.items.into_iter().zip(other).collect(),
        })
    }
}

impl<T> Index<usize> for AtLeastTwo<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index]
    }
}

impl<T> IntoIterator for AtLeastTwo<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a AtLeastTwo<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut AtLeastTwo<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

impl<T> NonEmpty<T> {
    /// Construct a non-empty sequence from its first item and the remaining items.
    #[must_use]
    pub fn new(first: T, rest: Vec<T>) -> Self {
        let mut items = Vec::with_capacity(1 + rest.len());
        items.push(first);
        items.extend(rest);
        Self { items }
    }

    /// Construct a singleton non-empty sequence.
    #[must_use]
    pub fn singleton(first: T) -> Self {
        Self { items: vec![first] }
    }

    /// Convert a vector into a non-empty sequence.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyVecError`] if `items` is empty.
    pub fn try_from_vec(items: Vec<T>) -> Result<Self, EmptyVecError> {
        if items.is_empty() {
            Err(EmptyVecError)
        } else {
            Ok(Self { items })
        }
    }

    /// Convert into a vector, preserving order.
    #[must_use]
    pub(crate) fn into_vec(self) -> Vec<T> {
        self.items
    }

    /// Borrow as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Mutably borrow as a slice.
    #[must_use]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items
    }

    /// Number of elements. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `false`; provided for API compatibility with sequence-like code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// First element in source order.
    #[must_use]
    pub fn first(&self) -> &T {
        &self.items[0]
    }

    /// The first element and the rest, in source order.
    #[must_use]
    pub fn split_first(&self) -> (&T, &[T]) {
        (&self.items[0], &self.items[1..])
    }

    /// Last element in source order.
    #[must_use]
    pub fn last(&self) -> &T {
        &self.items[self.items.len() - 1]
    }

    /// Iterate over all elements in source order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Mutably iterate over all elements in source order.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.items.iter_mut()
    }

    /// Append an item to the end of the sequence.
    pub(crate) fn push(&mut self, item: T) {
        self.items.push(item);
    }

    /// Map borrowed items while preserving non-emptiness.
    #[must_use]
    pub fn map_ref<U>(&self, f: impl FnMut(&T) -> U) -> NonEmpty<U> {
        NonEmpty {
            items: self.items.iter().map(f).collect(),
        }
    }

    /// Map each item while preserving non-emptiness.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> NonEmpty<U> {
        NonEmpty {
            items: self.items.into_iter().map(f).collect(),
        }
    }

    /// Fallibly map owned items while preserving non-emptiness.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `f`.
    pub fn try_map<U, E>(self, f: impl FnMut(T) -> Result<U, E>) -> Result<NonEmpty<U>, E> {
        Ok(NonEmpty {
            items: self.items.into_iter().map(f).collect::<Result<_, _>>()?,
        })
    }

    /// Fallibly map borrowed items while preserving non-emptiness.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `f`.
    pub fn try_map_ref<U, E>(&self, f: impl FnMut(&T) -> Result<U, E>) -> Result<NonEmpty<U>, E> {
        Ok(NonEmpty {
            items: self.items.iter().map(f).collect::<Result<_, _>>()?,
        })
    }
}

impl<A, B> NonEmpty<(A, B)> {
    /// Split a sequence of pairs into two sequences of the same length.
    #[must_use]
    pub fn unzip(self) -> (NonEmpty<A>, NonEmpty<B>) {
        let (left, right) = self.items.into_iter().unzip();
        (NonEmpty { items: left }, NonEmpty { items: right })
    }
}

impl<T> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = EmptyVecError;

    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        Self::try_from_vec(value)
    }
}

impl<T> From<(T, Vec<T>)> for NonEmpty<T> {
    fn from((first, rest): (T, Vec<T>)) -> Self {
        Self::new(first, rest)
    }
}

impl<T> From<T> for NonEmpty<T> {
    fn from(first: T) -> Self {
        Self::singleton(first)
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a NonEmpty<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut NonEmpty<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

impl<T> Index<usize> for NonEmpty<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index]
    }
}

impl<T> IndexMut<usize> for NonEmpty<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.items[index]
    }
}

impl<T, const N: usize> TryFrom<[T; N]> for NonEmpty<T> {
    type Error = EmptyVecError;

    fn try_from(value: [T; N]) -> Result<Self, Self::Error> {
        Self::try_from_vec(value.into_iter().collect())
    }
}

/// A non-empty sequence whose elements are pairwise distinct.
///
/// Order is preserved: consumers such as named-index variants treat the
/// declaration order as the axis order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmptyUnique<T> {
    items: NonEmpty<T>,
}

/// Two equal items found while building a [`NonEmptyUnique`] sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("item {duplicate} duplicates item {first}")]
pub struct DuplicateItemError {
    /// Position of the earlier item.
    pub first: usize,
    /// Position of the later, duplicated item.
    pub duplicate: usize,
}

impl<T: Eq + std::hash::Hash> NonEmptyUnique<T> {
    /// Validate that a non-empty sequence is free of duplicates.
    ///
    /// # Errors
    ///
    /// Returns the positions of the first duplicated pair in source order.
    pub fn try_from_non_empty(items: NonEmpty<T>) -> Result<Self, DuplicateItemError> {
        let mut seen = std::collections::HashMap::with_capacity(items.len());
        for (position, item) in items.iter().enumerate() {
            if let Some(first) = seen.insert(item, position) {
                return Err(DuplicateItemError {
                    first,
                    duplicate: position,
                });
            }
        }
        Ok(Self { items })
    }
}

impl<T> NonEmptyUnique<T> {
    /// Borrow the distinct items in order.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        self.items.as_slice()
    }

    /// Iterate over the distinct items in order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Number of items, which is never zero.
    #[must_use]
    pub const fn len(&self) -> NonZeroUsize {
        NonZeroUsize::MIN.saturating_add(self.items.len() - 1)
    }
}

impl<'a, T> IntoIterator for &'a NonEmptyUnique<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unzip_keeps_both_halves_in_order() {
        let (left, right) = NonEmpty::new((1, 'a'), vec![(2, 'b'), (3, 'c')]).unzip();
        assert_eq!(left.as_slice(), &[1, 2, 3]);
        assert_eq!(right.as_slice(), &['a', 'b', 'c']);
    }

    #[test]
    fn at_least_two_zip_exact_pairs_equal_lengths_and_returns_mismatches() {
        let pair = AtLeastTwo::new(1, 2);
        assert_eq!(pair.clone().zip_exact(vec![3]), Err(vec![3]));
        assert_eq!(pair.clone().zip_exact(vec![3, 4, 5]), Err(vec![3, 4, 5]));
        let zipped = pair.zip_exact(vec!['a', 'b']).unwrap();
        assert_eq!(zipped.as_slice(), &[(1, 'a'), (2, 'b')]);
        let mapped = zipped.map(|(n, _)| n * 10);
        assert_eq!(*mapped.first(), 10);
        assert_eq!(mapped[1], 20);
        assert_eq!(mapped.into_iter().collect::<Vec<_>>(), vec![10, 20]);
    }

    #[test]
    fn non_empty_unique_preserves_order_and_length() {
        let items = NonEmptyUnique::try_from_non_empty(NonEmpty::new(3, vec![1, 2])).unwrap();
        assert_eq!(items.as_slice(), &[3, 1, 2]);
        assert_eq!(items.iter().copied().collect::<Vec<_>>(), vec![3, 1, 2]);
        assert_eq!((&items).into_iter().count(), 3);
        assert_eq!(items.len().get(), 3);
    }

    #[test]
    fn non_empty_split_first_separates_the_head_from_the_rest() {
        let items = NonEmpty::new(3, vec![1, 2]);
        assert_eq!(items.split_first(), (&3, &[1, 2][..]));
        let single = NonEmpty::singleton('a');
        assert_eq!(single.split_first(), (&'a', &[][..]));
    }

    #[test]
    fn non_empty_unique_accepts_singleton() {
        let items = NonEmptyUnique::try_from_non_empty(NonEmpty::singleton("only")).unwrap();
        assert_eq!(items.len().get(), 1);
        assert_eq!(items.as_slice(), &["only"]);
    }

    #[test]
    fn non_empty_unique_reports_first_duplicate_pair() {
        assert_eq!(
            NonEmptyUnique::try_from_non_empty(NonEmpty::new(1, vec![2, 3, 2, 1])),
            Err(DuplicateItemError {
                first: 1,
                duplicate: 3,
            })
        );
        assert_eq!(
            NonEmptyUnique::try_from_non_empty(NonEmpty::new(7, vec![7])),
            Err(DuplicateItemError {
                first: 0,
                duplicate: 1,
            })
        );
    }

    #[test]
    fn map_ref_preserves_order() {
        let items = NonEmpty::new(1, vec![2, 3]);
        assert_eq!(items.map_ref(|item| item * 10).as_slice(), &[10, 20, 30]);
    }
}
