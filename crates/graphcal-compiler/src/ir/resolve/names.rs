use std::sync::Arc;

use crate::assertion_expectation::{ExpectedFail, ExpectedFailKeyPart};
use crate::desugar::desugared_ast::AttributeArg;
use crate::graphcal_error::GraphcalError;
use crate::ir::resolve::collected::{ParsedExpectedFail, ParsedExpectedFailKey};
use crate::syntax::non_empty::NonEmpty;
use miette::NamedSource;

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `IndexLabel` args (for example `Index#Label`) produce single-axis keys
/// - `FinitePosition` args (for example `#2`) produce structural keys
/// - `Group` args produce multi-axis keys
pub fn parse_expected_fail_args(
    args: &[AttributeArg],
    src: &NamedSource<Arc<String>>,
) -> Result<ParsedExpectedFail, GraphcalError> {
    let keys: Vec<ParsedExpectedFailKey> = args
        .iter()
        .map(|arg| match arg {
            AttributeArg::IndexLabel { index, label, span } => {
                Ok(vec![ExpectedFailKeyPart::Named {
                    index: index.value.clone(),
                    variant: label.value.clone(),
                    span: *span,
                }])
            }
            AttributeArg::Path { path } => Err(GraphcalError::ExpectedFailInvalidArg {
                src: src.clone(),
                span: path.span.into(),
            }),
            AttributeArg::FinitePosition { position, span } => {
                Ok(vec![ExpectedFailKeyPart::FinitePosition {
                    position: *position,
                    span: *span,
                }])
            }
            AttributeArg::Group { elements, span } => {
                let key: Result<ParsedExpectedFailKey, GraphcalError> = elements
                    .iter()
                    .map(|elem| match elem {
                        AttributeArg::IndexLabel { index, label, span } => {
                            Ok(ExpectedFailKeyPart::Named {
                                index: index.value.clone(),
                                variant: label.value.clone(),
                                span: *span,
                            })
                        }
                        AttributeArg::Path { path } => Err(GraphcalError::ExpectedFailInvalidArg {
                            src: src.clone(),
                            span: path.span.into(),
                        }),
                        AttributeArg::FinitePosition { position, span } => {
                            Ok(ExpectedFailKeyPart::FinitePosition {
                                position: *position,
                                span: *span,
                            })
                        }
                        AttributeArg::Group { span: g_span, .. } => {
                            Err(GraphcalError::ExpectedFailInvalidArg {
                                src: src.clone(),
                                span: (*g_span).into(),
                            })
                        }
                    })
                    .collect();
                let key = key?;
                if key.is_empty() {
                    Err(GraphcalError::ExpectedFailInvalidArg {
                        src: src.clone(),
                        span: (*span).into(),
                    })
                } else {
                    Ok(key)
                }
            }
        })
        .collect::<Result<_, _>>()?;

    Ok(NonEmpty::try_from_vec(keys).map_or(ExpectedFail::All, ExpectedFail::Variants))
}
