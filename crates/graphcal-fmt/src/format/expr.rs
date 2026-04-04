use graphcal_compiler::syntax::ast::{
    BinOp, Expr, ExprKind, FieldInit, ForBinding, Ident, IndexArg, LetBinding, MapEntry, MatchArm,
    MatchPattern, PatternBinding, TupleMatchArm, TypeExpr, UnaryOp,
};
use graphcal_compiler::syntax::names::{IndexName, Spanned};
use pretty::RcDoc;

use super::{
    Formatter, INDENT, format_type_expr_inline, format_unit_expr_inline, is_nil, prepend_comments,
    render_doc_to_string,
};

// ---------------------------------------------------------------------------
// Nat expression formatting
// ---------------------------------------------------------------------------

/// Format a `NatExpr` to a string (public for use by `type_expr` formatter).
pub(super) fn format_nat_expr_str_pub(expr: &graphcal_compiler::syntax::ast::NatExpr) -> String {
    format_nat_expr_str(expr)
}

/// Format a `NatExpr` to a string.
fn format_nat_expr_str(expr: &graphcal_compiler::syntax::ast::NatExpr) -> String {
    use graphcal_compiler::syntax::ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => n.to_string(),
        NatExpr::Var(ident) => ident.name.clone(),
        NatExpr::Add(lhs, rhs, _) => {
            format!(
                "{} + {}",
                format_nat_expr_str(lhs),
                format_nat_expr_str(rhs)
            )
        }
        NatExpr::Mul(lhs, rhs, _) => {
            format!(
                "{} * {}",
                format_nat_expr_str(lhs),
                format_nat_expr_str(rhs)
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[expect(clippy::too_many_lines, reason = "match on ExprKind variants")]
pub fn format_expr(fmt: &mut Formatter<'_>, expr: &Expr) -> RcDoc<'static> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Integer(_) => {
            // Recover original text from source to preserve formatting (e.g. 1_000, 3.98e5)
            RcDoc::text(fmt.slice(expr.span).to_string())
        }
        ExprKind::Bool(b) => RcDoc::text(if *b { "true" } else { "false" }),
        ExprKind::StringLiteral(s) => RcDoc::text(format!("\"{s}\"")),
        ExprKind::GraphRef(name) => RcDoc::text(format!("@{}", name.value.as_str())),
        ExprKind::QualifiedGraphRef { module, name } => RcDoc::text(format!(
            "@{}::{}",
            module.name.as_str(),
            name.value.as_str()
        )),
        ExprKind::ConstRef(name) => RcDoc::text(name.value.as_str().to_string()),
        ExprKind::QualifiedConstRef { module, name } => {
            RcDoc::text(format!("{}::{}", module.name.as_str(), name.value.as_str()))
        }
        ExprKind::LocalRef(ident) => RcDoc::text(ident.name.clone()),
        ExprKind::BinOp { op, lhs, rhs } => format_binop(fmt, *op, lhs, rhs),
        ExprKind::UnaryOp { op, operand } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            RcDoc::text(op_str).append(format_expr(fmt, operand))
        }
        ExprKind::FnCall {
            name,
            type_args,
            args,
        } => format_fn_call_expr(fmt, name.value.as_str(), type_args, args),
        ExprKind::QualifiedFnCall {
            module,
            name,
            type_args,
            args,
        } => {
            let fn_name = format!("{}::{}", module.name.as_str(), name.value.as_str());
            format_fn_call_expr(fmt, &fn_name, type_args, args)
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => format_if(fmt, condition, then_branch, else_branch),
        ExprKind::UnitLiteral { value: _, unit } => {
            // Recover the full literal from source to preserve number formatting
            let unit_start = unit.span.offset();
            let lit_source = &fmt.source[expr.span.offset()..unit_start];
            let lit_text = lit_source.trim_end();
            RcDoc::text(lit_text.to_string())
                .append(RcDoc::text(" "))
                .append(format_unit_expr_inline(unit))
        }
        ExprKind::Convert {
            expr: inner,
            target,
        } => format_expr(fmt, inner)
            .append(RcDoc::text(" -> "))
            .append(format_unit_expr_inline(target)),
        ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => format_expr(fmt, inner)
            .append(RcDoc::text(" -> "))
            .append(RcDoc::text(format!("\"{timezone}\""))),
        ExprKind::AsCast {
            expr: inner,
            target_type,
        } => format_expr(fmt, inner)
            .append(RcDoc::text(" as "))
            .append(format_type_expr_inline(target_type)),
        ExprKind::Block { stmts, expr: tail } => {
            let body = format_block_body(fmt, stmts, tail);
            RcDoc::text("{")
                .append(RcDoc::hardline().append(body).nest(INDENT))
                .append(RcDoc::hardline())
                .append(RcDoc::text("}"))
        }
        ExprKind::FieldAccess { expr: inner, field } => format_expr(fmt, inner)
            .append(RcDoc::text("."))
            .append(RcDoc::text(field.value.as_str().to_string())),
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } => format_struct_construction(fmt, type_name, type_args, fields),
        ExprKind::MapLiteral { entries } => format_map_literal(fmt, entries),
        ExprKind::TableLiteral { indexes, entries } => format_table_literal(fmt, indexes, entries),
        ExprKind::ForComp { bindings, body } => format_for_comp(fmt, bindings, body),
        ExprKind::IndexAccess { expr: inner, args } => {
            let arg_docs: Vec<RcDoc<'static>> = args
                .iter()
                .map(|a| match a {
                    IndexArg::Variant { index, variant } => RcDoc::text(format!(
                        "{}::{}",
                        index.value.as_str(),
                        variant.value.as_str()
                    )),
                    IndexArg::Var(ident) => RcDoc::text(ident.name.clone()),
                    IndexArg::Expr(e) => format_expr(fmt, e),
                })
                .collect();
            format_expr(fmt, inner)
                .append(RcDoc::text("["))
                .append(RcDoc::intersperse(arg_docs, RcDoc::text(", ")))
                .append(RcDoc::text("]"))
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => format_scan(fmt, source, init, acc_name, val_name, body),
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => format_unfold(fmt, init, prev_name, curr_name, body),
        ExprKind::Match { scrutinee, arms } => format_match(fmt, scrutinee, arms),
        ExprKind::TupleMatch { scrutinees, arms } => format_tuple_match(fmt, scrutinees, arms),
        ExprKind::VariantLiteral { index, variant } => {
            RcDoc::text(format!("{}::{}", index.value, variant.value))
        }
    }
}

/// Operator precedence (higher = binds tighter).
const fn precedence(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne => 3,
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 4,
        BinOp::Add | BinOp::Sub => 5,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 6,
        BinOp::Pow => 7,
    }
}

const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => " + ",
        BinOp::Sub => " - ",
        BinOp::Mul => " * ",
        BinOp::Div => " / ",
        BinOp::Mod => " % ",
        BinOp::Pow => " ^ ",
        BinOp::Eq => " == ",
        BinOp::Ne => " != ",
        BinOp::Lt => " < ",
        BinOp::Gt => " > ",
        BinOp::Le => " <= ",
        BinOp::Ge => " >= ",
        BinOp::And => " && ",
        BinOp::Or => " || ",
    }
}

