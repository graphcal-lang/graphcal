use std::sync::Arc;

use miette::NamedSource;

use graphcal_registry::error::GraphcalError;
use graphcal_registry::resolve_types::{ExpectedFail, ExpectedFailKey};
use graphcal_syntax::ast::AttributeArg;
use graphcal_syntax::names::{IndexName, VariantName};

/// Parse `#[expected_fail]` attribute arguments into an [`ExpectedFail`] value.
///
/// - No args → `ExpectedFail::All`
/// - `Path` args (e.g. `Index::Variant`) → `ExpectedFail::Variants` with single-index keys
/// - `Group` args (e.g. `(Mode::Boost, Phase::Launch)`) → `ExpectedFail::Variants` with multi-index keys
pub(super) fn parse_expected_fail_args(
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

#[must_use]
pub fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[must_use]
pub fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
