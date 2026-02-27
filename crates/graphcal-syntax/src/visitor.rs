//! Visitor traits for recursive traversal of [`ExprKind`] trees.
//!
//! These traits eliminate the need for hand-written recursive match expressions
//! across the codebase. Two traits are provided:
//!
//! - [`ExprVisitor`] for read-only traversals (reference collection, validation)
//! - [`ExprVisitorMut`] for in-place rewriting (name prefixing, qualification rewriting)
//!
//! Default implementations recurse into child expressions. Implementors override
//! only the leaf methods they care about.

use crate::ast::{Expr, ExprKind};

/// Read-only visitor for [`Expr`] trees.
///
/// Default implementations for container nodes recurse into children.
/// Override leaf methods to intercept specific node types.
pub trait ExprVisitor {
    type Error;

    /// Top-level dispatch. Override to add pre/post-visit logic.
    fn visit_expr(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        self.dispatch(expr)
    }

    /// Dispatches to the appropriate handler based on [`ExprKind`].
    /// Typically not overridden.
    fn dispatch(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        match &expr.kind {
            ExprKind::Number(_)
            | ExprKind::Integer(_)
            | ExprKind::Bool(_)
            | ExprKind::StringLiteral(_)
            | ExprKind::UnitLiteral { .. }
            | ExprKind::LocalRef(_)
            | ExprKind::VariantLiteral { .. } => self.visit_leaf(expr),

            ExprKind::GraphRef(_) => self.visit_graph_ref(expr),
            ExprKind::ConstRef(_) => self.visit_const_ref(expr),
            ExprKind::QualifiedGraphRef { .. } => self.visit_qualified_graph_ref(expr),
            ExprKind::QualifiedConstRef { .. } => self.visit_qualified_const_ref(expr),

            ExprKind::FnCall { args, .. } => self.visit_fn_call(expr, args),
            ExprKind::QualifiedFnCall { args, .. } => self.visit_qualified_fn_call(expr, args),

            ExprKind::BinOp { lhs, rhs, .. } => self.visit_bin_op(expr, lhs, rhs),
            ExprKind::UnaryOp { operand, .. } => self.visit_unary_op(expr, operand),

            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.visit_if(expr, condition, then_branch, else_branch),

            ExprKind::Convert { expr: inner, .. }
            | ExprKind::DisplayTimezone { expr: inner, .. }
            | ExprKind::AsCast { expr: inner, .. }
            | ExprKind::FieldAccess { expr: inner, .. }
            | ExprKind::IndexAccess { expr: inner, .. } => self.visit_single_child(expr, inner),

            ExprKind::Block { stmts, expr: body } => self.visit_block(expr, stmts, body),

            ExprKind::StructConstruction { fields, .. } => {
                self.visit_struct_construction(expr, fields)
            }

            ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
                self.visit_map_entries(expr, entries)
            }

            ExprKind::ForComp { body, .. } => self.visit_expr(body),

            ExprKind::Scan {
                source, init, body, ..
            } => self.visit_scan(expr, source, init, body),

            ExprKind::Unfold { init, body, .. } => self.visit_unfold(expr, init, body),

            ExprKind::Match { scrutinee, arms } => self.visit_match(expr, scrutinee, arms),

            ExprKind::TupleMatch { scrutinees, arms } => {
                self.visit_tuple_match(expr, scrutinees, arms)
            }
        }
    }

    // -- Leaf handlers (default: no-op) --

    /// Called for literal/reference leaves that have no sub-expressions.
    fn visit_leaf(&mut self, _expr: &Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_graph_ref(&mut self, _expr: &Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_const_ref(&mut self, _expr: &Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_qualified_graph_ref(&mut self, _expr: &Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_qualified_const_ref(&mut self, _expr: &Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    // -- Container handlers (default: recurse into children) --

    fn visit_fn_call(&mut self, _expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }

    fn visit_qualified_fn_call(&mut self, _expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }

    fn visit_bin_op(&mut self, _expr: &Expr, lhs: &Expr, rhs: &Expr) -> Result<(), Self::Error> {
        self.visit_expr(lhs)?;
        self.visit_expr(rhs)
    }

    fn visit_unary_op(&mut self, _expr: &Expr, operand: &Expr) -> Result<(), Self::Error> {
        self.visit_expr(operand)
    }

    fn visit_if(
        &mut self,
        _expr: &Expr,
        condition: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
    ) -> Result<(), Self::Error> {
        self.visit_expr(condition)?;
        self.visit_expr(then_branch)?;
        self.visit_expr(else_branch)
    }

    /// Called for `Convert`, `DisplayTimezone`, `AsCast`, `FieldAccess`, `IndexAccess`.
    fn visit_single_child(&mut self, _expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        self.visit_expr(inner)
    }

    fn visit_block(
        &mut self,
        _expr: &Expr,
        stmts: &[crate::ast::LetBinding],
        body: &Expr,
    ) -> Result<(), Self::Error> {
        for stmt in stmts {
            self.visit_expr(&stmt.value)?;
        }
        self.visit_expr(body)
    }

    fn visit_struct_construction(
        &mut self,
        _expr: &Expr,
        fields: &[crate::ast::FieldInit],
    ) -> Result<(), Self::Error> {
        for field in fields {
            if let Some(val) = &field.value {
                self.visit_expr(val)?;
            }
        }
        Ok(())
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr,
        entries: &[crate::ast::MapEntry],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_scan(
        &mut self,
        _expr: &Expr,
        source: &Expr,
        init: &Expr,
        body: &Expr,
    ) -> Result<(), Self::Error> {
        self.visit_expr(source)?;
        self.visit_expr(init)?;
        self.visit_expr(body)
    }

    fn visit_unfold(&mut self, _expr: &Expr, init: &Expr, body: &Expr) -> Result<(), Self::Error> {
        self.visit_expr(init)?;
        self.visit_expr(body)
    }

    fn visit_match(
        &mut self,
        _expr: &Expr,
        scrutinee: &Expr,
        arms: &[crate::ast::MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }

    fn visit_tuple_match(
        &mut self,
        _expr: &Expr,
        scrutinees: &[Expr],
        arms: &[crate::ast::TupleMatchArm],
    ) -> Result<(), Self::Error> {
        for s in scrutinees {
            self.visit_expr(s)?;
        }
        for arm in arms {
            if let Some(patterns) = &arm.patterns {
                for p in patterns {
                    self.visit_expr(p)?;
                }
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }
}

/// Mutable visitor for in-place rewriting of [`Expr`] trees.
///
/// Same structure as [`ExprVisitor`] but takes `&mut Expr` references.
pub trait ExprVisitorMut {
    type Error;

    fn visit_expr_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        self.dispatch_mut(expr)
    }

    fn dispatch_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        match &mut expr.kind {
            ExprKind::Number(_)
            | ExprKind::Integer(_)
            | ExprKind::Bool(_)
            | ExprKind::StringLiteral(_)
            | ExprKind::UnitLiteral { .. }
            | ExprKind::LocalRef(_)
            | ExprKind::VariantLiteral { .. } => Ok(()),

            ExprKind::GraphRef(_) => self.visit_graph_ref_mut(expr),
            ExprKind::ConstRef(_) => self.visit_const_ref_mut(expr),
            ExprKind::QualifiedGraphRef { .. } => self.visit_qualified_graph_ref_mut(expr),
            ExprKind::QualifiedConstRef { .. } => self.visit_qualified_const_ref_mut(expr),

            ExprKind::FnCall { .. } => self.visit_fn_call_mut(expr),
            ExprKind::QualifiedFnCall { .. } => self.visit_qualified_fn_call_mut(expr),

            ExprKind::BinOp { lhs, rhs, .. } => {
                self.visit_expr_mut(lhs)?;
                self.visit_expr_mut(rhs)
            }
            ExprKind::UnaryOp { operand, .. } => self.visit_expr_mut(operand),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.visit_expr_mut(condition)?;
                self.visit_expr_mut(then_branch)?;
                self.visit_expr_mut(else_branch)
            }
            ExprKind::Convert { expr: inner, .. }
            | ExprKind::DisplayTimezone { expr: inner, .. }
            | ExprKind::AsCast { expr: inner, .. }
            | ExprKind::FieldAccess { expr: inner, .. }
            | ExprKind::IndexAccess { expr: inner, .. } => self.visit_expr_mut(inner),
            ExprKind::Block { stmts, expr: body } => {
                for stmt in stmts {
                    self.visit_expr_mut(&mut stmt.value)?;
                }
                self.visit_expr_mut(body)
            }
            ExprKind::StructConstruction { fields, .. } => {
                for field in fields {
                    if let Some(val) = &mut field.value {
                        self.visit_expr_mut(val)?;
                    }
                }
                Ok(())
            }
            ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
                for entry in entries {
                    self.visit_expr_mut(&mut entry.value)?;
                }
                Ok(())
            }
            ExprKind::ForComp { body, .. } => self.visit_expr_mut(body),
            ExprKind::Scan {
                source, init, body, ..
            } => {
                self.visit_expr_mut(source)?;
                self.visit_expr_mut(init)?;
                self.visit_expr_mut(body)
            }
            ExprKind::Unfold { init, body, .. } => {
                self.visit_expr_mut(init)?;
                self.visit_expr_mut(body)
            }
            ExprKind::Match { scrutinee, arms } => {
                self.visit_expr_mut(scrutinee)?;
                for arm in arms {
                    self.visit_expr_mut(&mut arm.body)?;
                }
                Ok(())
            }
            ExprKind::TupleMatch { scrutinees, arms } => {
                for s in scrutinees {
                    self.visit_expr_mut(s)?;
                }
                for arm in arms {
                    if let Some(patterns) = &mut arm.patterns {
                        for p in patterns {
                            self.visit_expr_mut(p)?;
                        }
                    }
                    self.visit_expr_mut(&mut arm.body)?;
                }
                Ok(())
            }
        }
    }

    // -- Leaf handlers for mutable visitor (default: no-op) --

    fn visit_graph_ref_mut(&mut self, _expr: &mut Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_const_ref_mut(&mut self, _expr: &mut Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_qualified_graph_ref_mut(&mut self, _expr: &mut Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_qualified_const_ref_mut(&mut self, _expr: &mut Expr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fn_call_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { args, .. } = &mut expr.kind {
            for arg in args {
                self.visit_expr_mut(arg)?;
            }
        }
        Ok(())
    }

    fn visit_qualified_fn_call_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedFnCall { args, .. } = &mut expr.kind {
            for arg in args {
                self.visit_expr_mut(arg)?;
            }
        }
        Ok(())
    }
}
