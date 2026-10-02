//! Visitor trait for read-only recursive traversal of [`ExprKind`] trees.
//!
//! `ExprVisitor` eliminates the need for hand-written recursive match
//! expressions in read-only traversals (reference collection, validation).
//!
//! The trait is generic over the AST [`Phase`]: a single visitor can walk
//! either `Expr<Raw>` (parser output, surface-aware tooling) or
//! `Expr<Desugared>` (post-desugar consumers). The dispatch logic is
//! phase-invariant — variants and field shapes are identical across phases —
//! so the same default-method bodies work for both.
//!
//! Default implementations recurse into child expressions. Implementors override
//! only the leaf methods they care about.

use crate::syntax::ast::{Expr, ExprKind, GenericArg, IndexArg, TypeExpr, TypeExprKind};
use crate::syntax::phase::Phase;

/// Read-only visitor for [`Expr`] trees, generic over [`Phase`].
///
/// Default implementations for container nodes recurse into children.
/// Override leaf methods to intercept specific node types.
pub trait ExprVisitor<P: Phase> {
    type Error;

    /// Top-level dispatch. Override to add pre/post-visit logic.
    fn visit_expr(&mut self, expr: &Expr<P>) -> Result<(), Self::Error> {
        self.dispatch(expr)
    }

    /// Dispatches to the appropriate handler based on [`ExprKind`].
    /// Typically not overridden.
    ///
    /// Grows the stack on demand: visitors recurse once per expression-tree
    /// level, and left-nested operator chains make that depth unbounded.
    fn dispatch(&mut self, expr: &Expr<P>) -> Result<(), Self::Error> {
        crate::stack::with_stack_growth(|| self.dispatch_inner(expr))
    }

    /// Body of [`Self::dispatch`]. Not meant to be overridden or called
    /// directly — call [`Self::dispatch`] so the stack-growth guard runs.
    fn dispatch_inner(&mut self, expr: &Expr<P>) -> Result<(), Self::Error> {
        match &expr.kind {
            ExprKind::Number(_)
            | ExprKind::Integer(_)
            | ExprKind::Bool(_)
            | ExprKind::StringLiteral(_)
            | ExprKind::QuantityLiteral { .. } => self.visit_leaf(expr),

            ExprKind::UnresolvedRef(_) => self.visit_unresolved_ref(expr),
            ExprKind::GraphRef(_) => self.visit_graph_ref(expr),
            ExprKind::InlineDagRef { args, .. } => self.visit_inline_dag_ref(expr, args),

            ExprKind::FnCall {
                generic_args, args, ..
            } => {
                self.visit_generic_args(generic_args)?;
                self.visit_fn_call(expr, args)
            }

            ExprKind::BinOp { lhs, rhs, .. } => self.visit_bin_op(expr, lhs, rhs),
            ExprKind::UnaryOp { operand, .. } => self.visit_unary_op(expr, operand),

            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.visit_if(expr, condition, then_branch, else_branch),

            ExprKind::Convert { expr: inner, .. }
            | ExprKind::DisplayTimezone { expr: inner, .. }
            | ExprKind::FieldAccess { expr: inner, .. } => self.visit_single_child(expr, inner),

            ExprKind::IndexAccess {
                expr: inner, args, ..
            } => {
                self.visit_single_child(expr, inner)?;
                for arg in args {
                    if let IndexArg::Expr(e) = arg {
                        self.visit_expr(e)?;
                    }
                }
                Ok(())
            }

            ExprKind::ConstructorCall {
                generic_args,
                fields,
                ..
            } => {
                self.visit_generic_args(generic_args)?;
                self.visit_constructor_call(expr, fields)
            }

            ExprKind::MapLiteral { entries } => self.visit_map_entries(expr, entries),

            ExprKind::ForComp { body, .. } => self.visit_expr(body),

            ExprKind::Scan {
                source, init, body, ..
            } => self.visit_scan(expr, source, init, body),

            ExprKind::Unfold { init, body, .. } => self.visit_unfold(expr, init, body),

            ExprKind::KeyForm { arg, .. } => self.visit_expr(arg),

            ExprKind::Match { scrutinee, arms } => self.visit_match(expr, scrutinee, arms),

            // Phase-specific sugar. Default: ignore — Raw consumers that need
            // to walk into sugar (formatter) bypass the visitor; Desugared
            // consumers' Sugar payload is `Infallible` so this arm is
            // statically unreachable.
            ExprKind::Sugar(_) => self.visit_sugar(expr),
        }
    }

    // -- Leaf handlers (default: no-op) --

