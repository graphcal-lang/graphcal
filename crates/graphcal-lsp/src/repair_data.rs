//! Typed quick-fix payloads carried in `Diagnostic::data`.
//!
//! A diagnostic that has a mechanical repair carries one [`RepairData`]
//! value. The diagnostics producer builds it from the compiler's typed error,
//! and the code-action handler dispatches on the variant, never on the
//! diagnostic's code string or on string-keyed JSON fields. The JSON shape
//! exists only on the wire between the two.

use tower_lsp::lsp_types::Diagnostic;

/// Category marker an unknown-name auto-import inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoImportCategory {
    Term,
    Type,
    Dimension,
    Unit,
    Index,
}

impl AutoImportCategory {
    pub const fn namespace(self) -> graphcal_compiler::syntax::ast::ImportItemNamespace {
        use graphcal_compiler::syntax::ast::ImportItemNamespace;
        match self {
            Self::Term => ImportItemNamespace::Term,
            Self::Type => ImportItemNamespace::Type,
            Self::Dimension => ImportItemNamespace::Dimension,
            Self::Unit => ImportItemNamespace::Unit,
            Self::Index => ImportItemNamespace::Index,
        }
    }
}

/// The visibility a "make visible" repair adds to a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddedVisibility {
    /// `pub`: the item is referenced from a visible signature (V003, V006).
    Public,
    /// `pub(bind)`: a required item is part of the bindable interface (V002).
    Bindable,
}

impl AddedVisibility {
    /// The source prefix inserted before the declaration.
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Public => "pub ",
            Self::Bindable => "pub(bind) ",
        }
    }

    /// The prefix as shown in a code-action title.
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Public => "pub",
            Self::Bindable => "pub(bind)",
        }
    }
}

/// One diagnostic's mechanical repair.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RepairData {
    /// D020: replace the decimal exponent with the exactly equal rational
    /// the compiler proved.
    ExactExponent { replacement: String },
    /// C011: replace an integer-literal bound of a `Dimensionless` domain
    /// with the same number written as a `Dimensionless` literal.
    DimensionlessBound { replacement: String },
    /// Add `visibility` to the declaration of the private item `item`.
    AddVisibility {
        item: String,
        visibility: AddedVisibility,
    },
    /// V008: add `type T, …` for each listed type to the selection of the
    /// selective include the diagnostic spans.
    SelectTypes { types: Vec<String> },
    /// Import an unknown bare name from a loaded module that exports it.
    AutoImport {
        name: String,
        category: AutoImportCategory,
    },
}

impl RepairData {
    /// The wire form attached to a published diagnostic.
    pub fn to_value(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }

    /// The repair a diagnostic sent back by the client carries, if any.
    pub fn of(diagnostic: &Diagnostic) -> Option<Self> {
        serde_json::from_value(diagnostic.data.clone()?).ok()
    }

    /// The repair of a diagnostic the client sent back, recovered from the
    /// one `published` diagnostic with the same code and range when the
    /// client dropped `data` (it does not support diagnostic data).
    pub fn recover(diagnostic: &Diagnostic, published: &[Diagnostic]) -> Option<Self> {
        Self::of(diagnostic).or_else(|| {
            let mut matching = published.iter().filter(|candidate| {
                candidate.code == diagnostic.code && candidate.range == diagnostic.range
            });
            let only = matching.next()?;
            matching.next().is_none().then(|| Self::of(only)).flatten()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_repair_round_trips_through_diagnostic_data() {
        for repair in [
            RepairData::ExactExponent {
                replacement: "(1/4)".to_string(),
            },
            RepairData::DimensionlessBound {
                replacement: "-3.0".to_string(),
            },
            RepairData::AddVisibility {
                item: "Velocity".to_string(),
                visibility: AddedVisibility::Bindable,
            },
            RepairData::AutoImport {
                name: "Missing".to_string(),
                category: AutoImportCategory::Dimension,
            },
            RepairData::SelectTypes {
                types: vec!["T".to_string(), "U".to_string()],
            },
        ] {
            let diagnostic = Diagnostic {
                data: repair.to_value(),
                ..Default::default()
            };
            assert_eq!(RepairData::of(&diagnostic), Some(repair));
        }
        assert_eq!(
            RepairData::AddVisibility {
                item: "Velocity".to_string(),
                visibility: AddedVisibility::Public,
            }
            .to_value(),
            Some(serde_json::json!({
                "kind": "add_visibility",
                "item": "Velocity",
                "visibility": "public",
            }))
        );
        assert_eq!(RepairData::of(&Diagnostic::default()), None);
    }

    #[test]
    fn a_repair_is_recovered_only_from_one_matching_published_diagnostic() {
        let repair = RepairData::ExactExponent {
            replacement: "(1/4)".to_string(),
        };
        let code = Some(tower_lsp::lsp_types::NumberOrString::String(
            "graphcal::D020".to_string(),
        ));
        let published = Diagnostic {
            code: code.clone(),
            data: repair.to_value(),
            ..Default::default()
        };
        let returned = Diagnostic {
            code,
            ..Default::default()
        };
        assert_eq!(
            RepairData::recover(&returned, std::slice::from_ref(&published)),
            Some(repair)
        );
        assert_eq!(
            RepairData::recover(&returned, &[published.clone(), published]),
            None
        );
        assert_eq!(RepairData::recover(&returned, &[]), None);
    }
}
