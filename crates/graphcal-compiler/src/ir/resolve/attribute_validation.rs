//! Pure structural validation for declaration and include-item attributes.
//!
//! Target applicability and referenced-name resolution remain with their
//! owning compiler phases. This module enforces the source-shape invariants
//! that must be identical at every attribute boundary.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::desugar::desugared_ast::{Attribute, AttributeArg};
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::AttributeTarget;
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::{Span, Spanned};

/// One known attribute paired with its source syntax.
#[derive(Debug, Clone)]
pub struct ValidatedAttribute<'a> {
    name: AttributeName,
    attribute: &'a Attribute,
    assumes_arguments: Vec<Spanned<DeclName>>,
}

impl<'a> ValidatedAttribute<'a> {
    /// The typed attribute name.
    #[must_use]
    pub const fn name(&self) -> AttributeName {
        self.name
    }

    /// The validated source attribute.
    #[must_use]
    pub const fn attribute(&self) -> &'a Attribute {
        self.attribute
    }

    /// Typed assertion names supplied to an `assumes` attribute.
    ///
    /// This is empty for every other attribute kind.
    #[must_use]
    pub fn assumes_arguments(&self) -> &[Spanned<DeclName>] {
        &self.assumes_arguments
    }
}

/// Structural errors shared by declaration and include-item attributes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AttributeValidationError {
    /// The attribute name is not part of the language vocabulary.
    #[error("unknown attribute `{name}`")]
    UnknownAttribute { name: String, span: Span },
    /// A known attribute does not apply at this source target.
    #[error("`#[{name}]` is not valid on `{target}`")]
    InvalidTarget {
        name: AttributeName,
        target: AttributeTarget,
        span: Span,
    },
    /// Singleton metadata was supplied more than once to one target.
    #[error("attribute `#[{name}]` appears more than once")]
    RepeatedSingleton {
        name: AttributeName,
        first: Span,
        duplicate: Span,
    },
    /// An `assumes` attribute did not name any assertion.
    #[error("`#[assumes(...)]` requires at least one assertion name")]
    EmptyAssumes { span: Span },
    /// An `assumes` argument was not one plain identifier.
    #[error("`#[assumes(...)]` arguments must be plain identifiers")]
    InvalidAssumesArgument { span: Span },
    /// An assertion name was repeated in one `assumes` argument list.
    #[error("assertion `{name}` appears more than once in `#[assumes(...)]`")]
    DuplicateAssumesArgument {
        name: DeclName,
        first: Span,
        duplicate: Span,
    },
    /// Lazy evaluation syntax is reserved but has no semantics yet.
    #[error("`#[lazy]` is reserved but not supported")]
    UnsupportedLazy { span: Span },
}

/// Parse known attributes and enforce shared singleton/argument invariants.
///
/// `assumes` and `expected_fail` are singleton metadata. `assumes` must also
/// contain a non-empty set of unique plain assertion names. The function has
/// no registry or I/O dependencies, so declarations and include items cannot
/// drift onto separate validation rules.
pub fn validate_attributes<'a>(
    attributes: &'a [Attribute],
    target: &AttributeTarget,
) -> Result<Vec<ValidatedAttribute<'a>>, AttributeValidationError> {
    let mut first_assumes = None;
    let mut first_expected_fail = None;

    attributes
        .iter()
        .map(|attribute| {
            let name = attribute
                .name
                .name
                .parse::<AttributeName>()
                .map_err(|error| AttributeValidationError::UnknownAttribute {
                    name: error.into_raw(),
                    span: attribute.span,
                })?;

            if matches!(name, AttributeName::Lazy) {
                return Err(AttributeValidationError::UnsupportedLazy {
                    span: attribute.span,
                });
            }
            if !target.accepts(name) {
                return Err(AttributeValidationError::InvalidTarget {
                    name,
                    target: target.clone(),
                    span: attribute.span,
                });
            }

            let assumes_arguments = match name {
                AttributeName::Assumes => {
                    reject_repeated_singleton(name, &mut first_assumes, attribute.span)?;
                    validate_assumes_arguments(attribute)?
                }
                AttributeName::ExpectedFail => {
                    reject_repeated_singleton(name, &mut first_expected_fail, attribute.span)?;
                    Vec::new()
                }
                AttributeName::Hidden | AttributeName::Lazy => Vec::new(),
            };

            Ok(ValidatedAttribute {
                name,
                attribute,
                assumes_arguments,
            })
        })
        .collect()
}

