//! Scoped traversal of executable trees.
//!
//! An evaluator walks a checked tree only as [`ScopedNode`]s: every child of
//! a node comes out in the node's own scope, and every body handle a node
//! carries — a declaration reference, a constant reference, a unit term — is
//! handed out already resolved in that scope. There is no way to resolve a
//! handle in a scope chosen separately from the tree that holds it, so a
//! shared template body always reads the declarations of the instance that
//! runs it.

use std::collections::BTreeSet;

use crate::builtin::AggregationFn;
use crate::dag_id::DagId;
use crate::expression_id::ExprId;
use crate::hir::expr::{
    ExternFnRef, ForBinding, ForBindingIndex, IndexVariantRef, LocalDef, LocalId, LocalUnit,
    ResolvedUnitExpr, ResolvedUnitExprItem, ResolvedUnitRef, UnfoldRecurrence,
};
use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};
use crate::semantic::checked_type::CheckedType;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::FieldName;
use crate::tir::texpr::operators::{BExpr, CExpr, DExpr, IExpr, LinearAlgebraCall, QExpr};
use crate::tir::texpr::{
    CallSlot, ConstructorApplication, DatetimeLiteral, StaticPosition, TConstRef, TConstructorArm,
    TExpr, TExprKind, TExternArg, TFieldInit, TIndexArg, TKeyForm, TLabelArm, TMapEntry,
    TMatchArms, TNodeRef, TParamBinding, visit_tnodes,
};

use super::body_scope::Scoped;
use super::dag_position::DagPosition;

/// One node of an executable tree, in the scope of the DAG that runs it.
pub type ScopedNode<'t> = Scoped<'t, TExpr>;

