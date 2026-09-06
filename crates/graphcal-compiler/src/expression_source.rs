//! Diagnostic coordinates are a projection of expression identity, never its key.
//!
//! The map is sealed alongside strict HIR lowering. Equal spans are valid;
//! repeated identities and lookups from another body revision are errors.

use crate::expression_id::{ExprId, ExprIdExhausted, UnassignedExprId};
use crate::syntax::span::Span;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ExpressionSourceMap {
    spans: HashMap<ExprId, Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExpressionSourceError {
    #[error(transparent)]
    Exhausted(#[from] ExprIdExhausted),
    #[error(transparent)]
    Unassigned(#[from] UnassignedExprId),
    #[error("duplicate expression identity in one source map: {0:?}")]
    Duplicate(ExprId),
    #[error("expression identity is absent from this source revision: {0:?}")]
    Missing(ExprId),
}

impl ExpressionSourceMap {
    pub(crate) fn try_new(
        entries: impl IntoIterator<Item = (ExprId, Span)>,
    ) -> Result<Self, ExpressionSourceError> {
        entries.into_iter().try_fold(
            Self {
                spans: HashMap::new(),
            },
            |mut result, (id, span)| {
                if result.spans.insert(id.clone(), span).is_some() {
                    return Err(ExpressionSourceError::Duplicate(id));
                }
                Ok(result)
            },
        )
    }

    pub fn span(&self, id: &ExprId) -> Result<Span, ExpressionSourceError> {
        self.spans
            .get(id)
            .copied()
            .ok_or_else(|| ExpressionSourceError::Missing(id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression_id::ExprIds;

    #[test]
    fn duplicate_identity_is_rejected_even_when_the_spans_agree() {
        let id = ExprIds::default().allocate().unwrap();
        let span = Span::new(0, 1);
        assert!(matches!(
            ExpressionSourceMap::try_new([(id.clone(), span), (id, span)]),
            Err(ExpressionSourceError::Duplicate(_))
        ));
    }
}