/// Format a child expression of a binary op, adding parens if needed to
/// preserve semantics.
fn format_binop_child(
    fmt: &mut Formatter<'_>,
    child: &Expr,
    parent_op: BinOp,
    is_right: bool,
) -> RcDoc<'static> {
    if let ExprKind::BinOp { op: child_op, .. } = &child.kind {
        let child_prec = precedence(*child_op);
        let parent_prec = precedence(parent_op);
        // Need parens if:
        // 1. child has lower precedence than parent, or
        // 2. child has same precedence and is on the "wrong" side for associativity
        //    (all left-associative except Pow which is right-associative)
        let needs_parens = child_prec < parent_prec
            || (child_prec == parent_prec && is_right && parent_op != BinOp::Pow)
            || (child_prec == parent_prec && !is_right && parent_op == BinOp::Pow);
        if needs_parens {
            return RcDoc::text("(")
                .append(format_expr(fmt, child))
                .append(RcDoc::text(")"));
        }
    }
    format_expr(fmt, child)
}

fn format_binop(fmt: &mut Formatter<'_>, op: BinOp, lhs: &Expr, rhs: &Expr) -> RcDoc<'static> {
    let lhs_doc = format_binop_child(fmt, lhs, op, false);
    // Drain any comment between lhs and rhs (e.g. `1.0 + // comment\n 2.0`)
    let comment = fmt.drain_comments_before(rhs.span.offset());
    let rhs_doc = format_binop_child(fmt, rhs, op, true);
    if is_nil(&comment) {
        lhs_doc.append(RcDoc::text(op_str(op))).append(rhs_doc)
    } else {
        // Force multi-line: put operator and comment on the lhs line,
        // then rhs on the next line
        lhs_doc
            .append(RcDoc::text(op_str(op)))
            .append(comment)
            .append(rhs_doc)
    }
}

/// Shared logic for `FnCall` and `QualifiedFnCall` with comment handling per argument.
pub fn format_fn_call_expr(
    fmt: &mut Formatter<'_>,
    fn_name: &str,
    type_args: &[graphcal_compiler::syntax::ast::GenericArg],
    args: &[Expr],
) -> RcDoc<'static> {
    let mut arg_docs: Vec<RcDoc<'static>> = Vec::new();
    for arg in args {
        // Drain leading comments before this argument
        let leading = fmt.drain_comments_before(arg.span.offset());
        let arg_doc = format_expr(fmt, arg);
        // Drain trailing comment after this argument
        let arg_end = arg.span.offset() + arg.span.len();
        let trailing = fmt.drain_trailing_comment(arg_end);
        arg_docs.push(prepend_comments(leading, arg_doc.append(trailing)));
    }
    let sep = RcDoc::text(",").append(RcDoc::line());
    let inner = RcDoc::intersperse(arg_docs, sep);
    let mut doc = RcDoc::text(fn_name.to_string());
    if !type_args.is_empty() {
        doc = doc.append(format_generic_args(type_args));
    }
    doc.append(RcDoc::text("("))
        .append(inner.nest(INDENT).group())
        .append(RcDoc::text(")"))
}

