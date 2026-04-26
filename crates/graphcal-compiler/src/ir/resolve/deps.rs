use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{classify_special_fn, is_aggregation_fn, is_time_scale_name};
use crate::syntax::ast::{Expr, ExprKind, ParamBinding};
use crate::syntax::names::DeclName;
use crate::syntax::visitor::ExprVisitor;
use miette::NamedSource;

/// What kind of reference collection to perform.
#[derive(Clone, Copy)]
enum RefKind {
    /// Collect only const refs; reject any @name that is not a known const.
    ConstOnly,
    /// Collect both graph and const refs; reject any @name that is neither.
    All,
}

/// Shared visitor that walks expressions collecting `@`-references and
/// validating function calls / const references.
///
/// Parameterized by `RefKind` to select behavior:
/// - `ConstOnly`: only accept `@name` that is a const node; populate `const_refs`.
///   Runtime refs are expected to have been rejected upstream.
/// - `All`: accept `@name` for runtime OR const nodes; populate `graph_refs` and
///   `const_refs` respectively.
struct RefCollector<'a> {
    kind: RefKind,
    all_runtime_names: Option<&'a HashSet<&'a str>>,
    all_const_names: &'a HashSet<&'a str>,
    builtin_consts: &'a HashMap<&'a str, f64>,
    builtin_fns: &'a HashMap<&'a str, crate::registry::builtins::BuiltinFunction>,
    src: &'a NamedSource<Arc<String>>,
    graph_refs: &'a mut HashSet<DeclName>,
    const_refs: &'a mut HashSet<DeclName>,
}

impl RefCollector<'_> {
    fn handle_graph_ref(
        &mut self,
        ident: &crate::syntax::names::Spanned<DeclName>,
    ) -> Result<(), GraphcalError> {
        match self.kind {
            RefKind::ConstOnly => {
                // In const expressions, @name can reference other const nodes but not
                // runtime names. Runtime refs are already rejected by
                // check_no_runtime_graph_refs before we get here.
                if self.all_const_names.contains(ident.value.as_str()) {
                    self.const_refs.insert(DeclName::new(ident.value.as_str()));
                    Ok(())
                } else {
                    Err(GraphcalError::UnknownConstRef {
                        name: ident.value.clone(),
                        src: self.src.clone(),
                        span: ident.span.into(),
                    })
                }
            }
            RefKind::All => {
                // Safety: the public `extract_all_refs` entry point always passes
                // `Some(all_runtime_names)` for `RefKind::All`, so this unwrap is
                // an internal invariant.
                let runtime = self.all_runtime_names.unwrap_or(self.all_const_names);
                if runtime.contains(ident.value.as_str()) {
                    self.graph_refs.insert(DeclName::new(ident.value.as_str()));
                    Ok(())
                } else if self.all_const_names.contains(ident.value.as_str()) {
                    // @const_node_name in a node expression — track as const dependency.
                    self.const_refs.insert(DeclName::new(ident.value.as_str()));
                    Ok(())
                } else {
                    Err(GraphcalError::UnknownGraphRef {
                        name: ident.value.clone(),
                        src: self.src.clone(),
                        span: ident.span.into(),
                    })
                }
            }
        }
    }

    fn handle_const_ref(
        &self,
        ident: &crate::syntax::names::Spanned<DeclName>,
    ) -> Result<(), GraphcalError> {
        // Bare UPPER_SNAKE_CASE identifiers: built-in constants only.
        if self.builtin_consts.contains_key(ident.value.as_str())
            || is_time_scale_name(ident.value.as_str())
        {
            Ok(())
        } else {
            Err(GraphcalError::UnknownConstRef {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: ident.span.into(),
            })
        }
    }

    fn handle_fn_call(
        &self,
        name: &crate::syntax::names::Spanned<crate::syntax::names::FnName>,
        args: &[Expr],
    ) -> Result<(), GraphcalError> {
        let name_str = name.value.as_str();
        if !self.builtin_fns.contains_key(name_str) && classify_special_fn(name_str).is_none() {
            return Err(GraphcalError::UnknownFunction {
                name: name.value.clone(),
                src: self.src.clone(),
                span: name.span.into(),
            });
        }
        // Only check arity for builtins (user fn arity checked later in dim_check).
        // Skip arity check for aggregation/conversion functions.
        if let Some(builtin) = self.builtin_fns.get(name_str)
            && args.len() != builtin.arity()
            && !is_aggregation_fn(name_str)
        {
            return Err(GraphcalError::WrongArity {
                name: name.value.clone(),
                expected: builtin.arity(),
                got: args.len(),
                src: self.src.clone(),
                span: name.span.into(),
            });
        }
        Ok(())
    }
}

