//! Selective-import categories and category-mismatch diagnostics.

use crate::syntax::names::NameAtom;
use crate::syntax::non_empty::NonEmpty;

/// Category targeted by a single selective import item.
///
/// Every non-term category has an explicit source marker. Bare items target
/// the term sort, which contains value declarations and constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportItemNamespace {
    /// Term namespace (values and constructors), written without a marker.
    Term,
    /// Type namespace, written with the `type` marker.
    Type,
    /// Dimension namespace, written with the `dim` marker.
    Dimension,
    /// Unit namespace, written with the `unit` marker.
    Unit,
    /// Index namespace, written with the `index` marker.
    Index,
}

impl ImportItemNamespace {
    /// Source marker for this category, or `None` for a bare term item.
    #[must_use]
    pub const fn marker(self) -> Option<&'static str> {
        match self {
            Self::Term => None,
            Self::Type => Some("type"),
            Self::Dimension => Some("dim"),
            Self::Unit => Some("unit"),
            Self::Index => Some("index"),
        }
    }

    /// Render the canonical import-item spelling for `name`.
    #[must_use]
    pub fn render_item(self, name: &str) -> String {
        self.marker()
            .map_or_else(|| name.to_string(), |marker| format!("{marker} {name}"))
    }
}

impl std::fmt::Display for ImportItemNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Term => "a term",
            Self::Type => "a type",
            Self::Dimension => "a dimension",
            Self::Unit => "a unit",
            Self::Index => "an index",
        })
    }
}

/// Typed details for a selective import whose marker names the wrong category.
///
/// The alternatives are non-empty and stay paired with the source name, so
/// diagnostics cannot lose or invent a category while crossing compiler layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItemCategoryMismatch {
    name: NameAtom,
    expected: ImportItemNamespace,
    alternatives: NonEmpty<ImportItemNamespace>,
}

impl ImportItemCategoryMismatch {
    #[must_use]
    pub const fn new(
        name: NameAtom,
        expected: ImportItemNamespace,
        alternatives: NonEmpty<ImportItemNamespace>,
    ) -> Self {
        Self {
            name,
            expected,
            alternatives,
        }
    }

    #[must_use]
    pub const fn name(&self) -> &NameAtom {
        &self.name
    }

    #[must_use]
    pub const fn expected(&self) -> ImportItemNamespace {
        self.expected
    }

    #[must_use]
    pub const fn alternatives(&self) -> &NonEmpty<ImportItemNamespace> {
        &self.alternatives
    }
}

impl std::fmt::Display for ImportItemCategoryMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let alternatives = self
            .alternatives
            .iter()
            .map(|namespace| format!("`{}`", namespace.render_item(self.name.as_str())))
            .collect::<Vec<_>>()
            .join(" or ");
        write!(
            f,
            "`{}` exists, but not as {}; did you mean {alternatives}?",
            self.name, self.expected
        )
    }
}
