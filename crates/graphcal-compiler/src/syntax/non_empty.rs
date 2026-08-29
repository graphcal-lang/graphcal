//! Small semantic helper for homogeneous non-empty sequences.
//!
//! Use these only when the domain means “one or more homogeneous items” and
//! neither end of the sequence has a distinct semantic role. If the head or
//! tail is special (for example `qualifier + member`), prefer a domain-specific
//! type instead.

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

    /// Consume the sequence into its last item and the preceding items.
    #[must_use]
    pub(crate) fn into_last_and_init(mut self) -> (T, Vec<T>) {
        let last_index = self.items.len() - 1;
        let last = self.items.remove(last_index);
        (last, self.items)
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

    /// Last element in source order.
    #[must_use]
    pub fn last(&self) -> &T {
        &self.items[self.items.len() - 1]
    }

    /// Split into the last element and the elements before it.
    #[must_use]
    pub(crate) fn split_last(&self) -> (&T, &[T]) {
        let last_index = self.items.len() - 1;
        (&self.items[last_index], &self.items[..last_index])
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

    /// Map each item while preserving non-emptiness.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> NonEmpty<U> {
        NonEmpty {
            items: self.items.into_iter().map(f).collect(),
        }
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
