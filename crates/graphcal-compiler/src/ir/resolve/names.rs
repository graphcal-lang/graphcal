use std::sync::Arc;

use crate::desugar::desugared_ast::AttributeArg;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart};
use crate::syntax::names::{IndexVariantName, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use miette::NamedSource;

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `Path` args (e.g. `Index.Variant` or `module.Index.Variant`) → `ExpectedFail::Variants` with single-index keys
/// - `RangeStep` args (e.g. `#2`, for Nat range axes) → `ExpectedFail::Variants` with single-index keys
/// - `Group` args (e.g. `(Mode.Boost, Phase.Launch)`, `(Mode.Boost, #2)`) → `ExpectedFail::Variants` with multi-index keys
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
                expected_fail_key_part_from_segments(segments, *span, src).map(|part| vec![part])
            }
            AttributeArg::RangeStep { step, span } => Ok(vec![ExpectedFailKeyPart::RangeStep {
                step: *step,
                span: *span,
            }]),
            AttributeArg::Group { elements, span } => {
                let key: Result<ExpectedFailKey, GraphcalError> = elements
                    .iter()
                    .map(|elem| match elem {
                        AttributeArg::Path {
                            segments,
                            span: elem_span,
                        } => expected_fail_key_part_from_segments(segments, *elem_span, src),
                        AttributeArg::RangeStep { step, span } => {
                            Ok(ExpectedFailKeyPart::RangeStep {
                                step: *step,
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

fn expected_fail_key_part_from_segments(
    segments: &NonEmpty<crate::syntax::ast::Ident>,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<ExpectedFailKeyPart, GraphcalError> {
    if segments.len() < 2 {
        return Err(GraphcalError::ExpectedFailInvalidArg {
            src: src.clone(),
            span: span.into(),
        });
    }
    let index_atoms = segments.as_slice()[..segments.len() - 1]
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    let index_path = NamePath::new(NonEmpty::try_from_vec(index_atoms).map_err(|_| {
        GraphcalError::ExpectedFailInvalidArg {
            src: src.clone(),
            span: span.into(),
        }
    })?);
    let variant = IndexVariantName::from_atom(segments.last().name.clone());
    Ok(ExpectedFailKeyPart::unresolved(index_path, variant, span))
}