const fn reject_repeated_singleton(
    name: AttributeName,
    first: &mut Option<Span>,
    duplicate: Span,
) -> Result<(), AttributeValidationError> {
    let Some(first_span) = *first else {
        *first = Some(duplicate);
        return Ok(());
    };
    Err(AttributeValidationError::RepeatedSingleton {
        name,
        first: first_span,
        duplicate,
    })
}

fn validate_assumes_arguments(
    attribute: &Attribute,
) -> Result<Vec<Spanned<DeclName>>, AttributeValidationError> {
    if attribute.args.is_empty() {
        return Err(AttributeValidationError::EmptyAssumes {
            span: attribute.span,
        });
    }

    attribute
        .args
        .iter()
        .try_fold(
            (HashMap::<DeclName, Span>::new(), Vec::new()),
            |(mut seen, mut names), argument| {
                let ident = match argument {
                    AttributeArg::Path { segments, .. } if segments.len() == 1 => segments.first(),
                    AttributeArg::Path { .. }
                    | AttributeArg::FinitePosition { .. }
                    | AttributeArg::Group { .. } => {
                        return Err(AttributeValidationError::InvalidAssumesArgument {
                            span: argument.span(),
                        });
                    }
                };
                let name = DeclName::from_atom(ident.name.clone());
                seen.insert(name.clone(), ident.span).map_or_else(
                    || {
                        names.push(Spanned::new(name.clone(), ident.span));
                        Ok((seen, names))
                    },
                    |first| {
                        Err(AttributeValidationError::DuplicateAssumesArgument {
                            name: name.clone(),
                            first,
                            duplicate: ident.span,
                        })
                    },
                )
            },
        )
        .map(|(_, names)| names)
}

