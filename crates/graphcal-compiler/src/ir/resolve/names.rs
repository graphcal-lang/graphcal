use std::sync::Arc;

use miette::NamedSource;

use crate::gcl_err;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey};
use crate::syntax::ast::AttributeArg;
use crate::syntax::names::{IndexName, VariantName};

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
                    _ => Err(gcl_err!(ExpectedFailInvalidArg {} @ src, *span)),
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
                            _ => Err(gcl_err!(ExpectedFailInvalidArg {} @ src, *elem_span)),
                        },
                        AttributeArg::Group { span: g_span, .. } => {
                            Err(gcl_err!(ExpectedFailInvalidArg {} @ src, *g_span))
                        }
                    })
                    .collect();
                let key = key?;
                if key.is_empty() {
                    Err(gcl_err!(ExpectedFailInvalidArg {} @ src, *span))
                } else {
                    Ok(key)
                }
            }
        })
        .collect::<Result<_, _>>()?;

    Ok(ExpectedFail::Variants(keys))
}

pub use crate::syntax::names::is_lower_snake_case;