fn format_generic_args(type_args: &[graphcal_compiler::syntax::ast::GenericArg]) -> RcDoc<'static> {
    use graphcal_compiler::syntax::ast::GenericArg;
    let docs: Vec<RcDoc<'static>> = type_args
        .iter()
        .map(|arg| match arg {
            GenericArg::Type(te) => super::type_expr::format_type_expr_inline(te),
            GenericArg::Nat(ne) => RcDoc::text(format_nat_expr_str(ne)),
        })
        .collect();
    let sep = RcDoc::text(", ");
    RcDoc::text("<")
        .append(RcDoc::intersperse(docs, sep))
        .append(RcDoc::text(">"))
}

pub fn format_if(
    fmt: &mut Formatter<'_>,
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
) -> RcDoc<'static> {
    let cond = format_expr(fmt, condition);
    let then_doc = format_expr(fmt, then_branch);
    let else_doc = format_expr(fmt, else_branch);

    // Try single-line first via group
    let single_line = RcDoc::text("if ")
        .append(cond.clone())
        .append(RcDoc::text(" { "))
        .append(then_doc.clone())
        .append(RcDoc::text(" } else { "))
        .append(else_doc.clone())
        .append(RcDoc::text(" }"));

    let multi_line = RcDoc::text("if ")
        .append(cond)
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(then_doc).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("} else {"))
        .append(RcDoc::hardline().append(else_doc).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"));

    multi_line.flat_alt(single_line).group()
}

