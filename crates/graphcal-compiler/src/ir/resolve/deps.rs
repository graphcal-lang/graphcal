use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::desugar::desugared_ast::{Expr, ExprKind, ParamBinding};
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{classify_special_fn, is_aggregation_fn, is_time_scale_name};
use crate::syntax::names::{DeclName, ScopedName};
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
    all_runtime_names: Option<&'a HashSet<&'a ScopedName>>,
    all_const_names: &'a HashSet<&'a ScopedName>,
    builtin_consts: &'a HashMap<&'a str, f64>,
    builtin_fns: &'a HashMap<&'a str, crate::registry::builtins::BuiltinFunction>,
    src: &'a NamedSource<Arc<String>>,
    graph_refs: &'a mut HashSet<ScopedName>,
    const_refs: &'a mut HashSet<ScopedName>,
}

impl RefCollector<'_> {
    fn handle_graph_ref(
        &mut self,
        ident: &crate::syntax::names::Spanned<ScopedName>,
    ) -> Result<(), GraphcalError> {
        let scoped = &ident.value;
        match self.kind {
            RefKind::ConstOnly => {
                // In const expressions, @name can reference other const nodes but not
                // runtime names. Runtime refs are already rejected by
                // check_no_runtime_graph_refs before we get here.
                if self.all_const_names.contains(scoped) {
                    self.const_refs.insert(scoped.clone());
                    Ok(())
                } else {
                    Err(GraphcalError::UnknownConstRef {
                        name: DeclName::new(scoped.to_string()),
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
                if runtime.contains(scoped) {
                    self.graph_refs.insert(scoped.clone());
                    Ok(())
                } else if self.all_const_names.contains(scoped) {
                    // @const_node_name in a node expression — track as const dependency.
                    self.const_refs.insert(scoped.clone());
                    Ok(())
                } else {
                    Err(GraphcalError::UnknownGraphRef {
                        name: DeclName::new(scoped.to_string()),
                        src: self.src.clone(),
                        span: ident.span.into(),
                    })
                }
            }
        }
    }

    fn handle_const_ref(
        &self,
        ident: &crate::syntax::names::Spanned<ScopedName>,
    ) -> Result<(), GraphcalError> {
        // Built-in constant names (`PI`, `E`, time scales) are bare; a
        // qualified `module.CONST` path never resolves to a built-in. The
        // bare-name check is the only string-level use of the AST identifier
        // here — the rest of the resolver carries the typed value.
        let lookup = ident.value.member();
        let is_bare = !ident.value.is_qualified();
        if is_bare && (self.builtin_consts.contains_key(lookup) || is_time_scale_name(lookup)) {
            Ok(())
        } else {
            Err(GraphcalError::UnknownConstRef {
                name: DeclName::new(ident.value.to_string()),
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

impl ExprVisitor<crate::syntax::phase::Desugared> for RefCollector<'_> {
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
        _arms: &[crate::desugar::desugared_ast::TupleMatchArm],
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
    all_const_names: &HashSet<&ScopedName>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<ScopedName>, GraphcalError> {
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
    all_runtime_names: &HashSet<&ScopedName>,
    all_const_names: &HashSet<&ScopedName>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
    self_name: Option<&str>,
) -> Result<(HashSet<ScopedName>, HashSet<ScopedName>), GraphcalError> {
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
    // Self-name is the bare local name of the owning declaration.
    if let Some(name) = self_name
        && matches!(expr.kind, ExprKind::Unfold { .. })
    {
        graph_refs.remove(&ScopedName::local(name));
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
/// qualified `module::member` form for qualified refs, bare for locals).
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

/// Collect `@`-references as typed [`ScopedName`]s, preserving module qualification.
pub fn collect_scoped_graph_refs(expr: &Expr, refs: &mut std::collections::BTreeSet<ScopedName>) {
    struct ScopedSink<'a> {
        refs: &'a mut std::collections::BTreeSet<ScopedName>,
    }
    impl GraphRefSink for ScopedSink<'_> {
        fn on_ref(&mut self, scoped: &ScopedName) {
            self.refs.insert(scoped.clone());
        }
    }
    let mut sink = ScopedSink { refs };
    let mut visitor = GraphRefVisitor {
        sink: &mut sink,
        known_names: None,
    };
    let _ = visitor.visit_expr(expr);
}
