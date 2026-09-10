//! Closed-value bindings at the browser transport boundary.

use graphcal_compiler::syntax::decl_name::DeclName;
use serde::Deserialize;

/// Maximum UTF-8 size of one binding expression string.
pub const MAX_BINDING_EXPR_BYTES: usize = 4096;

/// One reader-supplied parameter binding in Graphcal source syntax.
#[derive(Debug, Clone, Deserialize)]
pub struct BindingRequest {
    pub name: String,
    pub expr: String,
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
