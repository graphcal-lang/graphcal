//! Dimension and unit expressions, which never contain value expressions.

use graphcal_compiler::syntax::ast::{DimExpr, DimTerm, MulDivOp, UnitExpr};
use pretty::RcDoc;

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
fn format_power(power: graphcal_compiler::dimension::Rational) -> String {
    power
        .fmt_exponent(graphcal_compiler::ratio::ExponentStyle::Source)
        .to_string()
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
