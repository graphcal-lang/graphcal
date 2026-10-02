//! Specialize checked typed trees without re-inferring their source bodies.
//!
//! A semantic instance's trees are its template's, with the instance's Static
//! substitution applied to every type they carry; a generic field bound's
//! tree is specialized with one application's `Nat` arguments. Both rewrite
//! only types (see `TypeMap`) and then classify the result.

use crate::semantic_error::index::IndexError;
use std::collections::HashMap;
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::expression_id::ExprId;
use crate::hir::expr::Expr;
use crate::semantic::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::semantic::index_axis::IndexAxis;
use crate::semantic::key_value::KeyValue;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;
use crate::tir::texpr::map::{KeyEntry, SymbolicView, TypeMap};
use crate::tir::texpr::{
    CallSlot, CallTargets, CheckedBodies, CheckedBody, ClaimedRoots, ConstructorApplication,
    ConstructorMatch, NominalObservation, StaticPosition, TBody,
};
use crate::tir::typed::model::DagTIR;
use crate::tir::typed::program::TirRead;
use crate::tir::typed::specialization::{specialize_expression_type, specialize_index_ref};

use super::expression_axes::{check_materializable, concrete_index_kind};

/// The bindings a specialization applies.
enum BodySubstitution<'a> {
    /// A semantic instance's Static substitution.
    Static(&'a crate::ir::static_substitution::StaticSubstitution),
    /// One generic application's `Nat` arguments.
    Generic(&'a crate::tir::typed::Substitution),
}

impl BodySubstitution<'_> {
    fn value_type(
        &self,
        ty: &CheckedType<Symbolic>,
        tir: &dyn TirRead,
        src: SourceId,
        span: Span,
    ) -> Result<CheckedType<Symbolic>, SemanticError> {
        match self {
            Self::Static(substitution) => specialize_expression_type(ty, substitution, tir, src),
            Self::Generic(substitution) => substitution
                .instantiate(ty, span)
                .map(|ty| ty.to_symbolic())
                .map_err(|error| error.into_graphcal(src)),
        }
    }

    fn index(
        &self,
        index: &IndexTypeRef<Symbolic>,
        src: SourceId,
        span: Span,
    ) -> Result<IndexTypeRef<Symbolic>, SemanticError> {
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
    src: SourceId,
    report_at: ReportAt,
}

/// A specialization invariant the checker established was violated.
fn internal(src: SourceId, message: impl Into<String>, anchor: DiagnosticAnchor) -> SemanticError {
    SemanticError::internal_error(message, src, anchor)
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
    type Error = SemanticError;

    fn node_type(
        &mut self,
        ty: &CheckedType<V>,
        span: Span,
    ) -> Result<CheckedType<Symbolic>, SemanticError> {
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
    ) -> Result<ConstructorApplication<Symbolic>, SemanticError> {
        let CheckedType::Struct(_, args) = ty else {
            return Err(internal(
                self.src,
                "constructor specialization has no nominal result",
                DiagnosticAnchor::Source(self.span(span)),
            ));
        };
        let report = self.span(span);
        let applied = application.applied.try_map_types(
            self.dag.frame().struct_type(application.definition()),
            args.clone(),
            |field_type| {
                self.substitution.value_type(
                    &V::symbolic_type(field_type),
                    self.tir,
                    self.src,
                    report,
                )
            },
        )?;
        Ok(ConstructorApplication {
            constructor: application.constructor.clone(),
            applied: std::sync::Arc::new(applied),
        })
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<V>,
        span: Span,
    ) -> Result<StaticPosition<Symbolic>, SemanticError> {
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

    /// A specialized tree is symbolic again: its axes are given when it is
    /// discharged.
    fn axis(
        &mut self,
        _carried: &V::Discharged<IndexAxis>,
        _index: Option<&IndexTypeRef<Symbolic>>,
        _span: Span,
    ) -> Result<(), SemanticError> {
        Ok(())
    }

    fn map_layout(
        &mut self,
        _carried: &V::Discharged<crate::tir::texpr::MapLayout>,
        _indexes: &[Option<&IndexTypeRef<Symbolic>>],
        _entries: &[&crate::syntax::non_empty::NonEmpty<crate::hir::expr::MapEntryKey>],
        _span: Span,
    ) -> Result<(), SemanticError> {
        Ok(())
    }

    fn key(
        &mut self,
        _carried: &V::Discharged<KeyValue>,
        _index: Option<&IndexTypeRef<Symbolic>>,
        _entry: KeyEntry<'_>,
        _span: Span,
    ) -> Result<(), SemanticError> {
        Ok(())
    }

    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch {
        ConstructorMatch {
            definition: target.definition.clone(),
            runtime_type: self.dag.frame().struct_type(&target.definition),
            constructor: target.constructor.clone(),
        }
    }

    /// Specialization keeps a tree in the body whose call targets number it.
    fn call_slot(&mut self, slot: CallSlot) -> CallSlot {
        slot
    }
}

impl Specializer<'_> {
    fn body(&mut self, body: &CheckedBody) -> Result<TBody<Symbolic>, SemanticError> {
        match body {
            CheckedBody::Executable(body) => body.map_types(self),
            CheckedBody::Deferred(body) => body.map_types(self),
        }
    }
}

/// Symbolic trees checked outside a body's canonical check, by root, with the
/// nominal uses checking observed in each and the call targets their call
/// nodes are numbered by.
#[derive(Default)]
pub(super) struct DerivedTrees {
    pub(super) bodies: HashMap<ExprId, TBody<Symbolic>>,
    pub(super) nominal_uses: HashMap<ExprId, Arc<[NominalObservation]>>,
    pub(super) calls: CallTargets,
}

/// The checked trees of one semantic instance: each root inferred
/// independently by the instance (a rebound parameter default) keeps its own
/// tree; every other root is its template's tree (or, when the instance binds
/// a defaulted dimension port, the tree checked where that port is rigid),
/// specialized with the instance's Static substitution. Each root keeps the
/// nominal uses of the tree it came from.
///
/// The instance's call targets extend its template's, so every tree taken
/// from the template keeps its call slots and the instance calls the same
/// DAGs through them; a tree checked outside the template is renumbered into
/// the extension.
pub(super) fn specialize_instance_bodies(
    dag: &DagTIR,
    tir: &dyn TirRead,
    mut independent: DerivedTrees,
    template: &CheckedBodies,
    port_generic: &DerivedTrees,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
    src: SourceId,
) -> Result<CheckedBodies, SemanticError> {
    let mut specializer = Specializer {
        substitution: BodySubstitution::Static(substitution),
        dag,
        tir,
        src,
        report_at: ReportAt::Node,
    };
    let mut seen = std::collections::HashSet::new();
    let mut roots = Vec::new();
    let mut nominal_uses = HashMap::new();
    let mut calls = template.calls().clone();
    for root in dag.owned_expression_roots() {
        let id = root.id();
        if !seen.insert(id) {
            continue;
        }
        let (body, uses) = if let Some(body) = independent.bodies.remove(id) {
            (
                body.rehome_calls(&independent.calls, &mut calls),
                independent.nominal_uses.remove(id),
            )
        } else if let Some(body) = port_generic.bodies.get(id) {
            (
                body.rehome_calls(&port_generic.calls, &mut calls)
                    .map_types(&mut specializer)?,
                port_generic.nominal_uses.get(id).cloned(),
            )
        } else {
            let body = template.get(id).ok_or_else(|| {
                internal(
                    src,
                    format!("missing checked expression: {id:?}"),
                    DiagnosticAnchor::Source(root.span),
                )
            })?;
            (specializer.body(body)?, template.shared_nominal_uses(id))
        };
        if let Some(uses) = uses {
            nominal_uses.insert(id.clone(), uses);
        }
        roots.push((id.clone(), body));
    }
    CheckedBodies::discharge(ClaimedRoots { roots, calls }, nominal_uses, &|index| {
        concrete_index_kind(tir, index)
    })
    .map_err(|error| internal(src, error.to_string(), DiagnosticAnchor::WholeFile))
}

/// The executable tree of one domain bound under one application's `Nat`
/// arguments, specialized from the tree checked in the scope of the bound's
/// owner, together with that scope.
///
/// This discharges the bound's retained `Nat`, type, and shape obligations in
/// its canonical environment; it does not infer the source body.
///
/// # Errors
///
/// Returns an evaluation error when a static position falls outside its
/// now-bound axis, and an internal error when the tree is unknown or still
/// awaits bindings after specialization.
#[expect(
    clippy::implicit_hasher,
    reason = "canonical binding services retain this exact map type"
)]
pub fn specialize_bound_expression<'t>(
    tir: &crate::tir::typed::CheckedTir,
    bound: crate::tir::typed::body_scope::Scoped<'t, crate::tir::typed::ResolvedDomainBound>,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
) -> Result<crate::tir::typed::ScopedTree<'t, crate::tir::texpr::TExpr>, SemanticError> {
    let scope = bound.scope();
    let dag = scope.dag();
    let bound = bound.get();
    specialize_bound_body(
        tir,
        dag.body(),
        dag.bodies(),
        &bound.value,
        bindings,
        bound.src,
    )
    .map(|tree| crate::tir::typed::ScopedTree::new(scope, tree))
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
    src: SourceId,
) -> Result<crate::tir::texpr::TExpr, SemanticError> {
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
    let checked = CheckedBody::discharge(tree, &|index| concrete_index_kind(tir, index)).map_err(
        |error| match error {
            crate::tir::texpr::DischargeError::StaticIndex(error) => {
                SemanticError::located(src, root.span, IndexError::StaticIndexOutOfBounds { error })
            }
            error @ (crate::tir::texpr::DischargeError::UnavailableIndex(_)
            | crate::tir::texpr::DischargeError::AxisShape(_)
            | crate::tir::texpr::DischargeError::NonFiniteAxis(_)
            | crate::tir::texpr::DischargeError::KeyOutsideAxis { .. }
            | crate::tir::texpr::DischargeError::MapLayout { .. }) => diagnostic(error.to_string()),
        },
    )?;
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
