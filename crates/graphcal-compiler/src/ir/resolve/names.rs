use std::sync::Arc;

use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey};
use crate::syntax::ast::AttributeArg;
use crate::syntax::names::{IndexName, VariantName};
use miette::NamedSource;

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `Path` args (e.g. `Index::Variant`) → `ExpectedFail::Variants` with single-index keys
/// - `Group` args (e.g. `(Mode::Boost, Phase::Launch)`) → `ExpectedFail::Variants` with multi-index keys
pub fn parse_expected_fail_args(
    args: &[AttributeArg],
    src: &NamedSource<Arc<String>>,
) -> Result<ExpectedFail, GraphcalError> {
    if args.is_empty() {
        return Ok(ExpectedFail::All);
    }

    let keys: Vec<ExpectedFailKey> = args
        .iter()
        .map(|arg| match arg {
            AttributeArg::Path { segments, span } => {
                // Must be exactly 2 segments: Index::Variant
                match segments.as_slice() {
                    [index, variant] => Ok(vec![(
                        IndexName::new(&index.name),
                        VariantName::new(&variant.name),
                    )]),
                    _ => Err(GraphcalError::ExpectedFailInvalidArg {
                        src: src.clone(),
                        span: (*span).into(),
                    }),
                }
            }
            AttributeArg::Group { elements, span } => {
                // Each element must be a 2-segment Path
                let key: Result<ExpectedFailKey, GraphcalError> = elements
                    .iter()
                    .map(|elem| match elem {
                        AttributeArg::Path {
                            segments,
                            span: elem_span,
                        } => match segments.as_slice() {
                            [index, variant] => {
                                Ok((IndexName::new(&index.name), VariantName::new(&variant.name)))
                            }
                            _ => Err(GraphcalError::ExpectedFailInvalidArg {
                                src: src.clone(),
                                span: (*elem_span).into(),
                            }),
                        },
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
