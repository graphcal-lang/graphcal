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

    let mut keys: Vec<ExpectedFailKey> = Vec::new();

    for arg in args {
        match arg {
            AttributeArg::Path { segments, span } => {
                // Must be exactly 2 segments: Index::Variant
                if segments.len() != 2 {
                    return Err(GraphcalError::ExpectedFailInvalidArg {
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                let index_name = IndexName::new(&segments[0].name);
                let variant_name = VariantName::new(&segments[1].name);
                keys.push(vec![(index_name, variant_name)]);
            }
            AttributeArg::Group { elements, span } => {
                // Each element must be a 2-segment Path
                let mut key: ExpectedFailKey = Vec::new();
                for elem in elements {
                    match elem {
                        AttributeArg::Path {
                            segments,
                            span: elem_span,
                        } => {
                            if segments.len() != 2 {
                                return Err(GraphcalError::ExpectedFailInvalidArg {
                                    src: src.clone(),
                                    span: (*elem_span).into(),
                                });
                            }
                            let index_name = IndexName::new(&segments[0].name);
                            let variant_name = VariantName::new(&segments[1].name);
                            key.push((index_name, variant_name));
                        }
                        AttributeArg::Group { span: g_span, .. } => {
                            return Err(GraphcalError::ExpectedFailInvalidArg {
                                src: src.clone(),
                                span: (*g_span).into(),
                            });
                        }
                    }
                }
                if key.is_empty() {
                    return Err(GraphcalError::ExpectedFailInvalidArg {
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                keys.push(key);
            }
        }
    }

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