    /// Called for literal/reference leaves that have no sub-expressions.
    fn visit_leaf(&mut self, _expr: &Expr<P>) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Called for `Sugar` variants (Raw-only surface forms). Default: no-op.
    /// Override in Raw-phase visitors that need to walk into sugar payloads.
    fn visit_sugar(&mut self, _expr: &Expr<P>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_graph_ref(&mut self, _expr: &Expr<P>) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Called for unresolved reference paths (`Foo`, `Foo#Bar`, ...). Leaf
    /// node — classification happens in HIR lowering, so visitors that care
    /// about reference shapes inspect the syntactic path.
    fn visit_unresolved_ref(&mut self, _expr: &Expr<P>) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Called for `InlineDagRef`. Default: recurse into binding value expressions.
    fn visit_inline_dag_ref(
        &mut self,
        _expr: &Expr<P>,
        args: &[crate::syntax::ast::ParamBinding<P>],
    ) -> Result<(), Self::Error> {
        for arg in args {
            self.visit_expr(&arg.value)?;
        }
        Ok(())
    }

    // -- Container handlers (default: recurse into children) --

    fn visit_generic_args(&mut self, args: &[GenericArg<P>]) -> Result<(), Self::Error> {
        for arg in args {
            if let GenericArg::Type(type_expr) = arg {
                self.visit_type_expr(type_expr)?;
            }
        }
        Ok(())
    }

    fn visit_type_expr(&mut self, type_expr: &TypeExpr<P>) -> Result<(), Self::Error> {
        for bound in &type_expr.constraints {
            self.visit_expr(&bound.value)?;
        }
        match &type_expr.kind {
            TypeExprKind::DatetimeApplication { type_args } => {
                for arg in type_args {
                    self.visit_type_expr(arg)?;
                }
            }
            TypeExprKind::TypeApplication { generic_args, .. } => {
                self.visit_generic_args(generic_args.as_slice())?;
            }
            TypeExprKind::ComplexApplication { generic_args }
            | TypeExprKind::KeyApplication { generic_args } => {
                self.visit_generic_args(generic_args)?;
            }
            TypeExprKind::Indexed { base, .. } => self.visit_type_expr(base)?,
            TypeExprKind::IndexLabel { .. }
            | TypeExprKind::Dimensionless
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime
            | TypeExprKind::DimExpr(_) => {}
        }
        Ok(())
    }

    fn visit_fn_call(&mut self, _expr: &Expr<P>, args: &[Expr<P>]) -> Result<(), Self::Error> {
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }

    fn visit_bin_op(
        &mut self,
        _expr: &Expr<P>,
        lhs: &Expr<P>,
        rhs: &Expr<P>,
    ) -> Result<(), Self::Error> {
        self.visit_expr(lhs)?;
        self.visit_expr(rhs)
    }

    fn visit_unary_op(&mut self, _expr: &Expr<P>, operand: &Expr<P>) -> Result<(), Self::Error> {
        self.visit_expr(operand)
    }

    fn visit_if(
        &mut self,
        _expr: &Expr<P>,
        condition: &Expr<P>,
        then_branch: &Expr<P>,
        else_branch: &Expr<P>,
    ) -> Result<(), Self::Error> {
        self.visit_expr(condition)?;
        self.visit_expr(then_branch)?;
        self.visit_expr(else_branch)
    }

    /// Called for `Convert`, `DisplayTimezone`, `FieldAccess`, `IndexAccess`.
    fn visit_single_child(&mut self, _expr: &Expr<P>, inner: &Expr<P>) -> Result<(), Self::Error> {
        self.visit_expr(inner)
    }

    fn visit_constructor_call(
        &mut self,
        _expr: &Expr<P>,
        fields: &[crate::syntax::ast::FieldInit<P>],
    ) -> Result<(), Self::Error> {
        for field in fields {
            self.visit_expr(&field.value)?;
        }
        Ok(())
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr<P>,
        entries: &[crate::syntax::ast::MapEntry<P>],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_scan(
        &mut self,
        _expr: &Expr<P>,
        source: &Expr<P>,
        init: &Expr<P>,
        body: &Expr<P>,
    ) -> Result<(), Self::Error> {
        self.visit_expr(source)?;
        self.visit_expr(init)?;
        self.visit_expr(body)
    }

    fn visit_unfold(
        &mut self,
        _expr: &Expr<P>,
        init: &Expr<P>,
        body: &Expr<P>,
    ) -> Result<(), Self::Error> {
        self.visit_expr(init)?;
        self.visit_expr(body)
    }

    fn visit_match(
        &mut self,
        _expr: &Expr<P>,
        scrutinee: &Expr<P>,
        arms: &[crate::syntax::ast::MatchArm<P>],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }
}
