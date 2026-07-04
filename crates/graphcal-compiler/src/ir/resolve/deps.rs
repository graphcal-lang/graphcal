use crate::desugar::desugared_ast::{Expr, ExprKind};
use crate::syntax::module_name::ScopedName;
use crate::syntax::visitor::ExprVisitor;

/// Sink for graph-reference observations during an AST walk.
trait GraphRefSink {
    /// Called for each `@`-reference. `scoped` is the typed name
    /// (`Local` for bare `@name`, `Qualified` for `@alias.member` after
    /// the namespace-alias rewrite).
    fn on_ref(&mut self, scoped: &ScopedName);
}

/// Generic visitor that routes every observed graph reference through a
/// [`GraphRefSink`].
struct GraphRefVisitor<'a, S: GraphRefSink> {
    sink: &'a mut S,
}

impl<S: GraphRefSink> ExprVisitor<crate::syntax::phase::Desugared> for GraphRefVisitor<'_, S> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            self.sink.on_ref(&ident.value);
        }
        Ok(())
    }
}

/// Returns `true` if `expr` contains any graph reference (local or qualified).
#[must_use]
pub fn contains_graph_ref(expr: &Expr) -> bool {
    struct DetectorSink(bool);
    impl GraphRefSink for DetectorSink {
        fn on_ref(&mut self, _scoped: &ScopedName) {
            self.0 = true;
        }
    }
    let mut sink = DetectorSink(false);
    let mut visitor = GraphRefVisitor { sink: &mut sink };
    let _ = visitor.visit_expr(expr);
    sink.0
}