/// The form of one [`ScopedNode`]: its children in the node's scope and its
/// body handles resolved in that scope.
#[derive(Debug, Clone)]
pub enum NodeKind<'t> {
    QuantityLiteral {
        value: f64,
        unit: ScopedUnitExpr<'t>,
    },
    Quantity(QExpr<ScopedNode<'t>>),
    Int(IExpr<ScopedNode<'t>>),
    Bool(BExpr<ScopedNode<'t>>),
    Complex(CExpr<ScopedNode<'t>>),
    Datetime(DExpr<ScopedNode<'t>>),
    KeyShift {
        key: ScopedNode<'t>,
        addend: ScopedNode<'t>,
    },
    /// `@name`: the declaration it denotes in the node's scope.
    GraphRef(Spanned<ResolvedDeclName>),
    Const(Spanned<ConstRef<'t>>),
    Local(&'t Spanned<LocalId>),
    DatetimeLiteral(&'t DatetimeLiteral),
    Aggregate {
        function: AggregationFn,
        arg: ScopedNode<'t>,
    },
    LinearAlgebra(LinearAlgebraCall<ScopedNode<'t>>),
    Extern {
        function: &'t ExternFnRef,
        args: Scoped<'t, [TExternArg]>,
    },
    If {
        condition: ScopedNode<'t>,
        then_branch: ScopedNode<'t>,
        else_branch: ScopedNode<'t>,
    },
    Convert {
        expr: ScopedNode<'t>,
        target: ScopedUnitExpr<'t>,
    },
    DisplayTimezone {
        expr: ScopedNode<'t>,
        timezone: &'t IanaTimeZoneId,
    },
    Field {
        expr: ScopedNode<'t>,
        field: &'t Spanned<FieldName>,
    },
    Construct {
        application: &'t ConstructorApplication,
        fields: Scoped<'t, [TFieldInit]>,
    },
    Map {
        entries: Scoped<'t, [TMapEntry]>,
    },
    For {
        bindings: &'t [ForBinding],
        body: ScopedNode<'t>,
    },
    Index {
        expr: ScopedNode<'t>,
        args: Scoped<'t, [TIndexArg]>,
    },
    Scan {
        source: ScopedNode<'t>,
        init: ScopedNode<'t>,
        acc: &'t LocalDef,
        val: &'t LocalDef,
        body: ScopedNode<'t>,
    },
    Unfold {
        recurrence: &'t UnfoldRecurrence,
        init: ScopedNode<'t>,
        body: ScopedNode<'t>,
    },
    Key {
        form: &'t TKeyForm,
        axis: &'t ForBindingIndex,
        arg: ScopedNode<'t>,
    },
    Match {
        scrutinee: ScopedNode<'t>,
        arms: ScopedMatchArms<'t>,
    },
    Variant(&'t IndexVariantRef),
    DagCall {
        /// The call target, named by its slot in the scope's body.
        call: ScopedCall<'t>,
        args: Scoped<'t, [TParamBinding]>,
        output: &'t Spanned<ResolvedDeclName>,
    },
}

/// A constant-like reference, resolved in its node's scope.
#[derive(Debug, Clone)]
pub enum ConstRef<'t> {
    Decl(ResolvedDeclName),
    Constructor(&'t ConstructorApplication),
}

/// A unit expression of a node, whose terms resolve in the node's scope.
pub type ScopedUnitExpr<'t> = Scoped<'t, ResolvedUnitExpr>;

/// The slot of a call node, together with the scope whose body numbers it:
/// only that scope gives the slot a target.
pub type ScopedCall<'t> = Scoped<'t, CallSlot>;

impl<'t> ScopedCall<'t> {
    /// The DAG this call targets.
    #[must_use]
    pub fn target(self) -> &'t DagId {
        self.scope().dag().call_targets().target(*self.get())
    }

    /// The position of the DAG this call targets in the registry of the
    /// program running it, which the registry resolved for the caller's
    /// body when the program was checked.
    #[must_use]
    pub const fn callee(self) -> DagPosition {
        self.scope().callee(*self.get())
    }
}

/// The arms of a match node, in the node's scope.
#[derive(Debug, Clone, Copy)]
pub enum ScopedMatchArms<'t> {
    Labels(Scoped<'t, [TLabelArm]>),
    Constructors(Scoped<'t, [TConstructorArm]>),
}

/// An index-access argument of a node, in the node's scope.
#[derive(Debug, Clone, Copy)]
pub enum ScopedIndexArg<'t> {
    Variant(&'t IndexVariantRef),
    Var(&'t Spanned<LocalId>),
    /// A key of the indexed axis.
    Key(ScopedNode<'t>),
    /// A static position proved in range of the indexed `Fin` axis.
    Position(&'t StaticPosition),
}

impl<'t> Scoped<'t, TExpr> {
    /// The HIR occurrence this node checks.
    #[must_use]
    pub const fn id(self) -> &'t ExprId {
        self.get().id()
    }

    #[must_use]
    pub const fn span(self) -> Span {
        self.get().span()
    }

    /// The checked type of the value this node evaluates to.
    #[must_use]
    pub const fn ty(self) -> &'t CheckedType {
        self.get().ty()
    }

    /// This node's form, with its children in this node's scope and its
    /// handles resolved in that scope.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive scoped view of every typed expression kind"
    )]
    pub fn kind(self) -> NodeKind<'t> {
        let scope = self.scope();
        let node = |child: &'t TExpr| Scoped::new(scope, child);
        let operand = |child: &'t Box<_>| Ok::<_, std::convert::Infallible>(node(child));
        let resolve = |handle| scope.resolve(handle);
        match self.get().kind() {
            TExprKind::QuantityLiteral { value, unit } => NodeKind::QuantityLiteral {
                value: *value,
                unit: Scoped::new(scope, unit),
            },
            TExprKind::Quantity(operation) => {
                let Ok(operation) = operation.try_map(operand);
                NodeKind::Quantity(operation)
            }
            TExprKind::Int(operation) => {
                let Ok(operation) = operation.try_map(operand);
                NodeKind::Int(operation)
            }
            TExprKind::Bool(operation) => {
                let Ok(operation) = operation.try_map(operand);
                NodeKind::Bool(operation)
            }
            TExprKind::Complex(operation) => {
                let Ok(operation) = operation.try_map(operand);
                NodeKind::Complex(operation)
            }
            TExprKind::Datetime(operation) => {
                let Ok(operation) = operation.try_map(operand);
                NodeKind::Datetime(operation)
            }
            TExprKind::KeyShift { key, addend } => NodeKind::KeyShift {
                key: node(key),
                addend: node(addend),
            },
            TExprKind::GraphRef(target) => {
                NodeKind::GraphRef(Spanned::new(resolve(&target.value), target.span))
            }
            TExprKind::Const(target) => NodeKind::Const(Spanned::new(
                match &target.value {
                    TConstRef::Decl(handle) => ConstRef::Decl(resolve(handle)),
                    TConstRef::Constructor(application) => ConstRef::Constructor(application),
                },
                target.span,
            )),
            TExprKind::Local(local) => NodeKind::Local(local),
            TExprKind::DatetimeLiteral(literal) => NodeKind::DatetimeLiteral(literal),
            TExprKind::Aggregate { function, arg } => NodeKind::Aggregate {
                function: *function,
                arg: node(arg),
            },
            TExprKind::LinearAlgebra(call) => {
                let Ok(call) = call.try_map(operand);
                NodeKind::LinearAlgebra(call)
            }
            TExprKind::Extern { function, args } => NodeKind::Extern {
                function,
                args: Scoped::new(scope, args.as_slice()),
            },
            TExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => NodeKind::If {
                condition: node(condition),
                then_branch: node(then_branch),
                else_branch: node(else_branch),
            },
            TExprKind::Convert { expr, target } => NodeKind::Convert {
                expr: node(expr),
                target: Scoped::new(scope, target),
            },
            TExprKind::DisplayTimezone { expr, timezone } => NodeKind::DisplayTimezone {
                expr: node(expr),
                timezone,
            },
            TExprKind::Field { expr, field } => NodeKind::Field {
                expr: node(expr),
                field,
            },
            TExprKind::Construct {
                application,
                fields,
            } => NodeKind::Construct {
                application,
                fields: Scoped::new(scope, fields.as_slice()),
            },
            TExprKind::Map { entries } => NodeKind::Map {
                entries: Scoped::new(scope, entries.as_slice()),
            },
            TExprKind::For { bindings, body } => NodeKind::For {
                bindings,
                body: node(body),
            },
            TExprKind::Index { expr, args } => NodeKind::Index {
                expr: node(expr),
                args: Scoped::new(scope, args.as_slice()),
            },
            TExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => NodeKind::Scan {
                source: node(source),
                init: node(init),
                acc,
                val,
                body: node(body),
            },
            TExprKind::Unfold {
                recurrence,
                init,
                body,
            } => NodeKind::Unfold {
                recurrence,
                init: node(init),
                body: node(body),
            },
            TExprKind::Key { form, axis, arg } => NodeKind::Key {
                form,
                axis,
                arg: node(arg),
            },
            TExprKind::Match { scrutinee, arms } => NodeKind::Match {
                scrutinee: node(scrutinee),
                arms: match arms {
                    TMatchArms::Labels(arms) => {
                        ScopedMatchArms::Labels(Scoped::new(scope, arms.as_slice()))
                    }
                    TMatchArms::Constructors(arms) => {
                        ScopedMatchArms::Constructors(Scoped::new(scope, arms.as_slice()))
                    }
                },
            },
            TExprKind::Variant(variant) => NodeKind::Variant(variant),
            TExprKind::DagCall {
                slot, args, output, ..
            } => NodeKind::DagCall {
                call: Scoped::new(scope, slot),
                args: Scoped::new(scope, args.as_slice()),
                output,
            },
        }
    }

    /// Visit this node and every value node below it, in pre-order and
    /// including unselected branches, each in this node's scope.
    pub fn visit(self, visitor: &mut dyn FnMut(Self)) {
        let scope = self.scope();
        visit_tnodes(TNodeRef::Value(self.get()), &mut |node| {
            if let TNodeRef::Value(expr) = node {
                visitor(Scoped::new(scope, expr));
            }
        });
    }

    /// Every declaration this tree references through `@name`, including
    /// unselected branches, resolved in its own scope, in handle order.
    #[must_use]
    pub fn graph_refs(self) -> Vec<ResolvedDeclName> {
        let scope = self.scope();
        let mut handles = BTreeSet::new();
        visit_tnodes(TNodeRef::Value(self.get()), &mut |node| {
            if let TNodeRef::Value(expr) = node
                && let TExprKind::GraphRef(target) = expr.kind()
            {
                handles.insert(&target.value);
            }
        });
        handles
            .into_iter()
            .map(|handle| scope.resolve(handle))
            .collect()
    }
}

impl<'t> Scoped<'t, TIndexArg> {
    /// This index argument, in its access's scope.
    #[must_use]
    pub fn view(self) -> ScopedIndexArg<'t> {
        match self.get() {
            TIndexArg::Variant(variant) => ScopedIndexArg::Variant(variant),
            TIndexArg::Var(local) => ScopedIndexArg::Var(local),
            TIndexArg::Key(operand) => ScopedIndexArg::Key(Scoped::new(self.scope(), operand)),
            TIndexArg::Position { position, .. } => ScopedIndexArg::Position(position),
        }
    }
}

impl<'t> Scoped<'t, ResolvedUnitExpr> {
    /// Each term of the unit expression with the unit whose scale applies in
    /// the expression's scope.
    pub fn terms(
        self,
    ) -> impl Iterator<Item = (&'t ResolvedUnitExprItem<LocalUnit>, ResolvedUnitName)> + use<'t>
    {
        let scope = self.scope();
        self.get()
            .terms
            .iter()
            .map(move |term| (term, scope.resolve_unit(&term.name.value)))
    }

    /// The unit expression with every term resolved in its scope.
    #[must_use]
    pub fn resolved(self) -> ResolvedUnitExpr<ResolvedUnitRef> {
        let unit = self.get();
        ResolvedUnitExpr {
            terms: self
                .terms()
                .map(|(term, resolved)| ResolvedUnitExprItem {
                    op: term.op,
                    name: Spanned::new(
                        ResolvedUnitRef::new(term.name.value.spelling().clone(), resolved),
                        term.name.span,
                    ),
                    power: term.power,
                })
                .collect(),
            span: unit.span,
        }
    }
}
