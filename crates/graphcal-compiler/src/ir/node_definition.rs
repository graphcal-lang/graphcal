//! Lower a node body without representing an unfinished marker as an expression.

use crate::hir::expr::{ExprLowerError, ExprLoweringContext, lower_expr};
use crate::hir::node_definition::NodeDefinition;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Spanned;

pub(super) fn lower(
    definition: &crate::node_definition::NodeDefinition<
        crate::desugar::desugared_ast::Expr,
        ScopedName,
    >,
    context: ExprLoweringContext<'_>,
) -> Result<NodeDefinition, ExprLowerError> {
    match definition {
        crate::node_definition::NodeDefinition::Formula(expression) => {
            lower_expr(expression, context).map(NodeDefinition::Formula)
        }
        crate::node_definition::NodeDefinition::Todo(dependencies) => {
            let resolved = dependencies
                .value
                .iter()
                .map(|reference| crate::hir::expr::lower_graph_reference(reference, context))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(NodeDefinition::Todo(Spanned::new(
                resolved,
                dependencies.span,
            )))
        }
    }
}