pub fn format_block_body(
    fmt: &mut Formatter<'_>,
    stmts: &[LetBinding],
    tail: &Expr,
) -> RcDoc<'static> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    for stmt in stmts {
        // Drain leading comments before this let binding
        let leading = fmt.drain_comments_before(stmt.span.offset());
        let stmt_doc = format_let_binding(fmt, stmt);
        // Drain trailing comment on the same line
        let stmt_end = stmt.span.offset() + stmt.span.len();
        let trailing = fmt.drain_trailing_comment(stmt_end);
        docs.push(prepend_comments(leading, stmt_doc.append(trailing)));
    }
    // Drain leading comments before the tail expression
    let leading = fmt.drain_comments_before(tail.span.offset());
    docs.push(prepend_comments(leading, format_expr(fmt, tail)));
    RcDoc::intersperse(docs, RcDoc::hardline())
}

pub fn format_let_binding(fmt: &mut Formatter<'_>, lb: &LetBinding) -> RcDoc<'static> {
    let mut doc = RcDoc::text("let ").append(RcDoc::text(lb.name.name.clone()));
    if let Some(ref ta) = lb.type_ann {
        doc = doc
            .append(RcDoc::text(": "))
            .append(format_type_expr_inline(ta));
    }
    doc.append(RcDoc::text(" = "))
        .append(format_expr(fmt, &lb.value))
        .append(RcDoc::text(";"))
}

pub fn format_struct_construction(
    fmt: &mut Formatter<'_>,
    type_name: &graphcal_compiler::syntax::names::Spanned<
        graphcal_compiler::syntax::names::StructTypeName,
    >,
    type_args: &[TypeExpr],
    fields: &[FieldInit],
) -> RcDoc<'static> {
    let mut header = RcDoc::text(type_name.value.as_str().to_string());
    if !type_args.is_empty() {
        let arg_docs: Vec<RcDoc<'static>> = type_args
            .iter()
            .map(|a| format_type_expr_inline(a))
            .collect();
        header = header
            .append(RcDoc::text("<"))
            .append(RcDoc::intersperse(arg_docs, RcDoc::text(", ")))
            .append(RcDoc::text(">"));
    }

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();
    for f in fields {
        // Drain leading comments before this field
        let leading = fmt.drain_comments_before(f.name.span.offset());
        let name = RcDoc::text(f.name.value.as_str().to_string());
        let field_doc = match &f.value {
            Some(val) => name.append(RcDoc::text(": ")).append(format_expr(fmt, val)),
            None => name, // shorthand
        };
        field_docs.push(prepend_comments(leading, field_doc));
    }

    let sep = RcDoc::text(",").append(RcDoc::line());
    let trailing_comma = RcDoc::text(",").flat_alt(RcDoc::nil());
    let inner = RcDoc::intersperse(field_docs, sep).append(trailing_comma);

    header
        .append(RcDoc::text(" {"))
        .append(RcDoc::line().append(inner).nest(INDENT).group())
        .append(RcDoc::line_())
        .append(RcDoc::text("}"))
        .group()
}

