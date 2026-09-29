//! Specialize checked typed trees without re-inferring their source bodies.
//!
//! A semantic instance's trees are its template's, with the instance's Static
//! substitution applied to every type they carry; a generic field bound's
//! tree is specialized with one application's `Nat` arguments. Both rewrite
//! only types ([`TypeMap`]) and then classify the result.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::expression_id::ExprId;
use crate::hir::expr::Expr;
use crate::registry::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::expression_facts::{ConstructorApplication, ConstructorMatch};
use crate::tir::texpr::map::{SymbolicView, TypeMap};
use crate::tir::texpr::{CheckedBodies, CheckedBody, StaticPosition, TBody};
use crate::tir::typed::model::{DagTIR, TirRead};
use crate::tir::typed::specialization::{specialize_expression_type, specialize_index_ref};

use super::expression_axes::{check_materializable, checked_index_cardinality};

/// The bindings a specialization applies.
pub(super) enum BodySubstitution<'a> {
    /// A semantic instance's Static substitution.
    Static(&'a crate::ir::static_substitution::StaticSubstitution),
    /// One generic application's `Nat` arguments.
    Generic(&'a crate::tir::typed::Substitution),
}

impl BodySubstitution<'_> {
    pub(super) fn value_type(
        &self,
        ty: &CheckedType<Symbolic>,
        tir: &dyn TirRead,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        match self {
            Self::Static(substitution) => specialize_expression_type(ty, substitution, tir, src),
            Self::Generic(substitution) => substitution
                .instantiate(ty, span)
                .map(|ty| ty.to_symbolic())
                .map_err(|error| error.into_graphcal(src)),
        }
    }

    pub(super) fn index(
        &self,
        index: &IndexTypeRef<Symbolic>,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<IndexTypeRef<Symbolic>, GraphcalError> {
        match self {
            Self::Static(substitution) => Ok(specialize_index_ref(index, substitution)),
            Self::Generic(substitution) => substitution
                .instantiate_index(index, span)
                .map(|index| index.to_symbolic())
                .map_err(|error| error.into_graphcal(src)),
        }
    }
}

/// Where a specialization reports a failing node.
#[derive(Clone, Copy)]
enum ReportAt {
    /// At the failing node itself.
    Node,
    /// At the specialized root, whatever node fails.
    Root(Span),
}

/// Rewrites a tree's types into `dag`'s environment under `substitution`.
struct Specializer<'a> {
    substitution: BodySubstitution<'a>,
    dag: &'a DagTIR,
    tir: &'a dyn TirRead,
    src: &'a NamedSource<Arc<String>>,
    report_at: ReportAt,
}

/// A specialization invariant the checker established was violated.
fn internal(
    src: &NamedSource<Arc<String>>,
    message: impl Into<String>,
    anchor: DiagnosticAnchor,
) -> GraphcalError {
    GraphcalError::internal_error(message, src, anchor)
}

impl Specializer<'_> {
    const fn span(&self, node: Span) -> Span {
        match self.report_at {
            ReportAt::Node => node,
            ReportAt::Root(root) => root,
        }
    }
}

impl<V: SymbolicView> TypeMap<V, Symbolic> for Specializer<'_> {
    type Error = GraphcalError;

    fn node_type(
        &mut self,
        ty: &CheckedType<V>,
        span: Span,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        let span = self.span(span);
        let ty = self
            .substitution
            .value_type(&V::symbolic_type(ty), self.tir, self.src, span)?;
        check_materializable(&ty, self.tir, self.src, span)?;
        Ok(ty)
    }

    fn application(
        &mut self,
        application: &ConstructorApplication<V>,
        ty: &CheckedType<Symbolic>,
        span: Span,
    ) -> Result<ConstructorApplication<Symbolic>, GraphcalError> {
        let CheckedType::Struct(_, args) = ty else {
            return Err(internal(
                self.src,
                "constructor specialization has no nominal result",
                DiagnosticAnchor::Source(self.span(span)),
            ));
        };
        Ok(ConstructorApplication {
            runtime_type: self.dag.frame().struct_type(application.definition()),
            constructor: application.constructor.clone(),
            generic_args: args.clone(),
        })
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<V>,
        span: Span,
    ) -> Result<StaticPosition<Symbolic>, GraphcalError> {
        Ok(StaticPosition {
            axis: self.substitution.index(
                &V::symbolic_index(&position.axis),
                self.src,
                self.span(span),
            )?,
            position: position.position,
            usage: position.usage,
        })
    }

    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch {
        ConstructorMatch {
            definition: target.definition.clone(),
            runtime_type: self.dag.frame().struct_type(&target.definition),
            constructor: target.constructor.clone(),
        }
    }
}

