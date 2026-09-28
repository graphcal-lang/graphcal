//! `Raw` → `Desugared` conversions that carry real logic.
//!
//! Every phase-parameterized AST type converts its `Raw` form to its
//! `Desugared` form through a [`From`] impl. The mechanical structural
//! pass-throughs are generated next to the type definitions by
//! `#[derive(PhaseLift)]` (crate `graphcal-ast-derive`). This module holds
//! only the conversions that make a decision:
//!
//! - [`File`] and [`DagDecl`] bodies expand one declaration into many: the
//!   derived `TryFrom<DeclKind<Raw>>` hands `DeclKind::Sugar(_)` back, and
//!   `convert_decl` expands a multi-decl through `super::multi`.
//! - [`Expr`] routes each tree level through the stack-growth guard and its
//!   private-marker constructor.
//! - [`RawExprSugar`] lowers to an ordinary [`ExprKind`]; the derived
//!   `ExprKind` impl dispatches its `Sugar` variant here.
//!
//! These impls let consumers say `vec_of_raw.into_iter().map(Into::into)` or
//! `option_of_raw.map(Into::into)` to lift any AST tree from `Raw` to
//! `Desugared`. The desugar pass uses them to produce `File<Desugared>`.
//!
//! # Phase-invariant types
//!
//! Types without a `<P>` parameter (e.g., `Attribute`, `Ident`, `ModulePath`,
//! `DimExpr`, `UnitExpr`, `IndexExpr`, `NatExpr`, `MapEntryKey`,
//! `MatchPattern`, etc.) are used as-is in both phases — no conversion
//! needed.
//!
//! The `MultiDecl` family is raw-only (it lives exclusively inside
//! `RawDeclSugar::Multi`) and carries no phase parameter; the desugar pass
//! eliminates it.

use crate::syntax::ast::{DagDecl, DeclKind, Declaration, Expr, ExprKind, File};
use crate::syntax::ast::{RawDeclSugar, RawExprSugar};
use crate::syntax::phase::{Desugared, Raw};

// ---------------------------------------------------------------------------
// File / Declaration / DeclKind
// ---------------------------------------------------------------------------

impl From<File<Raw>> for File<Desugared> {
    fn from(f: File<Raw>) -> Self {
        Self {
            declarations: f.declarations.into_iter().flat_map(convert_decl).collect(),
        }
    }
}

/// Convert one `Declaration<Raw>` into N `Declaration<Desugared>`.
///
/// Returns a `Vec` because multi-decl sugar expands one declaration into
/// many. All other variants produce exactly one output declaration.
fn convert_decl(d: Declaration<Raw>) -> Vec<Declaration<Desugared>> {
    let Declaration {
        attributes,
        kind,
        span,
        doc,
    } = d;
    match DeclKind::try_from(kind) {
        Ok(kind) => vec![Declaration {
            attributes,
            kind,
            span,
            doc,
        }],
        // The parser rejects attributes on a multi-decl; its doc block
        // documents every expanded slot.
        Err(RawDeclSugar::Multi(multi)) => super::multi::expand_multi_decl(&multi, doc.as_ref()),
    }
}

impl From<DagDecl<Raw>> for DagDecl<Desugared> {
    fn from(d: DagDecl<Raw>) -> Self {
        Self {
            visibility: d.visibility,
            name: d.name,
            body: d.body.into_iter().flat_map(convert_decl).collect(),
            span: d.span,
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl From<Expr<Raw>> for Expr<Desugared> {
    fn from(e: Expr<Raw>) -> Self {
        // Recursion choke point: conversion recurses once per tree level
        // (unbounded for left-nested operator chains).
        let (kind, span) = e.into_parts();
        crate::stack::with_stack_growth(|| Self::new(kind.into(), span))
    }
}

/// Expression sugar lowering, dispatched from the derived
/// `From<ExprKind<Raw>> for ExprKind<Desugared>` (`#[phase_lift(from_payload)]`).
impl From<RawExprSugar> for ExprKind<Desugared> {
    fn from(sugar: RawExprSugar) -> Self {
        match sugar {
            RawExprSugar::TableLiteral {
                indexes: _,
                entries,
            } => {
                // Drop the `indexes` metadata — the entries already carry
                // full typed keys (including numeric positions for `Fin`
                // axes). The
                // `table` keyword is purely surface syntax preserved by the
                // formatter via the raw AST; downstream stages see the
                // canonical map form.
                Self::MapLiteral {
                    entries: entries.into_iter().map(Into::into).collect(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::BinOp;
    use crate::syntax::span::Span;

    const LONG_CHAIN_TERM_COUNT: usize = 512;

    fn term_span(index: usize) -> Span {
        Span::new(index * 4, 1)
    }

    fn chain_span(last_term_index: usize) -> Span {
        Span::new(0, last_term_index * 4 + 1)
    }

    #[test]
    fn long_expression_conversion_moves_payloads_once_and_preserves_spans() {
        let terms_with_allocations = (0..LONG_CHAIN_TERM_COUNT).map(|index| {
            let value = format!("term-{index:04}-payload");
            let allocation = value.as_ptr();
            (
                Expr::new(ExprKind::StringLiteral(value), term_span(index)),
                allocation,
            )
        });
        let (terms, original_allocations): (Vec<Expr<Raw>>, Vec<*const u8>) =
            terms_with_allocations.unzip();

        let mut terms = terms.into_iter();
        let first = terms.next().expect("long chain has a first term");
        let raw = terms.enumerate().fold(first, |lhs, (offset, rhs)| {
            let term_index = offset + 1;
            Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                chain_span(term_index),
            )
        });

        let desugared: Expr<Desugared> = raw.into();

        // Moving each `String` keeps its allocation identity. The former
        // `e.kind.clone()` implementation allocated a fresh String for every
        // remaining leaf at every level of this left-nested chain, so this
        // deterministic assertion pins the conversion to move rather than
        // quadratic deep-cloning without relying on wall-clock timing.
        let mut current = &desugared;
        for term_index in (1..LONG_CHAIN_TERM_COUNT).rev() {
            assert_eq!(current.span, chain_span(term_index));
            let ExprKind::BinOp {
                op: BinOp::Add,
                lhs,
                rhs,
            } = &current.kind
            else {
                panic!("expected left-nested addition at term {term_index}");
            };
            assert_eq!(rhs.span, term_span(term_index));
            let ExprKind::StringLiteral(value) = &rhs.kind else {
                panic!("expected string term {term_index}");
            };
            assert!(
                std::ptr::eq(value.as_ptr(), original_allocations[term_index]),
                "term {term_index} payload was cloned"
            );
            current = lhs;
        }

        assert_eq!(current.span, term_span(0));
        let ExprKind::StringLiteral(value) = &current.kind else {
            panic!("expected first string term");
        };
        assert!(
            std::ptr::eq(value.as_ptr(), original_allocations[0]),
            "first term payload was cloned"
        );
    }
}
