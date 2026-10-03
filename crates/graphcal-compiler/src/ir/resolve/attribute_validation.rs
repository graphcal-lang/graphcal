//! Pure structural validation for declaration and include-item attributes.
//!
//! Each [`AttributeSite`] decides which attributes it admits and hands every
//! admitted one out as its typed role, so a consumer never sees an attribute
//! its site cannot carry. Referenced-name resolution remains with the owning
//! compiler phases. This module enforces the source-shape invariants that must
//! be identical at every attribute boundary.

use std::collections::{HashMap, hash_map::Entry};

use thiserror::Error;

use crate::declaration_kind::{AttributeTarget, DeclarationKind};
use crate::desugar::desugared_ast::{AssertDecl, Attribute, AttributeArg, DeclKind};
use crate::semantic_error::SemanticError;
use crate::semantic_error::attribute::AttributeError;
use crate::source_id::SourceId;
use crate::syntax::attribute::{AttributeName, AttributeNameError, ReservedAttributeName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::{Span, Spanned};

/// A source site attributes attach to.
///
/// The site is the single applicability table for its kind of target: it
/// admits an attribute by handing out the attribute's role, which carries any
/// site data the role needs.
pub trait AttributeSite {
    /// The role of an attribute this site admits.
    type Role;

    /// The site, for diagnostics.
    fn target(&self) -> AttributeTarget;

    /// The role of `name` at this site, or `None` when the site does not
    /// admit it.
    fn admit(&self, name: AttributeName) -> Option<Self::Role>;
}

/// A declaration as an attribute site.
#[derive(Debug, Clone, Copy)]
pub struct DeclarationSite<'d> {
    kind: &'d DeclKind,
}

impl<'d> DeclarationSite<'d> {
    #[must_use]
    pub const fn new(kind: &'d DeclKind) -> Self {
        Self { kind }
    }
}

/// The role of an attribute a declaration admits.
#[derive(Debug, Clone, Copy)]
pub enum DeclarationAttributeRole<'d> {
    /// `#[assumes(...)]` on a param or node.
    Assumes,
    /// `#[expected_fail]` on the assertion it marks.
    ExpectedFail(&'d AssertDecl),
    /// `#[hidden]` on a plot.
    Hidden,
}

impl<'d> AttributeSite for DeclarationSite<'d> {
    type Role = DeclarationAttributeRole<'d>;

    fn target(&self) -> AttributeTarget {
        AttributeTarget::declaration(DeclarationKind::from_decl_kind(self.kind))
    }

    fn admit(&self, name: AttributeName) -> Option<Self::Role> {
        match (self.kind, name) {
            (DeclKind::Param(_) | DeclKind::Node(_), AttributeName::Assumes) => {
                Some(DeclarationAttributeRole::Assumes)
            }
            (DeclKind::Assert(assertion), AttributeName::ExpectedFail) => {
                Some(DeclarationAttributeRole::ExpectedFail(assertion))
            }
            (DeclKind::Plot(_), AttributeName::Hidden) => Some(DeclarationAttributeRole::Hidden),
            _ => None,
        }
    }
}

/// An include/import item as an attribute site.
#[derive(Debug, Clone)]
pub struct IncludeItemSite {
    /// Producer category when the include item names an assertion or plot.
    producer: Option<DeclarationKind>,
    /// Producer spelling written on the include item.
    name: NameAtom,
}

impl IncludeItemSite {
    #[must_use]
    pub const fn new(producer: Option<DeclarationKind>, name: NameAtom) -> Self {
        Self { producer, name }
    }
}

/// The role of an attribute an include/import item admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeItemAttributeRole {
    /// `#[expected_fail]` on an included assertion.
    ExpectedFail,
    /// `#[hidden]` on an included plot.
    Hidden,
}

impl AttributeSite for IncludeItemSite {
    type Role = IncludeItemAttributeRole;

    fn target(&self) -> AttributeTarget {
        AttributeTarget::include_item(self.producer, self.name.clone())
    }

    fn admit(&self, name: AttributeName) -> Option<Self::Role> {
        match (self.producer, name) {
            (Some(DeclarationKind::Assert), AttributeName::ExpectedFail) => {
                Some(IncludeItemAttributeRole::ExpectedFail)
            }
            (Some(DeclarationKind::Plot), AttributeName::Hidden) => {
                Some(IncludeItemAttributeRole::Hidden)
            }
            _ => None,
        }
    }
}

