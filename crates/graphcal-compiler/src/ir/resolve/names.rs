use std::sync::Arc;

use crate::desugar::desugared_ast::AttributeArg;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{
    ExpectedFail, ExpectedFailKeyPart, ParsedExpectedFail, ParsedExpectedFailKey,
};
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
    if args.is_empty() {
        return Ok(ExpectedFail::All);
    }

    let keys: Vec<ParsedExpectedFailKey> = args
        .iter()
        .map(|arg| match arg {
            AttributeArg::IndexLabel {
                index,
                label,
                span,
            } => Ok(vec![ExpectedFailKeyPart::parsed(
                index.value.clone(),
                label.value.clone(),
                *span,
            )]),
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
                        AttributeArg::IndexLabel {
                            index,
                            label,
                            span,
                        } => Ok(ExpectedFailKeyPart::parsed(
                            index.value.clone(),
                            label.value.clone(),
                            *span,
                        )),
                        AttributeArg::Path { path } => {
                            Err(GraphcalError::ExpectedFailInvalidArg {
                                src: src.clone(),
                                span: path.span.into(),
                            })
                        }
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

    Ok(ExpectedFail::Variants(keys))
}