/// Attach source text to a pure structural attribute error.
#[must_use]
pub fn attribute_validation_error_to_graphcal(
    error: AttributeValidationError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    match error {
        AttributeValidationError::UnknownAttribute { name, span } => {
            GraphcalError::UnknownAttribute {
                name,
                src: src.clone(),
                span: span.into(),
            }
        }
        AttributeValidationError::InvalidTarget { name, target, span } => match name {
            AttributeName::Assumes => GraphcalError::InvalidAssumesTarget {
                kind: target,
                src: src.clone(),
                span: span.into(),
            },
            AttributeName::ExpectedFail => GraphcalError::InvalidExpectedFailTarget {
                kind: target,
                src: src.clone(),
                span: span.into(),
            },
            AttributeName::Hidden => match target {
                AttributeTarget::IncludeItem { name, .. } => {
                    GraphcalError::HiddenIncludeItemNotAPlot {
                        name: name.to_string(),
                        src: src.clone(),
                        span: span.into(),
                    }
                }
                target @ AttributeTarget::Declaration(_) => GraphcalError::InvalidHiddenTarget {
                    kind: target,
                    src: src.clone(),
                    span: span.into(),
                },
            },
            AttributeName::Lazy => GraphcalError::LazyNotSupported {
                src: src.clone(),
                span: span.into(),
            },
        },
        AttributeValidationError::RepeatedSingleton {
            name,
            first,
            duplicate,
        } => GraphcalError::RepeatedSingletonAttribute {
            name,
            src: src.clone(),
            first: first.into(),
            duplicate: duplicate.into(),
        },
        AttributeValidationError::EmptyAssumes { span } => GraphcalError::EmptyAssumes {
            src: src.clone(),
            span: span.into(),
        },
        AttributeValidationError::InvalidAssumesArgument { span } => {
            GraphcalError::InvalidAssumesArgument {
                src: src.clone(),
                span: span.into(),
            }
        }
        AttributeValidationError::DuplicateAssumesArgument {
            name,
            first,
            duplicate,
        } => GraphcalError::DuplicateAssumesArgument {
            name,
            src: src.clone(),
            first: first.into(),
            duplicate: duplicate.into(),
        },
        AttributeValidationError::UnsupportedLazy { span } => GraphcalError::LazyNotSupported {
            src: src.clone(),
            span: span.into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::registry::resolve_types::DeclarationKind;
    use crate::syntax::parser::Parser;

    use super::*;

    fn attributes(source: &str) -> Vec<Attribute> {
        Parser::new(source)
            .parse_file()
            .unwrap()
            .declarations
            .into_iter()
            .next()
            .unwrap()
            .attributes
    }

    #[test]
    fn singleton_attributes_report_first_and_duplicate_spans() {
        for (source, target) in [
            (
                "#[expected_fail]\n#[expected_fail]\nassert check = false;",
                AttributeTarget::declaration(DeclarationKind::Assert),
            ),
            (
                "#[assumes(first)]\n#[assumes(second)]\nnode output: Dimensionless = 1.0;",
                AttributeTarget::declaration(DeclarationKind::Node),
            ),
        ] {
            let attributes = attributes(source);
            let error = validate_attributes(&attributes, &target).unwrap_err();
            assert!(matches!(
                error,
                AttributeValidationError::RepeatedSingleton {
                    first,
                    duplicate,
                    ..
                } if first == attributes[0].span && duplicate == attributes[1].span
            ));
        }
    }

    #[test]
    fn assumes_requires_non_empty_unique_plain_names() {
        let cases = [
            ("#[assumes]\nnode output: Dimensionless = 1.0;", "empty"),
            (
                "#[assumes(first, first)]\nnode output: Dimensionless = 1.0;",
                "duplicate",
            ),
            (
                "#[assumes(module.first)]\nnode output: Dimensionless = 1.0;",
                "invalid",
            ),
        ];

        for (source, expected) in cases {
            let error = validate_attributes(
                &attributes(source),
                &AttributeTarget::declaration(DeclarationKind::Node),
            )
            .unwrap_err();
            assert!(
                matches!(
                    (expected, error),
                    ("empty", AttributeValidationError::EmptyAssumes { .. })
                        | (
                            "duplicate",
                            AttributeValidationError::DuplicateAssumesArgument { .. }
                        )
                        | (
                            "invalid",
                            AttributeValidationError::InvalidAssumesArgument { .. }
                        )
                ),
                "case: {expected}"
            );
        }
    }

    #[test]
    fn lazy_is_rejected_regardless_of_target_or_arguments() {
        for (source, target) in [
            (
                "#[lazy]\nnode output: Dimensionless = 1.0;",
                AttributeTarget::declaration(DeclarationKind::Node),
            ),
            (
                "#[lazy(guard)]\nassert output = true;",
                AttributeTarget::declaration(DeclarationKind::Assert),
            ),
        ] {
            assert!(matches!(
                validate_attributes(&attributes(source), &target),
                Err(AttributeValidationError::UnsupportedLazy { .. })
            ));
        }
    }

    #[test]
    fn distinct_assumptions_are_valid_and_order_independent() {
        for source in [
            "#[assumes(first, second)]\nnode output: Dimensionless = 1.0;",
            "#[assumes(second, first)]\nnode output: Dimensionless = 1.0;",
        ] {
            assert!(
                validate_attributes(
                    &attributes(source),
                    &AttributeTarget::declaration(DeclarationKind::Node),
                )
                .is_ok()
            );
        }
    }
}