impl ExprVisitor for RefCollector<'_> {
    type Error = GraphcalError;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            self.handle_graph_ref(ident)?;
        }
        Ok(())
    }

    fn visit_const_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::ConstRef(ident) = &expr.kind {
            self.handle_const_ref(ident)?;
        }
        Ok(())
    }

    fn visit_qualified_const_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedConstRef { name: ident, .. } = &expr.kind {
            self.handle_const_ref(ident)?;
        }
        Ok(())
    }

    fn visit_fn_call(&mut self, expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { name, .. } = &expr.kind {
            self.handle_fn_call(name, args)?;
        }
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }

    fn visit_inline_dag_ref(
        &mut self,
        _expr: &Expr,
        args: &[ParamBinding],
    ) -> Result<(), Self::Error> {
        for binding in args {
            self.visit_expr(&binding.value)?;
        }
        Ok(())
    }

    fn visit_tuple_match(
        &mut self,
        _expr: &Expr,
        _scrutinees: &[Expr],
        _arms: &[crate::syntax::ast::TupleMatchArm],
    ) -> Result<(), Self::Error> {
        // TupleMatch is desugared before resolution.
        #[expect(clippy::unreachable, reason = "invariant: desugared before resolution")]
        {
            unreachable!("TupleMatch should be desugared before resolution")
        }
    }
}

/// Extract const references from a const expression.
pub(super) fn extract_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<DeclName>, GraphcalError> {
    let mut deps = HashSet::new();
    let mut unused = HashSet::new();
    let mut collector = RefCollector {
        kind: RefKind::ConstOnly,
        all_runtime_names: None,
        all_const_names,
        builtin_consts,
        builtin_fns,
        src,
        graph_refs: &mut unused,
        const_refs: &mut deps,
    };
    collector.visit_expr(expr)?;
    Ok(deps)
}

/// Extract all graph and const references from an expression.
///
/// When `self_name` is `Some` and the expression is an `Unfold`, the self-reference
/// is excluded from the returned `graph_refs`. Unfold self-references (e.g.
/// `@my_node[prev]`) are temporal — they access the previous iteration, not a
/// true cyclic dependency.
pub(super) fn extract_all_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
    self_name: Option<&str>,
) -> Result<(HashSet<DeclName>, HashSet<DeclName>), GraphcalError> {
    let mut graph_refs = HashSet::new();
    let mut const_refs = HashSet::new();
    {
        let mut collector = RefCollector {
            kind: RefKind::All,
            all_runtime_names: Some(all_runtime_names),
            all_const_names,
            builtin_consts,
            builtin_fns,
            src,
            graph_refs: &mut graph_refs,
            const_refs: &mut const_refs,
        };
        collector.visit_expr(expr)?;
    }
    // Unfold self-references (@self[prev_i]) are not true cyclic dependencies —
    // they access the previous step. Remove the self-edge so the DAG stays acyclic.
    if let Some(name) = self_name
        && matches!(expr.kind, ExprKind::Unfold { .. })
    {
        graph_refs.remove(name);
    }
    Ok((graph_refs, const_refs))
}

/// Sink for graph-reference observations during an AST walk.
///
/// This trait is the single extension point for the four historical collectors
/// (`KnownGraphRefCollector`, `GraphRefCollector`, `GraphRefNameCollector`,
/// `GraphRefDetector`). Implementors record the observation however they
/// like — as a typed `ScopedName`, a raw `String`, or a `bool` flag.
trait GraphRefSink {
    /// Called for each `@name` reference. `name` is the bare identifier.
    /// (Qualified `@module.name` is no longer accepted by the parser.)
    fn on_local(&mut self, name: &str);
}

/// Generic visitor that routes every observed graph reference through a
/// [`GraphRefSink`]. Optionally filters to names in `known_names`.
struct GraphRefVisitor<'a, S: GraphRefSink> {
    sink: &'a mut S,
    known_names: Option<&'a HashSet<&'a str>>,
}

impl<S: GraphRefSink> GraphRefVisitor<'_, S> {
    fn should_record(&self, name: &str) -> bool {
        self.known_names.is_none_or(|set| set.contains(name))
    }
}

impl<S: GraphRefSink> ExprVisitor for GraphRefVisitor<'_, S> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            let name = ident.value.as_str();
            if self.should_record(name) {
                self.sink.on_local(name);
            }
        }
        Ok(())
    }

}

/// Sink that stores every observed ref name as a `String` (qualified refs drop the module).
struct StringNameSink<'a> {
    refs: &'a mut HashSet<String>,
}

impl GraphRefSink for StringNameSink<'_> {
    fn on_local(&mut self, name: &str) {
        self.refs.insert(name.to_string());
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
        fn on_local(&mut self, _name: &str) {
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

/// Collect `@`-references as typed [`ScopedName`]s, preserving module qualification.
pub fn collect_scoped_graph_refs(
    expr: &Expr,
    refs: &mut std::collections::BTreeSet<crate::registry::resolve_types::ScopedName>,
) {
    use crate::registry::resolve_types::ScopedName;
    struct ScopedSink<'a> {
        refs: &'a mut std::collections::BTreeSet<ScopedName>,
    }
    impl GraphRefSink for ScopedSink<'_> {
        fn on_local(&mut self, name: &str) {
            self.refs.insert(ScopedName::local(name));
        }
    }
    let mut sink = ScopedSink { refs };
    let mut visitor = GraphRefVisitor {
        sink: &mut sink,
        known_names: None,
    };
    let _ = visitor.visit_expr(expr);
}