/// One admitted attribute: its role at the site, paired with its source syntax.
#[derive(Debug, Clone)]
pub struct ValidatedAttribute<'a, R> {
    role: R,
    attribute: &'a Attribute,
    assumes_arguments: Vec<Spanned<DeclName>>,
}

impl<'a, R: Copy> ValidatedAttribute<'a, R> {
    /// The attribute's role at its site.
    #[must_use]
    pub const fn role(&self) -> R {
        self.role
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
    UnknownAttribute {
        name: crate::syntax::token::SourceIdentifier,
        span: Span,
    },
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
    /// A `hidden` attribute was given arguments.
    #[error("`#[hidden]` takes no arguments")]
    HiddenArguments { span: Span },
    /// A reserved attribute name has no semantics yet.
    #[error("`#[{name}]` is reserved but not supported")]
    Reserved {
        name: ReservedAttributeName,
        span: Span,
    },
}

/// Parse known attributes, admit each at `site`, and enforce shared
/// singleton/argument invariants.
///
/// Singleton metadata is classified by [`AttributeName::is_singleton`].
/// `assumes` must also contain a non-empty set of unique plain assertion names,
/// and `hidden` accepts no arguments.
/// The function has no registry or I/O dependencies, so declarations and
/// include items cannot drift onto separate validation rules.
pub fn validate_attributes<'a, S: AttributeSite>(
    attributes: &'a [Attribute],
    site: &S,
) -> Result<Vec<ValidatedAttribute<'a, S::Role>>, AttributeValidationError> {
    let mut first_singletons = HashMap::new();

    attributes
        .iter()
        .map(|attribute| {
            let name = attribute
                .name
                .name
                .as_str()
                .parse::<AttributeName>()
                .map_err(|error| match error {
                    AttributeNameError::Unknown(_) => AttributeValidationError::UnknownAttribute {
                        name: attribute.name.name.clone(),
                        span: attribute.span,
                    },
                    AttributeNameError::Reserved(name) => AttributeValidationError::Reserved {
                        name,
                        span: attribute.span,
                    },
                })?;

            let role = site
                .admit(name)
                .ok_or_else(|| AttributeValidationError::InvalidTarget {
                    name,
                    target: site.target(),
                    span: attribute.span,
                })?;

            if name.is_singleton() {
                reject_repeated_singleton(name, &mut first_singletons, attribute.span)?;
            }

            let assumes_arguments = match name {
                AttributeName::Assumes => validate_assumes_arguments(attribute)?,
                AttributeName::Hidden => {
                    if !attribute.args.is_empty() {
                        return Err(AttributeValidationError::HiddenArguments {
                            span: attribute.span,
                        });
                    }
                    Vec::new()
                }
                AttributeName::ExpectedFail => Vec::new(),
            };

            Ok(ValidatedAttribute {
                role,
                attribute,
                assumes_arguments,
            })
        })
        .collect()
}

