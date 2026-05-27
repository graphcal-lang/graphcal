use graphcal_compiler::syntax::ast::{
    BinOp, Expr, ExprKind, FieldInit, ForBinding, IndexArg, MapEntry, MatchArm, MatchPattern,
    ModulePath, ParamBinding, PatternBinding, TableIndexSpec, TupleMatchArm, UnaryOp,
};
use graphcal_compiler::syntax::names::{LocalName, ScopedName};
use graphcal_compiler::syntax::span::Spanned;
use pretty::RcDoc;

use super::{
    Formatter, INDENT, flat_alt_group, format_unit_expr_inline, prepend_comments,
    render_doc_to_string,
};

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
        ExprKind::TypeSystemRef(name) => RcDoc::text(name.value.as_str().to_string()),
        ExprKind::GraphRef(name) => RcDoc::text(format!("@{}", format_scoped_surface(&name.value))),
        ExprKind::InlineDagRef { path, args, output } => {
            format_inline_dag_ref(fmt, path, args, output.value.as_str())
        }
        ExprKind::ConstRef(name) => RcDoc::text(format_scoped_surface(&name.value)),
        ExprKind::LocalRef(ident) => RcDoc::text(ident.name.clone()),
        ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(path)) => {
            RcDoc::text(
                path.segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
            )
        }
        ExprKind::BinOp { op, lhs, rhs } => format_binop(fmt, *op, lhs, rhs),
        ExprKind::UnaryOp { op, operand } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            RcDoc::text(op_str).append(format_unary_operand(fmt, operand))
        }
        ExprKind::FnCall {
            name,
            type_args,
            args,
        } => format_fn_call_expr(fmt, name.value.as_str(), type_args, args),
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
        ExprKind::FieldAccess { expr: inner, field } => format_expr(fmt, inner)
            .append(RcDoc::text("."))
            .append(RcDoc::text(field.value.as_str().to_string())),
        ExprKind::ConstructorCall {
            constructor,
            generic_args,
            fields,
        } => format_constructor_call(fmt, constructor, generic_args, fields),
        ExprKind::MapLiteral { entries } => format_map_literal(fmt, entries),
        ExprKind::Sugar(graphcal_compiler::syntax::ast::RawExprSugar::TableLiteral {
            indexes,
            entries,
        }) => format_table_literal(fmt, indexes, entries),
        ExprKind::ForComp { bindings, body } => format_for_comp(fmt, bindings, body),
        ExprKind::IndexAccess { expr: inner, args } => {
            let arg_docs: Vec<RcDoc<'static>> = args
                .iter()
                .map(|a| match a {
                    IndexArg::Variant { index, variant } => RcDoc::text(format!(
                        "{}.{}",
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
        ExprKind::TupleMatch { scrutinees, arms } => {
            format_tuple_match(fmt, scrutinees.as_slice(), arms.as_slice())
        }
        ExprKind::VariantLiteral { index, variant } => {
            RcDoc::text(format!("{}.{}", index.value, variant.value))
        }
    }
}

/// Render a [`ScopedName`] in surface syntax.
fn format_scoped_surface(scoped: &ScopedName) -> String {
    scoped.to_string()
}

/// Operator precedence (higher = binds tighter).
///
/// Unary `!`/`-` sits at [`UNARY_PREC`] (7). `^` is intentionally above the
/// unary level so paren-elision around `(-x) ^ 2` stays semantics-preserving:
/// without the parens, `-x ^ 2` reparses as `-(x^2)` (issue #575).
const fn precedence(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne => 3,
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 4,
        BinOp::Add | BinOp::Sub => 5,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 6,
        BinOp::Pow => 8,
    }
}

/// Precedence of unary `!` and `-`. Sits between the multiplicative operators
/// and `^` (per `docs/language/expressions.md`).
const UNARY_PREC: u8 = 7;

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
    let parent_prec = precedence(parent_op);
    let needs_parens = match &child.kind {
        ExprKind::BinOp { op: child_op, .. } => {
            let child_prec = precedence(*child_op);
            // Lower precedence wraps; equal-precedence wraps on the
            // "wrong" associativity side (all left-associative except `^`).
            child_prec < parent_prec
                || (child_prec == parent_prec && is_right && parent_op != BinOp::Pow)
                || (child_prec == parent_prec && !is_right && parent_op == BinOp::Pow)
        }
        // A bare unary on the lhs of a higher-binding operator must be
        // parenthesized: stripping `(-x) ^ 2` to `-x ^ 2` reparses as
        // `-(x^2)` because `^` binds tighter than unary `-` (issue #575).
        // Symmetric on the rhs would be redundant — `^` is right-assoc, so
        // `x ^ -2` is unambiguous and parses as intended.
        ExprKind::UnaryOp { .. } if !is_right && parent_prec > UNARY_PREC => true,
        _ => false,
    };
    if needs_parens {
        return RcDoc::text("(")
            .append(format_expr(fmt, child))
            .append(RcDoc::text(")"));
    }
    format_expr(fmt, child)
}

/// Format the operand of a unary `!`/`-`, adding parens if the operand is a
/// binary expression that binds looser than the unary itself.
///
/// Without this, `!(a && b)` collapses to `!a && b` — which reparses as
/// `(!a) && b` because `!` binds tighter than `&&` (issue #575).
fn format_unary_operand(fmt: &mut Formatter<'_>, operand: &Expr) -> RcDoc<'static> {
    if let ExprKind::BinOp { op: child_op, .. } = &operand.kind
        && precedence(*child_op) < UNARY_PREC
    {
        return RcDoc::text("(")
            .append(format_expr(fmt, operand))
            .append(RcDoc::text(")"));
    }
    format_expr(fmt, operand)
}

fn format_binop(fmt: &mut Formatter<'_>, op: BinOp, lhs: &Expr, rhs: &Expr) -> RcDoc<'static> {
    let lhs_doc = format_binop_child(fmt, lhs, op, false);
    // Drain any comment between lhs and rhs (e.g. `1.0 + // comment\n 2.0`)
    let comment = fmt.drain_comments_before(rhs.span.offset());
    let rhs_doc = format_binop_child(fmt, rhs, op, true);
    match comment {
        None => lhs_doc.append(RcDoc::text(op_str(op))).append(rhs_doc),
        Some(comment) => {
            // Force multi-line: put operator and comment on the lhs line,
            // then rhs on the next line
            lhs_doc
                .append(RcDoc::text(op_str(op)))
                .append(comment)
                .append(rhs_doc)
        }
    }
}

/// Format a `FnCall` expression with comment handling per argument.
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
        let trailing = fmt
            .drain_trailing_comment(arg_end)
            .unwrap_or_else(RcDoc::nil);
        arg_docs.push(prepend_comments(leading, arg_doc.append(trailing)));
    }
    let sep = RcDoc::text(",").append(RcDoc::line());
    let inner = RcDoc::intersperse(arg_docs, sep);
    let mut doc = RcDoc::text(fn_name.to_string());
    if !type_args.is_empty() {
        doc = doc.append(format_generic_args(fmt, type_args));
    }
    doc.append(RcDoc::text("("))
        .append(inner.nest(INDENT).group())
        .append(RcDoc::text(")"))
}

fn format_generic_args(
    fmt: &mut Formatter<'_>,
    type_args: &[graphcal_compiler::syntax::ast::GenericArg],
) -> RcDoc<'static> {
    use graphcal_compiler::syntax::ast::GenericArg;
    let docs: Vec<RcDoc<'static>> = type_args
        .iter()
        .map(|arg| match arg {
            GenericArg::Type(te) => super::type_expr::format_type_expr_inline(fmt, te),
            GenericArg::Nat(ne) => RcDoc::text(ne.to_string()),
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

    flat_alt_group(single_line, multi_line)
}

pub fn format_constructor_call(
    fmt: &mut Formatter<'_>,
    constructor: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::names::ConstructorName,
    >,
    generic_args: &[graphcal_compiler::syntax::ast::GenericArg],
    fields: &[FieldInit],
) -> RcDoc<'static> {
    let mut header = RcDoc::text(constructor.value.as_str().to_string());
    if !generic_args.is_empty() {
        header = header.append(format_generic_args(fmt, generic_args));
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

    // Construction is always a constructor call — parens with named
    // args. There is no brace-form construction; the parser rejects
    // `Ctor { field: val }` outright.
    header
        .append(RcDoc::text("("))
        .append(RcDoc::line_().append(inner).nest(INDENT).group())
        .append(RcDoc::line_())
        .append(RcDoc::text(")"))
        .group()
}

pub fn format_map_literal(fmt: &mut Formatter<'_>, entries: &[MapEntry]) -> RcDoc<'static> {
    let mut lines: Vec<RcDoc<'static>> = Vec::new();
    for e in entries {
        // Drain leading comments before this entry
        let leading = fmt.drain_comments_before(e.value.span.offset());

        let key_doc = if e.keys.len() == 1 {
            RcDoc::text(format!(
                "{}.{}",
                e.keys[0].index.value,
                e.keys[0].variant.value.as_str()
            ))
        } else {
            let key_parts: Vec<String> = e
                .keys
                .iter()
                .map(|k| format!("{}.{}", k.index.value, k.variant.value.as_str()))
                .collect();
            RcDoc::text(format!("({})", key_parts.join(", ")))
        };
        let entry_doc = key_doc
            .append(RcDoc::text(": "))
            .append(format_expr(fmt, &e.value))
            .append(RcDoc::text(","));
        // Drain trailing comment after this entry's value (but before next entry)
        let value_end = e.value.span.offset() + e.value.span.len();
        let trailing = fmt
            .drain_trailing_comment(value_end)
            .unwrap_or_else(RcDoc::nil);
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
    indexes: &[TableIndexSpec],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();

    // Build the `table[Index1, Index2]` header
    let idx_names: Vec<String> = indexes
        .iter()
        .map(|i| match i {
            TableIndexSpec::Named(s) => s.value.as_str().to_string(),
            TableIndexSpec::NatRange(n, _) => n.to_string(),
        })
        .collect();
    let header = format!("table[{}]", idx_names.join(", "));

    if ndim == 1 {
        format_table_1d(fmt, &header, &indexes[0], entries)
    } else if ndim == 2 {
        format_table_2d(fmt, &header, indexes, entries)
    } else {
        format_table_sliced(fmt, &header, indexes, entries)
    }
}

/// Format a 1D table: `table[Maneuver] { Label: expr; ... }` or
/// `table[3] { expr; ... }` for Nat range indexes.
fn format_table_1d(
    fmt: &mut Formatter<'_>,
    header: &str,
    index: &TableIndexSpec,
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let nat_range = index.is_nat_range();

    // Compute max label width for alignment (unused for NatRange)
    let max_label_width = if nat_range {
        0
    } else {
        entries
            .iter()
            .map(|e| e.keys[0].variant.value.as_str().len())
            .max()
            .unwrap_or(0)
    };

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

        let value_padding = max_value_width - rendered.len();
        let row_text = if nat_range {
            format!("{}{};", " ".repeat(value_padding), rendered)
        } else {
            let label = e.keys[0].variant.value.as_str();
            let padding = max_label_width - label.len();
            format!(
                "{}:{} {};",
                label,
                " ".repeat(padding + 1 + value_padding),
                rendered
            )
        };

        // Drain trailing comment on the same line after this entry's value
        let value_end = e.value.span.offset() + e.value.span.len();
        let trailing = fmt
            .drain_trailing_comment(value_end)
            .unwrap_or_else(RcDoc::nil);

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
    indexes: &[TableIndexSpec],
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
    indexes: &[TableIndexSpec],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();
    // Row index is second-to-last, column index is last
    let col_idx = ndim - 1;
    let row_idx = ndim - 2;
    let row_is_nat = indexes[row_idx].is_nat_range();
    let col_is_nat = indexes[col_idx].is_nat_range();

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
        // Labels were built from the same entries, so lookup cannot miss.
        // If it somehow does, skip this entry rather than silently using row/col 0.
        let Some(ri) = row_labels.iter().position(|r| r == row_label) else {
            continue;
        };
        let Some(ci) = col_labels.iter().position(|c| c == col_label) else {
            continue;
        };
        grid[ri][ci] = render_doc_to_string(&format_expr(fmt, &e.value));
        entry_indices[ri][ci] = Some(ei);
    }

    // Compute column widths: max of (column label width, max cell width in that column)
    // When the column axis is a NatRange, no header row is emitted, so column labels
    // do not contribute to width.
    let col_widths: Vec<usize> = (0..num_cols)
        .map(|ci| {
            let label_width = if col_is_nat { 0 } else { col_labels[ci].len() };
            let max_cell = grid.iter().map(|row| row[ci].len()).max().unwrap_or(0);
            label_width.max(max_cell)
        })
        .collect();

    // Compute max row label width (0 when row axis is NatRange — no labels emitted)
    let max_row_label_width = if row_is_nat {
        0
    } else {
        row_labels
            .iter()
            .map(std::string::String::len)
            .max()
            .unwrap_or(0)
    };

    // Build the header row only when the column axis is named.
    // Header format: `: Col1, Col2, ...;` aligned to the row-label column.
    let mut all_rows: Vec<RcDoc<'static>> = Vec::new();
    if !col_is_nat {
        let header_cells: Vec<String> = col_labels
            .iter()
            .enumerate()
            .map(|(ci, label)| format!("{:>width$}", label, width = col_widths[ci]))
            .collect();
        let header_line = if row_is_nat {
            // No row labels — just `: Col1, Col2, ...;` at the row start.
            format!(": {};", header_cells.join(", "))
        } else {
            // Pad so `:` lines up with the data-row colons.
            let row_label_prefix_width = max_row_label_width;
            format!(
                "{}: {};",
                " ".repeat(row_label_prefix_width),
                header_cells.join(", ")
            )
        };
        all_rows.push(RcDoc::text(header_line));
    }

    for (ri, row_label) in row_labels.iter().enumerate() {
        // Drain leading comments before this row (use first entry's value span)
        let first_entry_idx = entry_indices[ri].iter().find_map(|idx| *idx);
        let leading = first_entry_idx
            .and_then(|ei| fmt.drain_comments_before(entries[ei].value.span.offset()));

        let cells: Vec<String> = (0..num_cols)
            .map(|ci| format!("{:>width$}", grid[ri][ci], width = col_widths[ci]))
            .collect();
        let row_line = if row_is_nat {
            format!("{};", cells.join(", "))
        } else {
            let label_padding = max_row_label_width - row_label.len();
            format!(
                "{}:{} {};",
                row_label,
                " ".repeat(label_padding),
                cells.join(", ")
            )
        };

        // Drain trailing comment from last entry in this row
        let last_entry_idx = entry_indices[ri].iter().rev().find_map(|idx| *idx);
        let trailing = last_entry_idx
            .and_then(|ei| {
                let value_end = entries[ei].value.span.offset() + entries[ei].value.span.len();
                fmt.drain_trailing_comment(value_end)
            })
            .unwrap_or_else(RcDoc::nil);

        let row_doc = prepend_comments(leading, RcDoc::text(row_line).append(trailing));
        all_rows.push(row_doc);
    }

    RcDoc::intersperse(all_rows, RcDoc::hardline())
}

/// Format a 3D+ table with slice sections.
fn format_table_sliced(
    fmt: &mut Formatter<'_>,
    header: &str,
    indexes: &[TableIndexSpec],
    entries: &[MapEntry],
) -> RcDoc<'static> {
    let ndim = indexes.len();
    let slice_dims = ndim - 2;

    // Group entries by their slice keys (first N-2 keys).
    // Named axes render as `Index.Variant`; NatRange axes render as `#N`
    // (the variant name is already the synthetic `#N` form).
    let mut slices: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
    for (idx, e) in entries.iter().enumerate() {
        let slice_key: Vec<String> = (0..slice_dims)
            .map(|i| match &indexes[i] {
                TableIndexSpec::Named(_) => format!(
                    "{}.{}",
                    e.keys[i].index.value,
                    e.keys[i].variant.value.as_str()
                ),
                TableIndexSpec::NatRange(_, _) => e.keys[i].variant.value.as_str().to_string(),
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
        let trailing = fmt
            .drain_trailing_comment(header_end)
            .unwrap_or_else(RcDoc::nil);

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
            RcDoc::text(b.var.value.as_str().to_owned())
                .append(RcDoc::text(": "))
                .append(match &b.index {
                    graphcal_compiler::syntax::ast::ForBindingIndex::Named(spanned) => {
                        RcDoc::text(spanned.value.as_str().to_string())
                    }
                    graphcal_compiler::syntax::ast::ForBindingIndex::Range { arg, .. } => {
                        let arg_str = arg.to_string();
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

    flat_alt_group(single_line, multi_line)
}

/// Format a call of the shape `head(<args>, |p1, p2| <body>)`.
///
/// `scan`, `unfold`, and similar builtins all follow this pattern — a fixed
/// `head`, a list of positional argument expressions, and a closing lambda
/// over two identifiers whose body is another expression.
fn format_lambda_call(
    fmt: &mut Formatter<'_>,
    head: &'static str,
    args: &[&Expr],
    lambda_params: (&Spanned<LocalName>, &Spanned<LocalName>),
    body: &Expr,
) -> RcDoc<'static> {
    let mut doc = RcDoc::text(head).append(RcDoc::text("("));
    for arg in args {
        doc = doc.append(format_expr(fmt, arg)).append(RcDoc::text(", "));
    }
    doc.append(RcDoc::text("|"))
        .append(RcDoc::text(lambda_params.0.value.as_str().to_owned()))
        .append(RcDoc::text(", "))
        .append(RcDoc::text(lambda_params.1.value.as_str().to_owned()))
        .append(RcDoc::text("| "))
        .append(format_expr(fmt, body))
        .append(RcDoc::text(")"))
}

fn format_scan(
    fmt: &mut Formatter<'_>,
    source: &Expr,
    init: &Expr,
    acc_name: &Spanned<LocalName>,
    val_name: &Spanned<LocalName>,
    body: &Expr,
) -> RcDoc<'static> {
    format_lambda_call(fmt, "scan", &[source, init], (acc_name, val_name), body)
}

fn format_unfold(
    fmt: &mut Formatter<'_>,
    init: &Expr,
    prev_name: &Spanned<LocalName>,
    curr_name: &Spanned<LocalName>,
    body: &Expr,
) -> RcDoc<'static> {
    format_lambda_call(fmt, "unfold", &[init], (prev_name, curr_name), body)
}

pub fn format_match(
    fmt: &mut Formatter<'_>,
    scrutinee: &Expr,
    arms: &[MatchArm],
) -> RcDoc<'static> {
    let arm_docs = collect_arm_docs(fmt, arms.iter().map(|arm| (arm.span, &arm.body)), |i| {
        format_match_pattern(&arms[i].pattern)
    });
    wrap_match_block(
        RcDoc::text("match ").append(format_expr(fmt, scrutinee)),
        arm_docs,
    )
}

pub fn format_match_pattern(p: &MatchPattern) -> RcDoc<'static> {
    let name = p.qualified_index.as_ref().map_or_else(
        || RcDoc::text(p.variant_name.value.as_str().to_string()),
        |index| RcDoc::text(format!("{}.{}", index.value, p.variant_name.value)),
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
    name.append(RcDoc::text("("))
        .append(RcDoc::intersperse(binding_docs, RcDoc::text(", ")))
        .append(RcDoc::text(")"))
}

pub fn format_tuple_match(
    fmt: &mut Formatter<'_>,
    scrutinees: &[Expr],
    arms: &[TupleMatchArm],
) -> RcDoc<'static> {
    let scrutinee_docs: Vec<RcDoc<'static>> =
        scrutinees.iter().map(|s| format_expr(fmt, s)).collect();
    let scrutinee_list = RcDoc::intersperse(scrutinee_docs, RcDoc::text(", "));

    // Pattern formatting needs `fmt` (recurses into format_expr), so collect
    // the pattern docs upfront before handing arm spans to the shared helper.
    let pattern_docs: Vec<RcDoc<'static>> = arms
        .iter()
        .map(|arm| {
            arm.patterns.as_ref().map_or_else(
                || RcDoc::text("_"),
                |patterns| {
                    let pat_docs: Vec<RcDoc<'static>> =
                        patterns.iter().map(|p| format_expr(fmt, p)).collect();
                    RcDoc::text("(")
                        .append(RcDoc::intersperse(pat_docs, RcDoc::text(", ")))
                        .append(RcDoc::text(")"))
                },
            )
        })
        .collect();

    let arm_docs = collect_arm_docs(fmt, arms.iter().map(|arm| (arm.span, &arm.body)), |i| {
        pattern_docs[i].clone()
    });

    wrap_match_block(
        RcDoc::text("match (")
            .append(scrutinee_list)
            .append(RcDoc::text(")")),
        arm_docs,
    )
}

/// Build per-arm docs for both `format_match` and `format_tuple_match`. Walks
/// each arm's span, drains leading and trailing comments, formats the body
/// expression, and assembles `pattern => body,` with comments preserved.
///
/// `pattern_for` returns the pre-formatted pattern doc for arm index `i`.
fn collect_arm_docs<'a>(
    fmt: &mut Formatter<'_>,
    arms: impl Iterator<Item = (graphcal_compiler::syntax::span::Span, &'a Expr)>,
    pattern_for: impl Fn(usize) -> RcDoc<'static>,
) -> Vec<RcDoc<'static>> {
    let mut arm_docs: Vec<RcDoc<'static>> = Vec::new();
    for (i, (span, body)) in arms.enumerate() {
        let leading = fmt.drain_comments_before(span.offset());
        let pattern = pattern_for(i);
        let body_doc = format_expr(fmt, body);
        let arm_doc = pattern
            .append(RcDoc::text(" => "))
            .append(body_doc)
            .append(RcDoc::text(","));
        let arm_end = span.offset() + span.len();
        let trailing = fmt
            .drain_trailing_comment(arm_end)
            .unwrap_or_else(RcDoc::nil);
        arm_docs.push(prepend_comments(leading, arm_doc.append(trailing)));
    }
    arm_docs
}

/// Wrap pre-formatted arm docs in the `<head> {\n  <arms>\n}` block shape.
fn wrap_match_block(head: RcDoc<'static>, arm_docs: Vec<RcDoc<'static>>) -> RcDoc<'static> {
    head.append(RcDoc::text(" {"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(arm_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

fn format_inline_dag_ref(
    fmt: &mut Formatter<'_>,
    path: &ModulePath,
    args: &[ParamBinding],
    output: &str,
) -> RcDoc<'static> {
    let binding_docs: Vec<RcDoc<'static>> = args
        .iter()
        .map(|b| {
            RcDoc::text(b.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &b.value))
        })
        .collect();
    let path_text = path.display_path();
    RcDoc::text(format!("@{path_text}"))
        .append(RcDoc::text("("))
        .append(RcDoc::intersperse(binding_docs, RcDoc::text(", ")))
        .append(RcDoc::text(")."))
        .append(RcDoc::text(output.to_string()))
}
