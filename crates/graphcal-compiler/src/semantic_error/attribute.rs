//! Diagnostics of declaration attributes and assertion annotations.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::declaration_kind::AttributeTarget;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::semantic::checked_type::{IndexDisplayName, TypeSpelling};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;

/// Diagnostics of declaration attributes and assertion annotations.
#[derive(Debug, Clone, Error)]
pub enum AttributeError {
    #[error("attribute `hidden` does not apply to include item `{name}`")]
    HiddenIncludeItemNotAPlot { name: NameAtom },
    #[error("cannot reference assert `{name}` with `@`")]
    GraphRefToAssert { name: DeclName },
    #[error("assert body must evaluate to Bool, got {found}")]
    AssertBodyNotBool { found: TypeSpelling },
    #[error("unknown assert `{name}` in #[assumes(...)]")]
    UnknownAssertInAssumes { name: DeclName },
    #[error("`#[assumes(...)]` is not valid on `{kind}` declarations")]
    InvalidAssumesTarget { kind: AttributeTarget },
    #[error("attribute `#[{name}]` appears more than once")]
    RepeatedSingletonAttribute { name: AttributeName, first: Span },
    #[error("`#[assumes(...)]` requires at least one assertion name")]
    EmptyAssumes,
    #[error("assertion `{name}` appears more than once in `#[assumes(...)]`")]
    DuplicateAssumesArgument { name: DeclName, first: Span },
    #[error("`#[assumes(...)]` arguments must be plain identifiers")]
    InvalidAssumesArgument,
    #[error("`#[lazy]` is reserved but not supported")]
    LazyNotSupported,
    #[error("attribute `hidden` does not apply to `{kind}` declarations")]
    InvalidHiddenTarget { kind: AttributeTarget },
    #[error("unknown attribute `{name}`")]
    UnknownAttribute {
        name: crate::syntax::token::SourceIdentifier,
    },
    #[error("`#[expected_fail]` is not valid on `{kind}` declarations")]
    InvalidExpectedFailTarget { kind: AttributeTarget },
    #[error(
        "invalid argument in `#[expected_fail(...)]`: expected `Index#Variant`, `module::Index#Variant`, `#N` (Fin axes), or grouped variants"
    )]
    ExpectedFailInvalidArg,
    #[error("`#[expected_fail(...)]` on non-indexed assertion")]
    ExpectedFailNotIndexed,
    #[error("`#[expected_fail]` without arguments on indexed assertion")]
    ExpectedFailAllOnIndexed,
    #[error("duplicate key in `#[expected_fail(...)]`")]
    ExpectedFailDuplicateKey,
    #[error("`#[expected_fail(...)]` key has the wrong index shape")]
    ExpectedFailKeyShapeMismatch { expected: usize, found: usize },
    #[error("`#[expected_fail(...)]` key does not belong to the assertion index")]
    ExpectedFailKeyIndexMismatch {
        expected: IndexDisplayName,
        found: Box<crate::assertion_expectation::ExpectedFailKeyPart>,
    },
    #[error("`#[expected_fail(...)]` finite-index position `#{position}` is out of bounds")]
    ExpectedFailFinitePositionOutOfBounds { position: u64, size: u64 },
    #[error("negative tolerance in tolerance assertion")]
    NegativeTolerance { value: f64 },
    #[error("`#[hidden]` takes no arguments")]
    HiddenTakesNoArguments,
}

