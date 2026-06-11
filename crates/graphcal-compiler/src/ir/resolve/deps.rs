use std::collections::HashSet;

use crate::desugar::desugared_ast::{Expr, ExprKind};
use crate::syntax::names::ScopedName;
use crate::syntax::visitor::ExprVisitor;

/// Sink for graph-reference observations during an AST walk.
///
/// This trait is the single extension point for the four historical collectors
/// (`KnownGraphRefCollector`, `GraphRefCollector`, `GraphRefNameCollector`,
/// `GraphRefDetector`). Implementors record the observation however they
/// like — as a typed `ScopedName`, a raw `String`, or a `bool` flag.
trait GraphRefSink {
    /// Called for each `@`-reference. `scoped` is the typed name
    /// (`Local` for bare `@name`, `Qualified` for `@alias.member` after
    /// the namespace-alias rewrite).
    fn on_ref(&mut self, scoped: &ScopedName);
}

/// Generic visitor that routes every observed graph reference through a
/// [`GraphRefSink`]. Optionally filters to names in `known_names`.
struct GraphRefVisitor<'a, S: GraphRefSink> {
    sink: &'a mut S,
    known_names: Option<&'a HashSet<&'a str>>,
}

impl<S: GraphRefSink> GraphRefVisitor<'_, S> {
    /// Filter against the (still flat-string) `known_names` set.
    /// Boundary stringification — see module note.
    fn should_record(&self, scoped: &ScopedName) -> bool {
        self.known_names
            .is_none_or(|set| set.contains(scoped.to_string().as_str()))
    }
}

impl<S: GraphRefSink> ExprVisitor<crate::syntax::phase::Desugared> for GraphRefVisitor<'_, S> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind
            && self.should_record(&ident.value)
        {
            self.sink.on_ref(&ident.value);
        }
        Ok(())
    }
}

/// Sink that stores every observed ref name as a flat `String` (the
/// qualified `module.member` form for qualified refs, bare for locals).
/// Boundary stringification — callers that want the typed form should use
/// `collect_scoped_graph_refs` instead.
struct StringNameSink<'a> {
    refs: &'a mut HashSet<String>,
}

impl GraphRefSink for StringNameSink<'_> {
    fn on_ref(&mut self, scoped: &ScopedName) {
        self.refs.insert(scoped.to_string());
    }
}

/// Collect `@`-references (graph refs) from an expression.
///
/// This is a lightweight version of `collect_all_refs` used for re-extracting
/// runtime dependencies after an override expression replaces a param's default.
/// Only collects names that exist in `all_runtime_names`.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn collect_graph_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    refs: &mut HashSet<String>,
) {
    let mut sink = StringNameSink { refs };
    let mut visitor = GraphRefVisitor {
        sink: &mut sink,
        known_names: Some(all_runtime_names),
    };
    let _ = visitor.visit_expr(expr);
}

/// Collect `@`-references (graph refs) from an expression into a raw-string set.
///
/// No filtering; every observed graph ref is recorded.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn collect_graph_ref_names(expr: &Expr, refs: &mut HashSet<String>) {
    let mut sink = StringNameSink { refs };
    let mut visitor = GraphRefVisitor {
        sink: &mut sink,
        known_names: None,
    };
    let _ = visitor.visit_expr(expr);
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
    let mut visitor = GraphRefVisitor {
        sink: &mut sink,
        known_names: None,
    };
    let _ = visitor.visit_expr(expr);
    sink.0
}
