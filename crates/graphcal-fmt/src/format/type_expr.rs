use graphcal_compiler::syntax::ast::{DimExpr, DimTerm, MulDivOp, TypeExpr, TypeExprKind, UnitExpr};
use pretty::RcDoc;

// ---------------------------------------------------------------------------
// Type expressions
// ---------------------------------------------------------------------------

/// Format a type expression.
pub fn format_type_expr_inline(te: &TypeExpr) -> RcDoc<'static> {
    match &te.kind {
        TypeExprKind::Dimensionless => RcDoc::text("Dimensionless"),
        TypeExprKind::Bool => RcDoc::text("Bool"),
        TypeExprKind::Int => RcDoc::text("Int"),
        TypeExprKind::Datetime => RcDoc::text("Datetime"),
        TypeExprKind::DimExpr(de) => format_dim_expr_inline(de),
        TypeExprKind::Indexed { base, indexes } => {
            let idx_docs: Vec<RcDoc<'static>> = indexes
                .iter()
                .map(|i| RcDoc::text(i.name.clone()))
                .collect();
            format_type_expr_inline(base)
                .append(RcDoc::text("["))
                .append(RcDoc::intersperse(idx_docs, RcDoc::text(", ")))
                .append(RcDoc::text("]"))
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            let mut doc = RcDoc::text(name.name.clone());
            if !type_args.is_empty() {
                let arg_docs: Vec<RcDoc<'static>> = type_args
                    .iter()
                    .map(|a| format_type_expr_inline(a))
                    .collect();
                doc = doc
                    .append(RcDoc::text("<"))
                    .append(RcDoc::intersperse(arg_docs, RcDoc::text(", ")))
                    .append(RcDoc::text(">"));
            }
            doc
        }
    }
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
    let mut doc = RcDoc::text(t.name.name.clone());
    if let Some(power) = t.power {
        doc = doc.append(RcDoc::text(format!("^{power}")));
    }
    doc
}

// ---------------------------------------------------------------------------
// Unit expressions
// ---------------------------------------------------------------------------

pub fn format_unit_expr_inline(unit_expr: &UnitExpr) -> RcDoc<'static> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    for (i, item) in unit_expr.terms.iter().enumerate() {
        if i > 0 {
            match item.op {
                MulDivOp::Mul => docs.push(RcDoc::text(" * ")),
                MulDivOp::Div => docs.push(RcDoc::text("/")),
            }
        }
        let mut term = RcDoc::text(item.name.value.as_str().to_string());
        if let Some(power) = item.power {
            term = term.append(RcDoc::text(format!("^{power}")));
        }
        docs.push(term);
    }
    RcDoc::concat(docs)
}
