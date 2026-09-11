//! Closed-value bindings at the browser transport boundary.

use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_eval::eval::{StructuredBindingPathSegment, StructuredValueExpr};
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 size of one binding expression string.
pub const MAX_BINDING_EXPR_BYTES: usize = 4096;
/// Maximum recursive container depth accepted from JavaScript.
pub const MAX_STRUCTURED_BINDING_DEPTH: usize = 32;
/// Maximum total nodes in one structured value.
pub const MAX_STRUCTURED_BINDING_NODES: usize = 4096;

/// One reader-supplied parameter binding in Graphcal source syntax.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingRequest {
    pub name: String,
    pub expr: String,
}

/// Tagged recursive value sent by structured report controls.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredValueRequest {
    Literal {
        expr: String,
    },
    Algebraic {
        definition: usize,
        constructor: usize,
        fields: Vec<Self>,
    },
    Indexed {
        entries: Vec<Self>,
    },
}

/// One structured parameter binding.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredBindingRequest {
    pub name: String,
    pub value: StructuredValueRequest,
}

/// Compatible browser transport: CLI-style expressions remain accepted while
/// report controls use the checked structured form.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BrowserBindingRequest {
    Structured(StructuredBindingRequest),
    Expression(BindingRequest),
}

impl BrowserBindingRequest {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Structured(binding) => &binding.name,
            Self::Expression(binding) => &binding.name,
        }
    }
}

pub fn bind_one(
    builder: &mut graphcal_eval::eval::ParameterBindingBuilder<'_>,
    binding: &BindingRequest,
) -> Result<(), String> {
    if binding.expr.len() > MAX_BINDING_EXPR_BYTES {
        return Err(format!(
            "binding expression exceeds {MAX_BINDING_EXPR_BYTES} bytes"
        ));
    }
    let name = DeclName::try_new(&binding.name)
        .map_err(|error| format!("invalid parameter name: {error}"))?;
    let raw = graphcal_compiler::syntax::parser::Parser::new(&binding.expr)
        .parse_single_expr()
        .map_err(|error| error.to_string())?;
    let expr: graphcal_compiler::desugar::desugared_ast::Expr = raw.into();
    builder
        .bind_expression(&name, &expr)
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingPathSegmentView {
    Field { index: usize },
    Entry { index: usize },
}

#[derive(Debug, Clone)]
pub struct BrowserBindingError {
    pub message: String,
    pub path: Vec<BindingPathSegmentView>,
}

fn root_binding_error(message: impl Into<String>) -> BrowserBindingError {
    BrowserBindingError {
        message: message.into(),
        path: Vec::new(),
    }
}

pub fn bind_browser_one(
    builder: &mut graphcal_eval::eval::ParameterBindingBuilder<'_>,
    binding: &BrowserBindingRequest,
) -> Result<(), BrowserBindingError> {
    match binding {
        BrowserBindingRequest::Expression(binding) => {
            bind_one(builder, binding).map_err(root_binding_error)
        }
        BrowserBindingRequest::Structured(binding) => {
            let name = DeclName::try_new(&binding.name)
                .map_err(|error| root_binding_error(format!("invalid parameter name: {error}")))?;
            let mut nodes = 0;
            let value = decode_structured_value(&binding.value, 0, &mut nodes, &mut Vec::new())?;
            builder
                .bind_structured_expression(&name, &value)
                .map_err(|error| BrowserBindingError {
                    message: error.to_string(),
                    path: error
                        .path()
                        .iter()
                        .map(|segment| match segment {
                            StructuredBindingPathSegment::Field(index) => {
                                BindingPathSegmentView::Field { index: *index }
                            }
                            StructuredBindingPathSegment::Entry(index) => {
                                BindingPathSegmentView::Entry { index: *index }
                            }
                        })
                        .collect(),
                })
        }
    }
}

fn decode_structured_value(
    value: &StructuredValueRequest,
    depth: usize,
    nodes: &mut usize,
    path: &mut Vec<BindingPathSegmentView>,
) -> Result<StructuredValueExpr, BrowserBindingError> {
    let error = |message: String| BrowserBindingError {
        message,
        path: path.clone(),
    };
    if depth > MAX_STRUCTURED_BINDING_DEPTH {
        return Err(error(format!(
            "structured binding exceeds maximum depth {MAX_STRUCTURED_BINDING_DEPTH}"
        )));
    }
    *nodes += 1;
    if *nodes > MAX_STRUCTURED_BINDING_NODES {
        return Err(error(format!(
            "structured binding exceeds maximum size {MAX_STRUCTURED_BINDING_NODES}"
        )));
    }
    match value {
        StructuredValueRequest::Literal { expr } => {
            if expr.len() > MAX_BINDING_EXPR_BYTES {
                return Err(error(format!(
                    "binding literal exceeds {MAX_BINDING_EXPR_BYTES} bytes"
                )));
            }
            let raw = graphcal_compiler::syntax::parser::Parser::new(expr)
                .parse_single_expr()
                .map_err(|parse_error| error(parse_error.to_string()))?;
            Ok(StructuredValueExpr::Literal(raw.into()))
        }
        StructuredValueRequest::Algebraic {
            definition,
            constructor,
            fields,
        } => {
            let fields = fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    path.push(BindingPathSegmentView::Field { index });
                    let value = decode_structured_value(field, depth + 1, nodes, path);
                    path.pop();
                    value
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(StructuredValueExpr::Algebraic {
                definition: *definition,
                constructor: *constructor,
                fields,
            })
        }
        StructuredValueRequest::Indexed { entries } => {
            let entries = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    path.push(BindingPathSegmentView::Entry { index });
                    let value = decode_structured_value(entry, depth + 1, nodes, path);
                    path.pop();
                    value
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(StructuredValueExpr::Indexed { entries })
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn structured_binding_transport_rejects_unknown_fields() {
        let request = json!({
            "name": "value",
            "value": { "kind": "literal", "expr": "1", "extra": true }
        });
        assert!(serde_json::from_value::<BrowserBindingRequest>(request).is_err());
    }

    #[test]
    fn structured_binding_decoder_enforces_depth_node_and_leaf_limits() {
        let leaf = StructuredValueRequest::Literal {
            expr: "1".to_string(),
        };
        let too_deep = (0..=MAX_STRUCTURED_BINDING_DEPTH).fold(leaf.clone(), |value, _| {
            StructuredValueRequest::Indexed {
                entries: vec![value],
            }
        });
        assert!(decode_structured_value(&too_deep, 0, &mut 0, &mut Vec::new()).is_err());

        let too_large = StructuredValueRequest::Indexed {
            entries: vec![leaf; MAX_STRUCTURED_BINDING_NODES],
        };
        assert!(decode_structured_value(&too_large, 0, &mut 0, &mut Vec::new()).is_err());

        let oversized_leaf = StructuredValueRequest::Literal {
            expr: "a".repeat(MAX_BINDING_EXPR_BYTES + 1),
        };
        assert!(decode_structured_value(&oversized_leaf, 0, &mut 0, &mut Vec::new()).is_err());
    }
}
