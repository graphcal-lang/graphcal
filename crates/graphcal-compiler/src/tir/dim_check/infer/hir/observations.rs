//! The single observation sink of one body-checking pass.
//!
//! Inference records everything it learns about a body exactly once, while it
//! checks that body: each expression's typed node (assembled bottom-up into the
//! body's typed tree), the static index proofs an expression relies on, and the
//! nominal uses a checked root makes. Consumers (tree publication,
//! template-closure validation, override-dependency summaries) read these
//! observations instead of inferring the body again.

use crate::hir::expr::{ConstRef, Expr, ExprKind, FunctionRef, MatchPattern};
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use crate::source_id::SourceId;
use crate::tir::texpr::ExternSignature;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::expression_id::ExprId;
use crate::graphcal_error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::static_index::StaticIndexRequirement;
use crate::tir::texpr::{
    AssemblyError, ConstructorApplication, ConstructorMatch, NodeFacts, NominalObservation,
    PendingNodes,
};

use crate::semantic::applied_constructor::{AppliedConstructor, AppliedField};
use crate::semantic::checked_type::{CheckedType, Symbolic};

/// One executable use that observes a nominal type's concrete definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tir::dim_check) struct TypeDefinitionDependency {
    identity: ResolvedStructTypeName,
    span: Span,
}

impl TypeDefinitionDependency {
    pub(in crate::tir::dim_check) const fn identity(&self) -> &ResolvedStructTypeName {
        &self.identity
    }

    pub(in crate::tir::dim_check) const fn span(&self) -> Span {
        self.span
    }
}

/// One nominal use observed while checking a root, with the source span of a
/// use that inspects the nominal type's concrete definition.
#[derive(Debug, Clone)]
struct NominalUse {
    observation: NominalObservation,
    definition_span: Option<Span>,
}

impl NominalUse {
    fn type_definition_dependency(&self) -> Option<TypeDefinitionDependency> {
        let span = self.definition_span?;
        match &self.observation {
            NominalObservation::Field { identity, .. }
            | NominalObservation::Constructor { identity, .. } => Some(TypeDefinitionDependency {
                identity: identity.clone(),
                span,
            }),
            NominalObservation::TypeArgument(_)
            | NominalObservation::IndexLabel { .. }
            | NominalObservation::IndexArgument(_) => None,
        }
    }
}

/// Everything one checking pass observed about the bodies of one DAG.
///
/// Every expression is recorded at most once, after its children: a second
/// record for the same expression is an internal error, because each body is
/// inferred exactly once.
#[derive(Default)]
pub(in crate::tir::dim_check) struct BodyObservations {
    recorded: RefCell<HashSet<ExprId>>,
    typed: RefCell<PendingNodes>,
    nominal_uses: RefCell<HashMap<ExprId, Vec<NominalUse>>>,
    static_indexes: RefCell<HashMap<ExprId, Vec<StaticIndexRequirement>>>,
}

/// The result of one checking pass: the typed trees of the checked roots and
/// the nominal uses each root makes.
pub(in crate::tir::dim_check) struct FinishedObservations {
    pub(in crate::tir::dim_check) typed: PendingNodes,
    pub(in crate::tir::dim_check) nominal_uses: HashMap<ExprId, Arc<[NominalObservation]>>,
}

