use std::sync::Arc;

use crate::desugar::resolved_ast::AttributeArg;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey};
use crate::syntax::names::{IndexName, IndexVariantName};
use miette::NamedSource;

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `Path` args (e.g. `Index.Variant`) → `ExpectedFail::Variants` with single-index keys
/// - `Group` args (e.g. `(Mode.Boost, Phase.Launch)`) → `ExpectedFail::Variants` with multi-index keys
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
                // Must be exactly 2 segments: Index.Variant
                match segments.len() {
                    2 => Ok(vec![(
                        IndexName::new(&segments[0].name),
                        IndexVariantName::new(&segments[1].name),
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
                        } => match segments.len() {
                            2 => Ok((
                                IndexName::new(&segments[0].name),
                                IndexVariantName::new(&segments[1].name),
                            )),
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