impl Specializer<'_> {
    fn body(&mut self, body: &CheckedBody) -> Result<TBody<Symbolic>, GraphcalError> {
        match body {
            CheckedBody::Executable(body) => body.map_types(self),
            CheckedBody::Deferred(body) => body.map_types(self),
        }
    }
}

/// The checked trees of one semantic instance: each root inferred
/// independently by the instance (a rebound parameter default) keeps its own
/// tree; every other root is its template's tree (or, when the instance binds
/// a defaulted dimension port, the tree checked where that port is rigid),
/// specialized with the instance's Static substitution.
pub(super) fn instance_bodies(
    dag: &DagTIR,
    tir: &dyn TirRead,
    mut independent: HashMap<ExprId, TBody<Symbolic>>,
    template: &CheckedBodies,
    port_generic: &HashMap<ExprId, TBody<Symbolic>>,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedBodies, GraphcalError> {
    let mut specializer = Specializer {
        substitution: BodySubstitution::Static(substitution),
        dag,
        tir,
        src,
        report_at: ReportAt::Node,
    };
    let mut seen = std::collections::HashSet::new();
    let mut roots = Vec::new();
    for root in dag.owned_expression_roots() {
        let id = root.id();
        if !seen.insert(id) {
            continue;
        }
        let body = if let Some(body) = independent.remove(id) {
            body
        } else if let Some(body) = port_generic.get(id) {
            body.map_types(&mut specializer)?
        } else {
            let body = template.get(id).ok_or_else(|| {
                internal(
                    src,
                    format!("missing checked expression: {id:?}"),
                    DiagnosticAnchor::Source(root.span),
                )
            })?;
            specializer.body(body)?
        };
        roots.push((id.clone(), body));
    }
    CheckedBodies::discharge(roots, &|index| checked_index_cardinality(tir, index))
        .map_err(|error| internal(src, error.to_string(), DiagnosticAnchor::WholeFile))
}

/// The executable tree of one generic field bound under one application's
/// `Nat` arguments, specialized from the tree checked in `dag`, its owner.
///
/// # Errors
///
/// Returns an evaluation error when a static position falls outside its
/// now-bound axis, and an internal error when the tree is unknown or still
/// awaits bindings after specialization.
pub(super) fn specialize_bound_body(
    tir: &dyn TirRead,
    dag: &DagTIR,
    bodies: &CheckedBodies,
    root: &Expr,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::texpr::TExpr, GraphcalError> {
    let substitution = crate::tir::typed::Substitution::for_nats(bindings);
    let mut specializer = Specializer {
        substitution: BodySubstitution::Generic(&substitution),
        dag,
        tir,
        src,
        report_at: ReportAt::Root(root.span),
    };
    let diagnostic = |message: String| internal(src, message, DiagnosticAnchor::Source(root.span));
    let body = bodies
        .get(root.id())
        .ok_or_else(|| diagnostic(format!("missing checked expression: {:?}", root.id())))?;
    let tree = specializer.body(body)?;
    let checked = CheckedBody::discharge(tree, &|index| checked_index_cardinality(tir, index))
        .map_err(|error| match error {
            crate::tir::texpr::DischargeError::StaticIndex(error) => GraphcalError::EvalError {
                message: error.to_string(),
                src: src.clone(),
                span: root.span.into(),
            },
            error @ crate::tir::texpr::DischargeError::UnavailableIndex(_) => {
                diagnostic(error.to_string())
            }
        })?;
    match checked {
        CheckedBody::Executable(TBody::Value(expr)) => Ok(*expr),
        CheckedBody::Executable(TBody::Contextual(_)) => Err(diagnostic(
            crate::tir::texpr::ExecutableBodyError::Contextual(root.id().clone()).to_string(),
        )),
        CheckedBody::Deferred(_) => Err(diagnostic(
            crate::tir::texpr::ExecutableBodyError::Deferred(root.id().clone()).to_string(),
        )),
    }
}