/// Why one checking pass's results cannot be published.
#[derive(Debug, thiserror::Error)]
pub(in crate::tir::dim_check) enum PublicationError {
    #[error(transparent)]
    TypedBodies(#[from] crate::tir::texpr::TypedBodiesError),
    #[error(transparent)]
    Discharge(#[from] crate::tir::texpr::DischargeError),
}

impl FinishedObservations {
    /// Publish the checked tree of each of `roots`, requiring that the trees
    /// cover exactly these roots.
    pub(in crate::tir::dim_check) fn publish(
        self,
        roots: &[&Expr],
        cardinality: &crate::tir::static_index::AxisCardinality<'_>,
    ) -> Result<crate::tir::texpr::CheckedBodies, PublicationError> {
        Ok(crate::tir::texpr::CheckedBodies::discharge(
            crate::tir::texpr::claim_roots(roots, self.typed)?,
            self.nominal_uses,
            cardinality,
        )?)
    }
}

impl BodyObservations {
    /// The typed trees of the checked roots and the nominal uses each makes.
    pub(in crate::tir::dim_check) fn finish(self) -> FinishedObservations {
        FinishedObservations {
            typed: self.typed.into_inner(),
            nominal_uses: self
                .nominal_uses
                .into_inner()
                .into_iter()
                .map(|(root, uses)| {
                    let uses = uses
                        .into_iter()
                        .map(|nominal_use| nominal_use.observation)
                        .collect::<Vec<_>>();
                    (root, uses.into())
                })
                .collect(),
        }
    }

    /// Record one nominal use made while checking `root`.
    pub(super) fn observe_nominal(
        &self,
        root: &ExprId,
        observation: NominalObservation,
        definition_span: Option<Span>,
    ) {
        self.nominal_uses
            .borrow_mut()
            .entry(root.clone())
            .or_default()
            .push(NominalUse {
                observation,
                definition_span,
            });
    }

    /// The uses of `root` that inspect a nominal type's concrete definition,
    /// in the order inference made them.
    pub(in crate::tir::dim_check) fn type_definition_dependencies(
        &self,
        root: &ExprId,
    ) -> Vec<TypeDefinitionDependency> {
        self.nominal_uses
            .borrow()
            .get(root)
            .into_iter()
            .flatten()
            .filter_map(NominalUse::type_definition_dependency)
            .collect()
    }

    /// Retain a static index proof `expr` relies on, attached when `expr` is recorded.
    pub(super) fn retain_static_index(&self, expr: &ExprId, requirement: StaticIndexRequirement) {
        self.static_indexes
            .borrow_mut()
            .entry(expr.clone())
            .or_default()
            .push(requirement);
    }

    /// Record one checked expression's typed node.
    fn insert(&self, expr: &Expr, node: CheckedNode, src: SourceId) -> Result<(), GraphcalError> {
        self.try_insert(expr, node).map_err(|error| {
            GraphcalError::internal_error(
                error.to_string(),
                src,
                DiagnosticAnchor::Source(expr.span),
            )
        })
    }

    fn try_insert(&self, expr: &Expr, node: CheckedNode) -> Result<(), AssemblyError> {
        let id = expr.id();
        if !self.recorded.borrow_mut().insert(id.clone()) {
            return Err(AssemblyError::CheckedTwice(id.clone()));
        }
        let static_indexes = self
            .static_indexes
            .borrow_mut()
            .remove(id)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        match node {
            CheckedNode::Contextual => self.typed.borrow_mut().record_contextual(expr),
            CheckedNode::Value {
                checked_type,
                constructor,
                constructor_matches,
                extern_signature,
            } => self.typed.borrow_mut().record_value(
                expr,
                checked_type,
                &NodeFacts {
                    constructor: constructor.as_deref(),
                    extern_signature: extern_signature.as_deref(),
                    constructor_matches: &constructor_matches,
                    static_indexes: &static_indexes,
                },
            ),
        }
    }

    /// Record a contextual literal accepted by the construct that consumes it.
    ///
    /// Contextual literals (plot strings, datetime and timezone literals) are
    /// never values of their own; the consuming rule records them where it
    /// accepts them.
    pub(in crate::tir::dim_check) fn record_contextual(
        &self,
        expr: &Expr,
        src: SourceId,
    ) -> Result<(), GraphcalError> {
        self.insert(expr, CheckedNode::Contextual, src)
    }

    pub(in crate::tir::dim_check) fn record(
        &self,
        expr: &Expr,
        inferred: &CheckedType<Symbolic>,
        dag: &crate::tir::typed::DagTIR,
        tir: &dyn crate::tir::typed::TirRead,
        src: SourceId,
    ) -> Result<(), GraphcalError> {
        let checked_type = inferred.clone();
        crate::tir::dim_check::expression_axes::check_materializable(
            &checked_type,
            tir,
            src,
            expr.span,
        )?;
        let resolved_constructor = |name| {
            tir.project_type_store()
                .lookup_constructor(name)
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("checked constructor `{name}` has no definition"),
                        src,
                        DiagnosticAnchor::Source(expr.span),
                    )
                })
        };
        let constructor = match expr.kind() {
            ExprKind::ConstructorCall { callee, .. } => Some(&callee.value),
            ExprKind::ConstRef(target) => match &target.value {
                ConstRef::Constructor(name) => Some(name),
                _ => None,
            },
            _ => None,
        }
        .map(|name| {
            let target = resolved_constructor(name)?;
            let CheckedType::Struct(_, args) = &checked_type else {
                return Err(GraphcalError::internal_error(
                    "constructor inferred a non-nominal type",
                    src,
                    DiagnosticAnchor::Source(expr.span),
                ));
            };
            // The field types checking the call resolved, instantiated at
            // the application's arguments, so a value built here never looks
            // its constructor up again.
            let fields = target
                .variant()
                .fields()
                .iter()
                .map(|field| {
                    crate::tir::dim_check::generic_substitution::resolved_field_type(
                        &crate::tir::dim_check::generic_substitution::resolved_type_field_key(
                            target.owning_type(),
                            target.variant(),
                            field.name(),
                        ),
                        target.definition(),
                        args,
                        dag,
                        src,
                        expr.span,
                    )
                    .map(|field_type| {
                        AppliedField::new(field.name().clone(), field_type.to_symbolic())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ConstructorApplication {
                constructor: target.clone(),
                applied: Arc::new(AppliedConstructor::new(
                    dag.frame().struct_type(target.owning_type()),
                    target.name(),
                    args.clone(),
                    fields,
                )),
            })
        })
        .transpose()?;
        let constructor_matches = match expr.kind() {
            ExprKind::Match { arms, .. } => arms
                .iter()
                .filter_map(|arm| match &arm.pattern {
                    MatchPattern::Constructor { constructor, .. } => Some(&constructor.value),
                    MatchPattern::IndexLabel { .. } => None,
                })
                .map(|name| {
                    let target = resolved_constructor(name)?;
                    Ok((
                        name.clone(),
                        ConstructorMatch {
                            definition: target.owning_type().clone(),
                            runtime_type: dag.frame().struct_type(target.owning_type()),
                            constructor: target.name(),
                        },
                    ))
                })
                .collect::<Result<_, GraphcalError>>()?,
            _ => HashMap::new(),
        };
        // The declared signature a plugin call was checked against, so its
        // typed node carries each argument's ABI kind and the result kind. A
        // call without a resolved signature was not checked, and assembly
        // rejects it.
        let extern_signature = match expr.kind() {
            ExprKind::FnCall {
                callee:
                    crate::syntax::span::Spanned {
                        value: FunctionRef::External(function),
                        ..
                    },
                ..
            } => tir
                .extern_functions()
                .get(&function.key())
                .map(|entry| Box::new(entry.signature.clone())),
            _ => None,
        };
        self.insert(
            expr,
            CheckedNode::Value {
                checked_type,
                constructor: constructor.map(Box::new),
                constructor_matches,
                extern_signature,
            },
            src,
        )
    }
}

/// What checking established about one expression, before it is recorded.
enum CheckedNode {
    Contextual,
    Value {
        checked_type: CheckedType<Symbolic>,
        constructor: Option<Box<ConstructorApplication<Symbolic>>>,
        constructor_matches: HashMap<ResolvedConstructorName, ConstructorMatch>,
        extern_signature: Option<Box<ExternSignature>>,
    },
}
