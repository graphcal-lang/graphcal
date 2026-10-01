use crate::assertion_expectation::{ExpectedFail, ExpectedFailKeyPart};
use crate::desugar::desugared_ast::AttributeArg;
use crate::ir::resolve::collected::{ParsedExpectedFail, ParsedExpectedFailKey};
use crate::semantic_error::SemanticError;
use crate::semantic_error::attribute::AttributeError;
use crate::source_id::SourceId;
use crate::syntax::non_empty::NonEmpty;

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `IndexLabel` args (for example `Index#Label`) produce single-axis keys
/// - `FinitePosition` args (for example `#2`) produce structural keys
/// - `Group` args produce multi-axis keys
pub fn parse_expected_fail_args(
    args: &[AttributeArg],
    src: SourceId,
) -> Result<ParsedExpectedFail, SemanticError> {
    let keys: Vec<ParsedExpectedFailKey> = args
        .iter()
        .map(|arg| match arg {
            AttributeArg::IndexLabel { index, label, span } => {
                Ok(NonEmpty::singleton(ExpectedFailKeyPart::Named {
                    index: index.value.clone(),
                    variant: label.value.clone(),
                    span: *span,
                }))
            }
            AttributeArg::Path { path } => Err(SemanticError::located(
                src,
                path.span,
                AttributeError::ExpectedFailInvalidArg,
            )),
            AttributeArg::FinitePosition { position, span } => {
                Ok(NonEmpty::singleton(ExpectedFailKeyPart::FinitePosition {
                    position: *position,
                    span: *span,
                }))
            }
            AttributeArg::Group { elements, span } => {
                let key: Result<Vec<_>, SemanticError> = elements
                    .iter()
                    .map(|elem| match elem {
                        AttributeArg::IndexLabel { index, label, span } => {
                            Ok(ExpectedFailKeyPart::Named {
                                index: index.value.clone(),
                                variant: label.value.clone(),
                                span: *span,
                            })
                        }
                        AttributeArg::Path { path } => Err(SemanticError::located(
                            src,
                            path.span,
                            AttributeError::ExpectedFailInvalidArg,
                        )),
                        AttributeArg::FinitePosition { position, span } => {
                            Ok(ExpectedFailKeyPart::FinitePosition {
                                position: *position,
                                span: *span,
                            })
                        }
                        AttributeArg::Group { span: g_span, .. } => Err(SemanticError::located(
                            src,
                            *g_span,
                            AttributeError::ExpectedFailInvalidArg,
                        )),
                    })
                    .collect();
                NonEmpty::try_from_vec(key?).map_err(|_| {
                    SemanticError::located(src, *span, AttributeError::ExpectedFailInvalidArg)
                })
            }
        })
        .collect::<Result<_, _>>()?;

    Ok(NonEmpty::try_from_vec(keys).map_or(ExpectedFail::All, ExpectedFail::Variants))
}