impl DiagnosticKind for AttributeError {
    fn code(&self) -> &'static str {
        match self {
            Self::HiddenIncludeItemNotAPlot { .. } => "graphcal::A018",
            Self::GraphRefToAssert { .. } => "graphcal::A003",
            Self::AssertBodyNotBool { .. } => "graphcal::A004",
            Self::UnknownAssertInAssumes { .. } => "graphcal::A005",
            Self::InvalidAssumesTarget { .. } => "graphcal::A006",
            Self::RepeatedSingletonAttribute { .. } => "graphcal::A019",
            Self::EmptyAssumes => "graphcal::A020",
            Self::DuplicateAssumesArgument { .. } => "graphcal::A021",
            Self::InvalidAssumesArgument => "graphcal::A022",
            Self::LazyNotSupported => "graphcal::A023",
            Self::InvalidHiddenTarget { .. } => "graphcal::A017",
            Self::UnknownAttribute { .. } => "graphcal::A007",
            Self::InvalidExpectedFailTarget { .. } => "graphcal::A008",
            Self::ExpectedFailInvalidArg => "graphcal::A009",
            Self::ExpectedFailNotIndexed => "graphcal::A010",
            Self::ExpectedFailAllOnIndexed => "graphcal::A011",
            Self::ExpectedFailDuplicateKey => "graphcal::A012",
            Self::ExpectedFailKeyShapeMismatch { .. } => "graphcal::A013",
            Self::ExpectedFailKeyIndexMismatch { .. } => "graphcal::A014",
            Self::ExpectedFailFinitePositionOutOfBounds { .. } => "graphcal::A016",
            Self::NegativeTolerance { .. } => "graphcal::A015",
            Self::HiddenTakesNoArguments => "graphcal::A024",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::HiddenIncludeItemNotAPlot { .. } => Some("not a plot item".to_owned()),
            Self::GraphRefToAssert { name, .. } => {
                Some(format!("`@{name}` is an assert, not a param or node"))
            }
            Self::AssertBodyNotBool { found, .. } => Some(format!("expected Bool, found {found}")),
            Self::UnknownAssertInAssumes { .. } => Some("not an assert declaration".to_owned()),
            Self::InvalidAssumesTarget { .. } => Some("not a node or param".to_owned()),
            Self::RepeatedSingletonAttribute { name, .. } => {
                Some(format!("duplicate `#[{name}]` attribute"))
            }
            Self::EmptyAssumes => Some("no assertions named".to_owned()),
            Self::DuplicateAssumesArgument { .. } => Some("duplicate assertion name".to_owned()),
            Self::InvalidAssumesArgument => Some("not a plain assertion name".to_owned()),
            Self::LazyNotSupported => Some("lazy evaluation is not implemented".to_owned()),
            Self::InvalidHiddenTarget { .. } => Some("not a plot".to_owned()),
            Self::UnknownAttribute { .. } => Some("unknown attribute".to_owned()),
            Self::InvalidExpectedFailTarget { .. } => Some("not an assert".to_owned()),
            Self::ExpectedFailInvalidArg => Some("invalid argument".to_owned()),
            Self::ExpectedFailNotIndexed => Some("this assertion is not indexed".to_owned()),
            Self::ExpectedFailAllOnIndexed => Some("this assertion is indexed".to_owned()),
            Self::ExpectedFailDuplicateKey => Some("duplicate expected-fail key".to_owned()),
            Self::ExpectedFailKeyShapeMismatch {
                expected, found, ..
            } => Some(format!(
                "expected {expected} index axis/axes, found {found}"
            )),
            Self::ExpectedFailKeyIndexMismatch {
                expected, found, ..
            } => Some(format!(
                "expected index `{expected}`, found `{}`",
                found.display()
            )),
            Self::ExpectedFailFinitePositionOutOfBounds { position, size, .. } => {
                Some(format!("position #{position} on an axis of size {size}"))
            }
            Self::NegativeTolerance { value } => Some(format!(
                "tolerance is {}",
                if *value == 0.0 {
                    "-0".to_owned()
                } else {
                    crate::display::number::format_number(*value)
                }
            )),
            Self::HiddenTakesNoArguments => Some("error here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::HiddenIncludeItemNotAPlot { .. } => Some("`#[hidden]` on an include item is only valid when the item names a plot".to_owned()),
            Self::GraphRefToAssert { .. } => Some("assert declarations are post-evaluation checks and cannot be referenced with `@`".to_owned()),
            Self::AssertBodyNotBool { .. } => Some("assert declarations must have a body that evaluates to Bool".to_owned()),
            Self::UnknownAssertInAssumes { .. } => Some("`#[assumes(...)]` arguments must reference `assert` declarations".to_owned()),
            Self::InvalidAssumesTarget { .. } => Some("`#[assumes(...)]` is only valid on `node` and `param` declarations".to_owned()),
            Self::RepeatedSingletonAttribute { name, .. } => Some(format!("`#[{name}]` is singleton metadata; combine its contents into one attribute")),
            Self::EmptyAssumes => Some("name one or more distinct assertions, or remove the inert attribute".to_owned()),
            Self::DuplicateAssumesArgument { .. } => Some("each assertion may be named at most once by one declaration".to_owned()),
            Self::InvalidAssumesArgument => Some("name assertions directly, for example `#[assumes(first_check, second_check)]`".to_owned()),
            Self::LazyNotSupported => Some("remove `#[lazy]`; Graphcal currently evaluates nodes eagerly".to_owned()),
            Self::InvalidHiddenTarget { .. } => Some("`#[hidden]` suppresses a plot's standalone output; it is only valid on `plot` declarations".to_owned()),
            Self::UnknownAttribute { .. } => Some("recognized attributes are `#[assumes(...)]`, `#[expected_fail]`, `#[hidden]`, and `#[lazy]`".to_owned()),
            Self::InvalidExpectedFailTarget { .. } => Some("`#[expected_fail]` is only valid on `assert` declarations".to_owned()),
            Self::ExpectedFailInvalidArg
            | Self::HiddenTakesNoArguments => None,
            Self::ExpectedFailNotIndexed => Some("use `#[expected_fail]` without arguments for non-indexed assertions".to_owned()),
            Self::ExpectedFailAllOnIndexed => Some("use `#[expected_fail(Index#Variant, ...)]` (qualified `module::Index#Variant` also works) to specify which variants are expected to fail; for finite structural axes use `#[expected_fail(#N, ...)]`".to_owned()),
            Self::ExpectedFailDuplicateKey => Some("each expected-fail key must be unique".to_owned()),
            Self::ExpectedFailKeyShapeMismatch { .. } => Some("single-index assertions require `Index#Variant` keys; multi-index assertions require full tuple keys in assertion axis order".to_owned()),
            Self::ExpectedFailKeyIndexMismatch { .. } => Some("expected-fail keys must use the assertion's indexes in axis order".to_owned()),
            Self::ExpectedFailFinitePositionOutOfBounds { .. } => Some("finite-index positions in expected-fail keys must satisfy `0 <= N < size` for a `Fin(size)` axis".to_owned()),
            Self::NegativeTolerance { .. } => Some("a literal tolerance must not have a negative sign; use `0` for exact-match semantics".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::HiddenIncludeItemNotAPlot { .. }
            | Self::GraphRefToAssert { .. }
            | Self::AssertBodyNotBool { .. }
            | Self::UnknownAssertInAssumes { .. }
            | Self::InvalidAssumesTarget { .. }
            | Self::EmptyAssumes
            | Self::InvalidAssumesArgument
            | Self::LazyNotSupported
            | Self::InvalidHiddenTarget { .. }
            | Self::UnknownAttribute { .. }
            | Self::InvalidExpectedFailTarget { .. }
            | Self::ExpectedFailInvalidArg
            | Self::ExpectedFailNotIndexed
            | Self::ExpectedFailAllOnIndexed
            | Self::ExpectedFailDuplicateKey
            | Self::ExpectedFailKeyShapeMismatch { .. }
            | Self::ExpectedFailKeyIndexMismatch { .. }
            | Self::ExpectedFailFinitePositionOutOfBounds { .. }
            | Self::NegativeTolerance { .. }
            | Self::HiddenTakesNoArguments => Vec::new(),
            Self::RepeatedSingletonAttribute { first, name, .. } => vec![SecondaryLabel {
                span: *first,
                text: format!("first `#[{name}]` attribute"),
            }],
            Self::DuplicateAssumesArgument { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first named here".to_owned(),
            }],
        }
    }
}
