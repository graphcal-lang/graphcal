//! `Raw` → `Desugared` conversions that carry real logic.
//!
//! Every phase-parameterized AST type converts its `Raw` form to its
//! `Desugared` form through a [`From`] impl. The mechanical structural
//! pass-throughs are generated next to the type definitions by
//! `#[derive(PhaseLift)]` (crate `graphcal-ast-derive`). This module holds
//! only the conversions that make a decision:
//!
//! - [`File`] and [`DagDecl`] bodies expand one declaration into many, because
//!   `DeclKind::Sugar(RawDeclSugar::Multi(_))` is expanded via
//!   `crate::syntax::desugar::expand_multi_decl` (see `convert_decl`).
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
//! # `MultiDecl` family
//!
//! `MultiDecl`, `MultiDeclSlot`, `MultiDeclSlice`, `MultiDataRow` are
//! parameterized over `<P>` for symmetry but only ever instantiated with
//! `<Raw>` (they live exclusively inside `RawDeclSugar::Multi`). They have
//! no `From<…<Raw>> for …<Desugared>` impl because the desugar pass
//! eliminates them entirely via `expand_multi_decl`.

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
    let kind = match kind {
        DeclKind::Sugar(RawDeclSugar::Multi(multi)) => {
            // `expand_multi_decl` produces one `ExpandedSlotDecl` per slot
            // (Param/Node/ConstNode only — never `Sugar`). Lift each to
            // `Declaration<Desugared>` so the rest of the pass sees a uniform
            // post-desugar type. A doc block above the multi-decl documents
            // every expanded slot.
            return crate::syntax::desugar::expand_multi_decl(&multi)
                .into_iter()
                .map(|slot| lift_slot_decl(slot, doc.clone()))
                .collect();
        }
        DeclKind::Param(p) => DeclKind::Param(p.into()),
        DeclKind::Node(n) => DeclKind::Node(n.into()),
        DeclKind::ConstNode(c) => DeclKind::ConstNode(c.into()),
        DeclKind::BaseDimension(d) => DeclKind::BaseDimension(d),
        DeclKind::Dimension(d) => DeclKind::Dimension(d),
        DeclKind::Unit(u) => DeclKind::Unit(u.into()),
        DeclKind::Type(t) => DeclKind::Type(t.into()),
        DeclKind::Index(i) => DeclKind::Index(i.into()),
        DeclKind::Import(i) => DeclKind::Import(i),
        DeclKind::PluginImport(p) => DeclKind::PluginImport(p.into()),
        DeclKind::Include(i) => DeclKind::Include(i.into()),
        DeclKind::Dag(d) => DeclKind::Dag(d.into()),
        DeclKind::Assert(a) => DeclKind::Assert(a.into()),
        DeclKind::Plot(p) => DeclKind::Plot(p.into()),
        DeclKind::Figure(f) => DeclKind::Figure(f.into()),
        DeclKind::Layer(l) => DeclKind::Layer(l.into()),
    };
    vec![Declaration {
        attributes,
        kind,
        span,
        doc,
    }]
}

/// Lift one multi-decl expansion slot to a `Declaration<Desugared>`.
///
/// [`ExpandedSlotDecl`] can only hold `Param` / `Node` / `ConstNode`, so no
/// unreachable `Sugar` arm (and no panic) is needed here.
fn lift_slot_decl(
    d: crate::syntax::desugar::ExpandedSlotDecl,
    doc: Option<crate::syntax::comments::DocComment>,
) -> Declaration<Desugared> {
    use crate::syntax::desugar::ExpandedSlotDecl;
    let (kind, span) = match d {
        ExpandedSlotDecl::Param(p, span) => (DeclKind::Param(p.into()), span),
        ExpandedSlotDecl::Node(n, span) => (DeclKind::Node(n.into()), span),
        ExpandedSlotDecl::ConstNode(c, span) => (DeclKind::ConstNode(c.into()), span),
    };
    Declaration {
        attributes: vec![],
        kind,
        span,
        doc,
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