pub fn format_map_literal(fmt: &mut Formatter<'_>, entries: &[MapEntry]) -> RcDoc<'static> {
    let mut lines: Vec<RcDoc<'static>> = Vec::new();
    for e in entries {
        // Drain leading comments before this entry
        let leading = fmt.drain_comments_before(e.value.span.offset());

        let key_doc = if e.keys.len() == 1 {
            RcDoc::text(format!(
                "{}::{}",
                e.keys[0].index.value.as_str(),
                e.keys[0].variant.value.as_str()
            ))
        } else {
            let key_parts: Vec<String> = e
                .keys
                .iter()
                .map(|k| format!("{}::{}", k.index.value.as_str(), k.variant.value.as_str()))
                .collect();
            RcDoc::text(format!("({})", key_parts.join(", ")))
        };
        let entry_doc = key_doc
            .append(RcDoc::text(": "))
            .append(format_expr(fmt, &e.value))
            .append(RcDoc::text(","));
        // Drain trailing comment after this entry's value (but before next entry)
        let value_end = e.value.span.offset() + e.value.span.len();
        let trailing = fmt.drain_trailing_comment(value_end);
        lines.push(prepend_comments(leading, entry_doc.append(trailing)));
    }

    RcDoc::text("{")
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(lines, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// Format a table literal expression: `table[Index1, Index2] { ... }`
///
/// Handles 1D, 2D, and 3D+ tables with column-aligned output.
pub fn format_table_literal(
    fmt: &mut Formatter<'_>,
    indexes: &[Spanned<IndexName>],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();

    // Build the `table[Index1, Index2]` header
    let idx_names: Vec<String> = indexes
        .iter()
        .map(|i| i.value.as_str().to_string())
        .collect();
    let header = format!("table[{}]", idx_names.join(", "));

    if ndim == 1 {
        format_table_1d(fmt, &header, entries)
    } else if ndim == 2 {
        format_table_2d(fmt, &header, indexes, entries)
    } else {
        format_table_sliced(fmt, &header, indexes, entries)
    }
}

/// Format a 1D table: `table[Maneuver] { Label: expr; ... }`
fn format_table_1d(fmt: &mut Formatter<'_>, header: &str, entries: &[MapEntry]) -> RcDoc<'static> {
    // Compute max label width for alignment
    let max_label_width = entries
        .iter()
        .map(|e| e.keys[0].variant.value.as_str().len())
        .max()
        .unwrap_or(0);

    // Render each cell value to a string for width computation
    let rendered_values: Vec<String> = entries
        .iter()
        .map(|e| render_doc_to_string(&format_expr(fmt, &e.value)))
        .collect();

    // Compute max value width for right-alignment
    let max_value_width = rendered_values
        .iter()
        .map(std::string::String::len)
        .max()
        .unwrap_or(0);

    let mut rows: Vec<RcDoc<'static>> = Vec::new();
    for (e, rendered) in entries.iter().zip(&rendered_values) {
        // Drain leading comments before this entry (use value span since
        // key spans may point to the index declaration, not the table row)
        let leading = fmt.drain_comments_before(e.value.span.offset());

        let label = e.keys[0].variant.value.as_str();
        let padding = max_label_width - label.len();
        let value_padding = max_value_width - rendered.len();
        let row_text = format!(
            "{}:{} {};",
            label,
            " ".repeat(padding + 1 + value_padding),
            rendered
        );

        // Drain trailing comment on the same line after this entry's value
        let value_end = e.value.span.offset() + e.value.span.len();
        let trailing = fmt.drain_trailing_comment(value_end);

        // Prepend leading comments (they already end with hardline)
        let row_doc = prepend_comments(leading, RcDoc::text(row_text).append(trailing));
        rows.push(row_doc);
    }

    RcDoc::text(format!("{header} {{"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(rows, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// Format a 2D table: `table[Phase, Maneuver] { ColLabel, ...; RowLabel: val, ...; ... }`
fn format_table_2d(
    fmt: &mut Formatter<'_>,
    header: &str,
    indexes: &[Spanned<IndexName>],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let body = format_table_2d_body(fmt, indexes, entries);

    RcDoc::text(format!("{header} {{"))
        .append(RcDoc::hardline().append(body).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// Format the inner body of a 2D table (header row + data rows).
/// Shared between 2D tables and 3D+ slice sections.
fn format_table_2d_body(
    fmt: &mut Formatter<'_>,
    indexes: &[Spanned<IndexName>],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();
    // Row index is second-to-last, column index is last
    let col_idx = ndim - 1;

    // Extract unique column labels (from the last key, preserving order)
    let mut col_labels: Vec<String> = Vec::new();
    for e in entries {
        let col_label = e.keys[col_idx].variant.value.as_str().to_string();
        if !col_labels.contains(&col_label) {
            col_labels.push(col_label);
        }
    }
    let num_cols = col_labels.len();

    // Extract unique row labels (from the second-to-last key, preserving order)
    let row_idx = ndim - 2;
    let mut row_labels: Vec<String> = Vec::new();
    for e in entries {
        let row_label = e.keys[row_idx].variant.value.as_str().to_string();
        if !row_labels.contains(&row_label) {
            row_labels.push(row_label);
        }
    }

    // Build 2D grid of rendered values and track entry indices per cell
    let mut grid: Vec<Vec<String>> = vec![vec![String::new(); num_cols]; row_labels.len()];
    let mut entry_indices: Vec<Vec<Option<usize>>> = vec![vec![None; num_cols]; row_labels.len()];
    for (ei, e) in entries.iter().enumerate() {
        let row_label = e.keys[row_idx].variant.value.as_str();
        let col_label = e.keys[col_idx].variant.value.as_str();
        let ri = row_labels.iter().position(|r| r == row_label).unwrap_or(0);
        let ci = col_labels.iter().position(|c| c == col_label).unwrap_or(0);
        grid[ri][ci] = render_doc_to_string(&format_expr(fmt, &e.value));
        entry_indices[ri][ci] = Some(ei);
    }

    // Compute column widths: max of (column label width, max cell width in that column)
    let col_widths: Vec<usize> = (0..num_cols)
        .map(|ci| {
            let label_width = col_labels[ci].len();
            let max_cell = grid.iter().map(|row| row[ci].len()).max().unwrap_or(0);
            label_width.max(max_cell)
        })
        .collect();

    // Compute max row label width
    let max_row_label_width = row_labels
        .iter()
        .map(std::string::String::len)
        .max()
        .unwrap_or(0);

    // Build header row: right-aligned column labels, indented to account for row label column
    let row_label_prefix_width = max_row_label_width + 2; // "Label: " minus the space that is part of the value
    let header_cells: Vec<String> = col_labels
        .iter()
        .enumerate()
        .map(|(ci, label)| format!("{:>width$}", label, width = col_widths[ci]))
        .collect();
    let header_line = format!(
        "{}{};",
        " ".repeat(row_label_prefix_width),
        header_cells.join(", ")
    );

    // Build data rows
    let mut all_rows: Vec<RcDoc<'static>> = Vec::new();
    all_rows.push(RcDoc::text(header_line));

    for (ri, row_label) in row_labels.iter().enumerate() {
        // Drain leading comments before this row (use first entry's value span)
        let first_entry_idx = entry_indices[ri].iter().find_map(|idx| *idx);
        let leading = first_entry_idx.map_or_else(RcDoc::nil, |ei| {
            fmt.drain_comments_before(entries[ei].value.span.offset())
        });

        let label_padding = max_row_label_width - row_label.len();
        let cells: Vec<String> = (0..num_cols)
            .map(|ci| format!("{:>width$}", grid[ri][ci], width = col_widths[ci]))
            .collect();
        let row_line = format!(
            "{}:{} {};",
            row_label,
            " ".repeat(label_padding),
            cells.join(", ")
        );

        // Drain trailing comment from last entry in this row
        let last_entry_idx = entry_indices[ri].iter().rev().find_map(|idx| *idx);
        let trailing = last_entry_idx.map_or_else(RcDoc::nil, |ei| {
            let value_end = entries[ei].value.span.offset() + entries[ei].value.span.len();
            fmt.drain_trailing_comment(value_end)
        });

        let row_doc = prepend_comments(leading, RcDoc::text(row_line).append(trailing));
        all_rows.push(row_doc);
    }

    RcDoc::intersperse(all_rows, RcDoc::hardline())
}

/// Format a 3D+ table with slice sections.
fn format_table_sliced(
    fmt: &mut Formatter<'_>,
    header: &str,
    indexes: &[Spanned<IndexName>],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();
    let slice_dims = ndim - 2;

    // Group entries by their slice keys (first N-2 keys)
    let mut slices: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
    for (idx, e) in entries.iter().enumerate() {
        let slice_key: Vec<String> = (0..slice_dims)
            .map(|i| {
                format!(
                    "{}::{}",
                    e.keys[i].index.value.as_str(),
                    e.keys[i].variant.value.as_str()
                )
            })
            .collect();

        if let Some(last) = slices.last_mut()
            && last.1 == slice_key
        {
            last.0.push(idx);
            continue;
        }
        slices.push((vec![idx], slice_key));
    }

    // Build each slice doc and nest it for indentation.
    let mut slice_docs: Vec<RcDoc<'static>> = Vec::new();
    for (entry_indices, slice_key) in &slices {
        let slice_header = format!("[{}]", slice_key.join(", "));

        // Drain leading comments before this slice header
        let first_idx = entry_indices[0];
        let first_key_offset = entries[first_idx].keys[0].index.span.offset();
        let leading = fmt.drain_comments_before(first_key_offset);

        // Drain trailing comment on the same line as the slice header "]"
        let last_slice_key = &entries[first_idx].keys[slice_dims - 1];
        let header_end = last_slice_key.variant.span.offset() + last_slice_key.variant.span.len();
        let trailing = fmt.drain_trailing_comment(header_end);

        let slice_entries: Vec<MapEntry> = entry_indices
            .iter()
            .map(|&idx| entries[idx].clone())
            .collect();
        slice_docs.push(prepend_comments(
            leading,
            RcDoc::text(slice_header)
                .append(trailing)
                .append(RcDoc::hardline())
                .append(format_table_2d_body(fmt, indexes, &slice_entries)),
        ));
    }

    // Join slices: each slice is indented, separated by a blank line (no trailing whitespace).
    let mut body = RcDoc::nil();
    for (i, slice_doc) in slice_docs.into_iter().enumerate() {
        if i > 0 {
            // End previous slice's indentation, emit un-nested blank line, start new indented slice
            body = body.append(RcDoc::hardline());
        }
        body = body.append(RcDoc::hardline().append(slice_doc).nest(INDENT));
    }

    RcDoc::text(format!("{header} {{"))
        .append(body)
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

pub fn format_for_comp(
    fmt: &mut Formatter<'_>,
    bindings: &[ForBinding],
    body: &Expr,
) -> RcDoc<'static> {
    let binding_docs: Vec<RcDoc<'static>> = bindings
        .iter()
        .map(|b| {
            RcDoc::text(b.var.name.clone())
                .append(RcDoc::text(": "))
                .append(match &b.index {
                    graphcal_compiler::syntax::ast::ForBindingIndex::Named(spanned) => {
                        RcDoc::text(spanned.value.as_str().to_string())
                    }
                    graphcal_compiler::syntax::ast::ForBindingIndex::Range { arg, .. } => {
                        let arg_str = format_nat_expr_str(arg);
                        RcDoc::text(format!("range({arg_str})"))
                    }
                })
        })
        .collect();
    let bindings_doc = RcDoc::intersperse(binding_docs, RcDoc::text(", "));

    // Drain leading comments before the body expression
    let leading = fmt.drain_comments_before(body.span.offset());
    let body_doc = format_expr(fmt, body);
    let body_doc = prepend_comments(leading, body_doc);

    let single_line = RcDoc::text("for ")
        .append(bindings_doc.clone())
        .append(RcDoc::text(" { "))
        .append(body_doc.clone())
        .append(RcDoc::text(" }"));

    let multi_line = RcDoc::text("for ")
        .append(bindings_doc)
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(body_doc).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"));

    multi_line.flat_alt(single_line).group()
}

fn format_scan(
    fmt: &mut Formatter<'_>,
    source: &Expr,
    init: &Expr,
    acc_name: &Ident,
    val_name: &Ident,
    body: &Expr,
) -> RcDoc<'static> {
    RcDoc::text("scan(")
        .append(format_expr(fmt, source))
        .append(RcDoc::text(", "))
        .append(format_expr(fmt, init))
        .append(RcDoc::text(", |"))
        .append(RcDoc::text(acc_name.name.clone()))
        .append(RcDoc::text(", "))
        .append(RcDoc::text(val_name.name.clone()))
        .append(RcDoc::text("| "))
        .append(format_expr(fmt, body))
        .append(RcDoc::text(")"))
}

fn format_unfold(
    fmt: &mut Formatter<'_>,
    init: &Expr,
    prev_name: &Ident,
    curr_name: &Ident,
    body: &Expr,
) -> RcDoc<'static> {
    RcDoc::text("unfold(")
        .append(format_expr(fmt, init))
        .append(RcDoc::text(", |"))
        .append(RcDoc::text(prev_name.name.clone()))
        .append(RcDoc::text(", "))
        .append(RcDoc::text(curr_name.name.clone()))
        .append(RcDoc::text("| "))
        .append(format_expr(fmt, body))
        .append(RcDoc::text(")"))
}

pub fn format_match(
    fmt: &mut Formatter<'_>,
    scrutinee: &Expr,
    arms: &[MatchArm],
) -> RcDoc<'static> {
    let mut arm_docs: Vec<RcDoc<'static>> = Vec::new();
    for arm in arms {
        // Drain leading comments before this arm
        let leading = fmt.drain_comments_before(arm.span.offset());
        let pattern = format_match_pattern(&arm.pattern);
        let body = format_expr(fmt, &arm.body);
        let arm_doc = pattern
            .append(RcDoc::text(" => "))
            .append(body)
            .append(RcDoc::text(","));
        // Drain trailing comment after this arm
        let arm_end = arm.span.offset() + arm.span.len();
        let trailing = fmt.drain_trailing_comment(arm_end);
        arm_docs.push(prepend_comments(leading, arm_doc.append(trailing)));
    }

    RcDoc::text("match ")
        .append(format_expr(fmt, scrutinee))
        .append(RcDoc::text(" {"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(arm_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

pub fn format_match_pattern(p: &MatchPattern) -> RcDoc<'static> {
    let name = p.qualified_index.as_ref().map_or_else(
        || RcDoc::text(p.variant_name.value.as_str().to_string()),
        |index| RcDoc::text(format!("{}::{}", index.value, p.variant_name.value)),
    );
    if p.bindings.is_empty() {
        return name;
    }
    let binding_docs: Vec<RcDoc<'static>> = p
        .bindings
        .iter()
        .map(|b| match b {
            PatternBinding::Bind { field, var } => {
                if field.value.as_str() == var.name {
                    RcDoc::text(var.name.clone())
                } else {
                    RcDoc::text(field.value.as_str().to_string())
                        .append(RcDoc::text(": "))
                        .append(RcDoc::text(var.name.clone()))
                }
            }
            PatternBinding::Wildcard { field, .. } => {
                RcDoc::text(field.value.as_str().to_string()).append(RcDoc::text(": _"))
            }
        })
        .collect();
    name.append(RcDoc::text(" { "))
        .append(RcDoc::intersperse(binding_docs, RcDoc::text(", ")))
        .append(RcDoc::text(" }"))
}

pub fn format_tuple_match(
    fmt: &mut Formatter<'_>,
    scrutinees: &[Expr],
    arms: &[TupleMatchArm],
) -> RcDoc<'static> {
    // Format scrutinees: `match (a, b)`
    let scrutinee_docs: Vec<RcDoc<'static>> =
        scrutinees.iter().map(|s| format_expr(fmt, s)).collect();
    let scrutinee_list = RcDoc::intersperse(scrutinee_docs, RcDoc::text(", "));

    let mut arm_docs: Vec<RcDoc<'static>> = Vec::new();
    for arm in arms {
        let leading = fmt.drain_comments_before(arm.span.offset());
        let pattern_doc = arm.patterns.as_ref().map_or_else(
            || RcDoc::text("_"),
            |patterns| {
                let pat_docs: Vec<RcDoc<'static>> =
                    patterns.iter().map(|p| format_expr(fmt, p)).collect();
                RcDoc::text("(")
                    .append(RcDoc::intersperse(pat_docs, RcDoc::text(", ")))
                    .append(RcDoc::text(")"))
            },
        );
        let body = format_expr(fmt, &arm.body);
        let arm_doc = pattern_doc
            .append(RcDoc::text(" => "))
            .append(body)
            .append(RcDoc::text(","));
        let arm_end = arm.span.offset() + arm.span.len();
        let trailing = fmt.drain_trailing_comment(arm_end);
        arm_docs.push(prepend_comments(leading, arm_doc.append(trailing)));
    }

    RcDoc::text("match (")
        .append(scrutinee_list)
        .append(RcDoc::text(") {"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(arm_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}
