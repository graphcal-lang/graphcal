use graphcal_compiler::syntax::ast::{
    DimExpr, DimTerm, DomainBound, MulDivOp, TypeExpr, TypeExprKind, UnitExpr,
};
use pretty::RcDoc;

use super::Formatter;

// ---------------------------------------------------------------------------
// Type expressions
// ---------------------------------------------------------------------------

/// Format a type expression.
pub fn format_type_expr_inline(fmt: &mut Formatter<'_>, te: &TypeExpr) -> RcDoc<'static> {
    let base = match &te.kind {
        TypeExprKind::Dimensionless => RcDoc::text("Dimensionless"),
        TypeExprKind::Bool => RcDoc::text("Bool"),
        TypeExprKind::Int => RcDoc::text("Int"),
        TypeExprKind::Datetime => RcDoc::text("Datetime"),
        TypeExprKind::DimExpr(de) => format_dim_expr_inline(de),
        TypeExprKind::Indexed { base, indexes } => {
            let idx_docs: Vec<RcDoc<'static>> = indexes
                .iter()
                .map(|i| match i {
                    graphcal_compiler::syntax::ast::IndexExpr::Name(name) => {
                        RcDoc::text(name.value.display_path())
                    }
                    graphcal_compiler::syntax::ast::IndexExpr::NatExpr(nat_expr) => {
                        RcDoc::text(nat_expr.to_string())
                    }
                })
                .collect();
            format_type_expr_inline(fmt, base)
                .append(RcDoc::text("["))
                .append(RcDoc::intersperse(idx_docs, RcDoc::text(", ")))
                .append(RcDoc::text("]"))
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            let mut doc = RcDoc::text(name.value.display_path());
            if !type_args.is_empty() {
                let arg_docs: Vec<RcDoc<'static>> = type_args
                    .iter()
                    .map(|a| format_type_expr_inline(fmt, a))
                    .collect();
                doc = doc
                    .append(RcDoc::text("<"))
                    .append(RcDoc::intersperse(arg_docs, RcDoc::text(", ")))
                    .append(RcDoc::text(">"));
            }
            doc
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            let arg_docs: Vec<RcDoc<'static>> = type_args
                .iter()
                .map(|a| format_type_expr_inline(fmt, a))
                .collect();
            RcDoc::text("Datetime")
                .append(RcDoc::text("<"))
                .append(RcDoc::intersperse(arg_docs, RcDoc::text(", ")))
                .append(RcDoc::text(">"))
        }
    };

    if te.constraints.is_empty() {
        base
    } else {
        base.append(format_domain_constraints(fmt, &te.constraints))
    }
}

/// Format domain constraints: `(min: expr, max: expr)`.
fn format_domain_constraints(
    fmt: &mut Formatter<'_>,
    constraints: &[DomainBound],
) -> RcDoc<'static> {
    let docs: Vec<RcDoc<'static>> = constraints
        .iter()
        .map(|bound| {
            RcDoc::text(bound.kind.to_string())
                .append(RcDoc::text(": "))
                .append(super::expr::format_expr(fmt, &bound.value))
        })
        .collect();
    RcDoc::text("(")
        .append(RcDoc::intersperse(docs, RcDoc::text(", ")))
        .append(RcDoc::text(")"))
}

// ---------------------------------------------------------------------------
// Dimension expressions
// ---------------------------------------------------------------------------

pub fn format_dim_expr_inline(de: &DimExpr) -> RcDoc<'static> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    for (i, item) in de.terms.iter().enumerate() {
        if i > 0 {
            match item.op {
                MulDivOp::Mul => docs.push(RcDoc::text(" * ")),
                MulDivOp::Div => docs.push(RcDoc::text(" / ")),
            }
        }
        docs.push(format_dim_term(&item.term));
    }
    RcDoc::concat(docs)
}

fn format_dim_term(t: &DimTerm) -> RcDoc<'static> {
    let mut doc = RcDoc::text(t.name.value.display_path());
    if let Some(power) = t.power {
        doc = doc.append(RcDoc::text(format_power(power)));
    }
    doc
}

// ---------------------------------------------------------------------------
// Unit expressions
// ---------------------------------------------------------------------------

/// Render an exponent suffix: `^2` for integers, `^(1/2)` for rationals —
/// the parenthesized form is what the grammar accepts back.
fn format_power(power: graphcal_compiler::syntax::dimension::Rational) -> String {
    if power.is_integer() {
        format!("^{}", power.num())
    } else {
        format!("^({}/{})", power.num(), power.den())
    }
}

pub fn format_unit_expr_inline(unit_expr: &UnitExpr) -> RcDoc<'static> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    for (i, item) in unit_expr.terms.iter().enumerate() {
        match (i, item.op) {
            (0, MulDivOp::Mul) => {}
            // `1/unit` shorthand: a leading division carries an implicit `1`
            // numerator that must be preserved to stay parseable.
            (0, MulDivOp::Div) => docs.push(RcDoc::text("1/")),
            (_, MulDivOp::Mul) => docs.push(RcDoc::text(" * ")),
            (_, MulDivOp::Div) => docs.push(RcDoc::text("/")),
        }
        let mut term = RcDoc::text(item.name.value.to_string());
        if let Some(power) = item.power {
            term = term.append(RcDoc::text(format_power(power)));
        }
        docs.push(term);
    }
    RcDoc::concat(docs)
}