fn reject_repeated_singleton(
    name: AttributeName,
    first: &mut HashMap<AttributeName, Span>,
    duplicate: Span,
) -> Result<(), AttributeValidationError> {
    match first.entry(name) {
        Entry::Vacant(entry) => {
            entry.insert(duplicate);
            Ok(())
        }
        Entry::Occupied(entry) => Err(AttributeValidationError::RepeatedSingleton {
            name,
            first: *entry.get(),
            duplicate,
        }),
    }
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
                let (atom, span) = match argument {
                    AttributeArg::Path { path } => match path.value.as_bare() {
                        Some(atom) => (atom, path.span),
                        None => {
                            return Err(AttributeValidationError::InvalidAssumesArgument {
                                span: argument.span(),
                            });
                        }
                    },
                    AttributeArg::IndexLabel { .. }
                    | AttributeArg::FinitePosition { .. }
                    | AttributeArg::Group { .. } => {
                        return Err(AttributeValidationError::InvalidAssumesArgument {
                            span: argument.span(),
                        });
                    }
                };
                let name = DeclName::classify(atom.clone());
                seen.insert(name.clone(), span).map_or_else(
                    || {
                        names.push(Spanned::new(name.clone(), span));
                        Ok((seen, names))
                    },
                    |first| {
                        Err(AttributeValidationError::DuplicateAssumesArgument {
                            name: name.clone(),
                            first,
                            duplicate: span,
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
    src: SourceId,
) -> SemanticError {
    match error {
        AttributeValidationError::UnknownAttribute { name, span } => {
            SemanticError::located(src, span, AttributeError::UnknownAttribute { name })
        }
        AttributeValidationError::InvalidTarget { name, target, span } => match name {
            AttributeName::Assumes => SemanticError::located(
                src,
                span,
                AttributeError::InvalidAssumesTarget { kind: target },
            ),
            AttributeName::ExpectedFail => SemanticError::located(
                src,
                span,
                AttributeError::InvalidExpectedFailTarget { kind: target },
            ),
            AttributeName::Hidden => match target {
                AttributeTarget::IncludeItem { name, .. } => SemanticError::located(
                    src,
                    span,
                    AttributeError::HiddenIncludeItemNotAPlot { name },
                ),
                target @ AttributeTarget::Declaration(_) => SemanticError::located(
                    src,
                    span,
                    AttributeError::InvalidHiddenTarget { kind: target },
                ),
            },
        },
        AttributeValidationError::RepeatedSingleton {
            name,
            first,
            duplicate,
        } => SemanticError::located(
            src,
            duplicate,
            AttributeError::RepeatedSingletonAttribute { name, first },
        ),
        AttributeValidationError::EmptyAssumes { span } => {
            SemanticError::located(src, span, AttributeError::EmptyAssumes)
        }
        AttributeValidationError::InvalidAssumesArgument { span } => {
            SemanticError::located(src, span, AttributeError::InvalidAssumesArgument)
        }
        AttributeValidationError::DuplicateAssumesArgument {
            name,
            first,
            duplicate,
        } => SemanticError::located(
            src,
            duplicate,
            AttributeError::DuplicateAssumesArgument { name, first },
        ),
        AttributeValidationError::HiddenArguments { span } => {
            SemanticError::located(src, span, AttributeError::HiddenTakesNoArguments)
        }
        AttributeValidationError::Reserved { name, span } => match name {
            ReservedAttributeName::Lazy => {
                SemanticError::located(src, span, AttributeError::LazyNotSupported)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::desugar::desugared_ast::Declaration;
    use crate::syntax::parser::Parser;

    use super::*;

    fn declaration(source: &str) -> Declaration {
        crate::desugar::desugared_ast::File::from(Parser::new(source).parse_file().unwrap())
            .declarations
            .into_iter()
            .next()
            .unwrap()
    }

    fn include_item(producer: DeclarationKind) -> IncludeItemSite {
        IncludeItemSite::new(Some(producer), NameAtom::parse("output").unwrap())
    }

    #[test]
    fn singleton_attributes_report_first_and_duplicate_spans() {
        for source in [
            "#[expected_fail]\n#[expected_fail]\nassert check = false;",
            "#[assumes(first)]\n#[assumes(second)]\nnode output: Dimensionless = 1.0;",
            "#[hidden]\n#[hidden]\nplot output = { mark: line, encode: { x: 1.0, y: 1.0 } };",
        ] {
            let declaration = declaration(source);
            let attributes = &declaration.attributes;
            let error = validate_attributes(attributes, &DeclarationSite::new(&declaration.kind))
                .unwrap_err();
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
                "#[assumes(module::first)]\nnode output: Dimensionless = 1.0;",
                "invalid",
            ),
        ];

        for (source, expected) in cases {
            let declaration = declaration(source);
            let error = validate_attributes(
                &declaration.attributes,
                &DeclarationSite::new(&declaration.kind),
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
        for source in [
            "#[lazy]\nnode output: Dimensionless = 1.0;",
            "#[lazy(guard)]\nassert output = true;",
        ] {
            let declaration = declaration(source);
            assert!(matches!(
                validate_attributes(
                    &declaration.attributes,
                    &DeclarationSite::new(&declaration.kind)
                ),
                Err(AttributeValidationError::Reserved {
                    name: ReservedAttributeName::Lazy,
                    ..
                })
            ));
        }
    }

    #[test]
    fn hidden_rejects_arguments_on_plots_and_include_items() {
        let declaration = declaration(
            "#[hidden(now)]\nplot output = { mark: line, encode: { x: 1.0, y: 1.0 } };",
        );
        let attributes = &declaration.attributes;
        for result in [
            validate_attributes(attributes, &DeclarationSite::new(&declaration.kind)).map(|_| ()),
            validate_attributes(attributes, &include_item(DeclarationKind::Plot)).map(|_| ()),
        ] {
            assert_eq!(
                result,
                Err(AttributeValidationError::HiddenArguments {
                    span: attributes[0].span
                })
            );
        }
    }

    #[test]
    fn declarations_admit_each_attribute_on_its_own_kind_only() {
        let admitted = [
            "#[assumes(guard)]\nnode output: Dimensionless = 1.0;",
            "#[assumes(guard)]\nparam output: Dimensionless = 1.0;",
            "#[expected_fail]\nassert output = false;",
            "#[hidden]\nplot output = { mark: line, encode: { x: 1.0, y: 1.0 } };",
        ];
        for source in admitted {
            let declaration = declaration(source);
            let validated = validate_attributes(
                &declaration.attributes,
                &DeclarationSite::new(&declaration.kind),
            )
            .unwrap();
            let role = validated[0].role();
            assert!(
                matches!(
                    (&declaration.kind, role),
                    (
                        DeclKind::Node(_) | DeclKind::Param(_),
                        DeclarationAttributeRole::Assumes
                    ) | (DeclKind::Plot(_), DeclarationAttributeRole::Hidden)
                ) || matches!(
                    (&declaration.kind, role),
                    (DeclKind::Assert(marked), DeclarationAttributeRole::ExpectedFail(assertion))
                        if std::ptr::eq(marked, assertion)
                ),
                "case: {source}"
            );
        }

        let rejected = [
            (
                "#[expected_fail]\nnode output: Dimensionless = 1.0;",
                AttributeName::ExpectedFail,
            ),
            (
                "#[assumes(guard)]\nassert output = true;",
                AttributeName::Assumes,
            ),
            (
                "#[hidden]\nnode output: Dimensionless = 1.0;",
                AttributeName::Hidden,
            ),
        ];
        for (source, expected_name) in rejected {
            let declaration = declaration(source);
            assert!(matches!(
                validate_attributes(&declaration.attributes, &DeclarationSite::new(&declaration.kind)),
                Err(AttributeValidationError::InvalidTarget { name, .. })
                    if name == expected_name
            ));
        }
    }

    #[test]
    fn include_items_admit_expected_fail_on_assertions_and_hidden_on_plots() {
        let rejected = [
            (
                "#[assumes(guard)]\nnode output: Dimensionless = 1.0;",
                include_item(DeclarationKind::Node),
                AttributeName::Assumes,
            ),
            (
                "#[assumes(guard)]\nassert output = true;",
                include_item(DeclarationKind::Assert),
                AttributeName::Assumes,
            ),
            (
                "#[hidden]\nassert output = true;",
                include_item(DeclarationKind::Assert),
                AttributeName::Hidden,
            ),
            (
                "#[expected_fail]\nassert output = false;",
                IncludeItemSite::new(None, NameAtom::parse("output").unwrap()),
                AttributeName::ExpectedFail,
            ),
        ];
        for (source, site, expected_name) in rejected {
            assert!(matches!(
                validate_attributes(&declaration(source).attributes, &site),
                Err(AttributeValidationError::InvalidTarget { name, .. })
                    if name == expected_name
            ));
        }

        for (source, site, expected_role) in [
            (
                "#[expected_fail]\nassert output = false;",
                include_item(DeclarationKind::Assert),
                IncludeItemAttributeRole::ExpectedFail,
            ),
            (
                "#[hidden]\nnode output: Dimensionless = 1.0;",
                include_item(DeclarationKind::Plot),
                IncludeItemAttributeRole::Hidden,
            ),
        ] {
            let declaration = declaration(source);
            let validated = validate_attributes(&declaration.attributes, &site).unwrap();
            assert_eq!(validated[0].role(), expected_role);
        }
    }

    #[test]
    fn distinct_assumptions_are_valid_and_order_independent() {
        for source in [
            "#[assumes(first, second)]\nnode output: Dimensionless = 1.0;",
            "#[assumes(second, first)]\nnode output: Dimensionless = 1.0;",
        ] {
            let declaration = declaration(source);
            assert!(
                validate_attributes(
                    &declaration.attributes,
                    &DeclarationSite::new(&declaration.kind),
                )
                .is_ok()
            );
        }
    }
}
