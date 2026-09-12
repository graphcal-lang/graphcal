//! A node has either a formula or an explicit, closed dependency interface.
//!
//! The expression and reference parameters keep syntax and resolved bodies in
//! their own phases. An unfinished definition is deliberately not an expression.

use crate::syntax::span::Spanned;

#[derive(Debug, Clone)]
pub enum NodeDefinition<E, R> {
    Formula(E),
    /// The marker span includes the braces, including an explicitly empty list.
    Todo(Spanned<Vec<Spanned<R>>>),
}

impl<E, R> NodeDefinition<E, R> {
    #[must_use]
    pub const fn formula(&self) -> Option<&E> {
        match self {
            Self::Formula(expression) => Some(expression),
            Self::Todo(_) => None,
        }
    }

    #[must_use]
    pub const fn formula_mut(&mut self) -> Option<&mut E> {
        match self {
            Self::Formula(expression) => Some(expression),
            Self::Todo(_) => None,
        }
    }

    #[must_use]
    pub const fn todo(&self) -> Option<&Spanned<Vec<Spanned<R>>>> {
        match self {
            Self::Formula(_) => None,
            Self::Todo(dependencies) => Some(dependencies),
        }
    }

    #[must_use]
    pub fn map_formula<F>(self, transform: impl FnOnce(E) -> F) -> NodeDefinition<F, R> {
        match self {
            Self::Formula(expression) => NodeDefinition::Formula(transform(expression)),
            Self::Todo(dependencies) => NodeDefinition::Todo(dependencies),
        }
    }
}
