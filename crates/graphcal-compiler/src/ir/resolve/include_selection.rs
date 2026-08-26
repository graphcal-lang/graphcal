//! Pure validation for selective include producer identities.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::desugar::desugared_ast::ImportItem;
use crate::registry::error::GraphcalError;
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;

/// Typed producer identity selected by one include item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IncludeProducer {
    namespace: ImportItemNamespace,
    name: NameAtom,
}

/// A selective include chose one producer more than once.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("selective include chooses {namespace} producer `{name}` more than once")]
pub struct DuplicateIncludeProducer {
    pub namespace: ImportItemNamespace,
    pub name: NameAtom,
    pub first: Span,
    pub duplicate: Span,
}

/// Require each `(namespace, producer name)` at most once in one include list.
///
/// Local aliases are deliberately absent from the key. Downstream assertion,
/// plot, and attribute metadata is producer-owned, so accepting alias fan-out
/// would require a multi-entry representation rather than producer-keyed maps.
pub fn validate_unique_include_producers(
    items: &[ImportItem],
) -> Result<(), DuplicateIncludeProducer> {
    items
        .iter()
        .try_fold(HashMap::<IncludeProducer, Span>::new(), |mut seen, item| {
            let producer = IncludeProducer {
                namespace: item.namespace,
                name: item.name.name.clone(),
            };
            seen.insert(producer.clone(), item.name.span)
                .map_or(Ok(seen), |first| {
                    Err(DuplicateIncludeProducer {
                        namespace: producer.namespace,
                        name: producer.name,
                        first,
                        duplicate: item.name.span,
                    })
                })
        })
        .map(drop)
}

/// Attach importer source text to a pure producer-uniqueness error.
#[must_use]
pub fn duplicate_include_producer_to_graphcal(
    error: DuplicateIncludeProducer,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::DuplicateIncludeSelection {
        namespace: error.namespace,
        name: error.name,
        src: src.clone(),
        first: error.first.into(),
        duplicate: error.duplicate.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::desugar::desugared_ast::{DeclKind, ImportKind};
    use crate::syntax::desugar::desugar_multi_decls_in_file;
    use crate::syntax::parser::Parser;

    use super::*;

    fn include_items(source: &str) -> Vec<ImportItem> {
        let file = desugar_multi_decls_in_file(Parser::new(source).parse_file().unwrap());
        let DeclKind::Include(include) = &file.declarations[0].kind else {
            panic!("expected include declaration")
        };
        let ImportKind::Selective(items) = &include.kind else {
            panic!("expected selective include")
        };
        items.clone()
    }

    #[test]
    fn aliases_do_not_make_repeated_producers_unique() {
        let items = include_items("include source().{ value as first, value as second };");
        let error = validate_unique_include_producers(&items).unwrap_err();
        assert_eq!(error.name.as_str(), "value");
        assert_eq!(error.first, items[0].name.span);
        assert_eq!(error.duplicate, items[1].name.span);
    }

    #[test]
    fn unique_producers_are_order_independent() {
        for source in [
            "include source().{ first as x, second as y };",
            "include source().{ second as y, first as x };",
        ] {
            assert!(validate_unique_include_producers(&include_items(source)).is_ok());
        }
    }
}
