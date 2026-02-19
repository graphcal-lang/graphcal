use graphcal_syntax::ast::{
    AssertBody, AssertDecl, Attribute, BinOp, ConstDecl, DeclKind, Declaration, DeriveOp, DimDecl,
    DimExpr, DimTerm, Expr, ExprKind, FieldDecl, FieldInit, File, FnBody, FnDecl, FnParam,
    ForBinding, GenericConstraint, GenericParam, Ident, IndexArg, IndexDecl, IndexDeclKind,
    LetBinding, MapEntry, MatchArm, MatchPattern, MulDivOp, NodeDecl, ParamDecl, PatternBinding,
    TypeDecl, TypeExpr, TypeExprKind, UnaryOp, UnitDecl, UnitDef, UnitExpr, UseDecl, VariantDecl,
};
use graphcal_syntax::comments::SourceMetadata;
use graphcal_syntax::span::Span;
use pretty::RcDoc;

const INDENT: isize = 4;

/// State for tracking comments during formatting.
struct Formatter<'src> {
    source: &'src str,
    metadata: &'src SourceMetadata,
    next_comment: usize,
}

impl<'src> Formatter<'src> {
    const fn new(source: &'src str, metadata: &'src SourceMetadata) -> Self {
        Self {
            source,
            metadata,
            next_comment: 0,
        }
    }

    /// Get the original source text for a span.
    fn slice(&self, span: Span) -> &'src str {
        &self.source[span.offset..span.offset + span.len]
    }

    /// Drain all comments whose span starts before `before_offset`,
    /// returning them as a Doc with hardlines.
    fn drain_comments_before(&mut self, before_offset: usize) -> RcDoc<'static> {
        let mut docs: Vec<RcDoc<'static>> = Vec::new();
        while self.next_comment < self.metadata.comments.len() {
            let comment = &self.metadata.comments[self.next_comment];
            if comment.span.offset >= before_offset {
                break;
            }
            docs.push(RcDoc::text(comment.text.clone()));
            docs.push(RcDoc::hardline());
            self.next_comment += 1;
        }
        RcDoc::concat(docs)
    }

    /// Drain a trailing comment on the same line as `after_offset`.
    /// Returns the comment text (with leading space) or nil.
    fn drain_trailing_comment(&mut self, line_end_offset: usize) -> RcDoc<'static> {
        if self.next_comment >= self.metadata.comments.len() {
            return RcDoc::nil();
        }
        let comment = &self.metadata.comments[self.next_comment];
        // A trailing comment must be on the same line — its offset must be
        // between the end of the node and the next newline.
        if comment.span.offset > line_end_offset {
            // Check there's no newline between node end and comment start
            let between = &self.source[line_end_offset..comment.span.offset.min(self.source.len())];
            if !between.contains('\n') {
                self.next_comment += 1;
                return RcDoc::text(format!(" {}", comment.text));
            }
        }
        RcDoc::nil()
    }

    /// Check if there's a blank line in the source between two byte offsets.
    fn has_blank_line_between(&self, start: usize, end: usize) -> bool {
        self.metadata
            .blank_line_offsets
            .iter()
            .any(|&offset| offset >= start && offset < end)
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn format_file(file: &File, source: &str, metadata: &SourceMetadata) -> RcDoc<'static> {
    let mut fmt = Formatter::new(source, metadata);
    let mut docs: Vec<RcDoc<'static>> = Vec::new();

    let mut prev_end: usize = 0;
    for (i, decl) in file.declarations.iter().enumerate() {
        // Emit leading comments before this declaration
        let leading = fmt.drain_comments_before(decl.span.offset);
        let has_leading_comments = !is_nil(&leading);

        if i > 0 {
            docs.push(RcDoc::hardline());
            // Extra blank line before comments or when original had a blank line
            if has_leading_comments || fmt.has_blank_line_between(prev_end, decl.span.offset) {
                docs.push(RcDoc::hardline());
            }
        }
        if has_leading_comments {
            docs.push(leading);
        }

        let decl_doc = format_decl(&mut fmt, decl);
        let decl_end = decl.span.offset + decl.span.len;
        let trailing = fmt.drain_trailing_comment(decl_end);
        docs.push(decl_doc.append(trailing));
        prev_end = decl_end;
    }

    // Drain any remaining comments at end of file
    let remaining = fmt.drain_comments_before(usize::MAX);
    if !is_nil(&remaining) {
        docs.push(RcDoc::hardline());
        docs.push(remaining);
    }

    // Final newline
    docs.push(RcDoc::hardline());

    RcDoc::concat(docs)
}

/// Helper: check if an `RcDoc` is effectively nil (empty).
/// We use a simple heuristic — render to empty string.
fn is_nil(doc: &RcDoc<'static>) -> bool {
    let mut buf = Vec::new();
    let _ = doc.render(1000, &mut buf);
    buf.is_empty()
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

fn format_decl(fmt: &mut Formatter<'_>, decl: &Declaration) -> RcDoc<'static> {
    let body = match &decl.kind {
        DeclKind::Param(d) => format_param_decl(fmt, d),
        DeclKind::Node(d) => format_node_decl(fmt, d),
        DeclKind::Const(d) => format_const_decl(fmt, d),
        DeclKind::Dimension(d) => format_dim_decl(fmt, d),
        DeclKind::Unit(d) => format_unit_decl(fmt, d),
        DeclKind::Type(d) => format_type_decl(fmt, d),
        DeclKind::Fn(d) => format_fn_decl(fmt, d),
        DeclKind::Index(d) => format_index_decl(fmt, d),
        DeclKind::Use(d) => format_use_decl(fmt, d),
        DeclKind::Assert(d) => format_assert_decl(fmt, d),
    };

    if decl.attributes.is_empty() {
        body
    } else {
        let mut parts: Vec<RcDoc<'static>> = Vec::new();
        for attr in &decl.attributes {
            parts.push(format_attribute(attr));
            parts.push(RcDoc::hardline());
        }
        parts.push(body);
        RcDoc::concat(parts)
    }
}

fn format_attribute(attr: &Attribute) -> RcDoc<'static> {
    let mut doc = RcDoc::text("#[").append(RcDoc::text(attr.name.name.clone()));
    if !attr.args.is_empty() {
        let args = attr
            .args
            .iter()
            .map(|a| RcDoc::text(a.name.clone()))
            .collect::<Vec<_>>();
        doc = doc
            .append(RcDoc::text("("))
            .append(RcDoc::intersperse(args, RcDoc::text(", ")))
            .append(RcDoc::text(")"));
    }
    doc.append(RcDoc::text("]"))
}

/// `param name: Type = expr;`
fn format_param_decl(fmt: &mut Formatter<'_>, d: &ParamDecl) -> RcDoc<'static> {
    format_value_decl(fmt, "param", &d.name.value, &d.type_ann, &d.value)
}

/// `node name: Type = expr;`
fn format_node_decl(fmt: &mut Formatter<'_>, d: &NodeDecl) -> RcDoc<'static> {
    format_value_decl(fmt, "node", &d.name.value, &d.type_ann, &d.value)
}

/// `const name: Type = expr;`
fn format_const_decl(fmt: &mut Formatter<'_>, d: &ConstDecl) -> RcDoc<'static> {
    format_value_decl(fmt, "const", &d.name.value, &d.type_ann, &d.value)
}

/// Shared logic for param/node/const declarations.
fn format_value_decl(
    fmt: &mut Formatter<'_>,
    keyword: &str,
    name: &graphcal_syntax::names::DeclName,
    type_ann: &TypeExpr,
    value: &Expr,
) -> RcDoc<'static> {
    let header = RcDoc::text(keyword.to_string())
        .append(RcDoc::text(" "))
        .append(RcDoc::text(name.as_str().to_string()))
        .append(RcDoc::text(": "))
        .append(format_type_expr_inline(type_ann))
        .append(RcDoc::text(" = "));

    let val = format_expr(fmt, value);
    header.append(val).append(RcDoc::text(";"))
}

/// `dimension Name = DimExpr;` or `dimension Name;`
fn format_dim_decl(_fmt: &Formatter<'_>, d: &DimDecl) -> RcDoc<'static> {
    let mut doc = RcDoc::text("dimension ").append(RcDoc::text(d.name.value.as_str().to_string()));
    if let Some(ref def) = d.definition {
        doc = doc
            .append(RcDoc::text(" = "))
            .append(format_dim_expr_inline(def));
    }
    doc.append(RcDoc::text(";"))
}

/// `unit name: Dim = scale unit_expr;` or `unit name: Dim;`
fn format_unit_decl(fmt: &Formatter<'_>, d: &UnitDecl) -> RcDoc<'static> {
    let mut doc = RcDoc::text("unit ")
        .append(RcDoc::text(d.name.value.as_str().to_string()))
        .append(RcDoc::text(": "))
        .append(format_dim_expr_inline(&d.dim_type));
    if let Some(ref def) = d.definition {
        doc = doc
            .append(RcDoc::text(" = "))
            .append(format_unit_def(fmt, def));
    }
    doc.append(RcDoc::text(";"))
}

fn format_unit_def(fmt: &Formatter<'_>, def: &UnitDef) -> RcDoc<'static> {
    // Recover original scale text from source
    let scale_text = fmt.slice(Span::new(
        def.span.offset,
        // Find where the unit expr starts
        def.unit_expr.span.offset - def.span.offset,
    ));
    // The scale text includes the number and trailing space; trim it
    let scale_text = scale_text.trim_end();
    RcDoc::text(scale_text.to_string())
        .append(RcDoc::text(" "))
        .append(format_unit_expr_inline(&def.unit_expr))
}

/// `type Name { ... }` or `type Name<...> derive(...) { ... }`
fn format_type_decl(_fmt: &mut Formatter<'_>, d: &TypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(&d.generic_params));
    }

    if !d.derives.is_empty() {
        let derives: Vec<RcDoc<'static>> = d
            .derives
            .iter()
            .map(|d| {
                RcDoc::text(match d.value {
                    DeriveOp::Add => "Add",
                    DeriveOp::Sub => "Sub",
                    DeriveOp::Neg => "Neg",
                })
            })
            .collect();
        header = header
            .append(RcDoc::text(" derive("))
            .append(RcDoc::intersperse(derives, RcDoc::text(", ")))
            .append(RcDoc::text(")"));
    }

    if d.variants.is_empty() {
        return header.append(RcDoc::text(" {}"));
    }

    // Check if this is struct sugar (single variant with same name as type)
    let is_struct_sugar =
        d.variants.len() == 1 && d.variants[0].name.value.as_str() == d.name.value.as_str();

    if is_struct_sugar {
        let variant = &d.variants[0];
        let fields = format_field_decls(&variant.fields);
        header
            .append(RcDoc::text(" {"))
            .append(RcDoc::hardline().append(fields).nest(INDENT))
            .append(RcDoc::hardline())
            .append(RcDoc::text("}"))
    } else {
        // Multi-variant (tagged union)
        let mut variant_docs: Vec<RcDoc<'static>> = Vec::new();
        for variant in &d.variants {
            variant_docs.push(format_variant_decl(variant));
        }
        header
            .append(RcDoc::text(" {"))
            .append(
                RcDoc::hardline()
                    .append(RcDoc::intersperse(variant_docs, RcDoc::hardline()))
                    .nest(INDENT),
            )
            .append(RcDoc::hardline())
            .append(RcDoc::text("}"))
    }
}

fn format_variant_decl(v: &VariantDecl) -> RcDoc<'static> {
    let name = RcDoc::text(v.name.value.as_str().to_string());
    if v.fields.is_empty() {
        name
    } else {
        let fields: Vec<RcDoc<'static>> = v
            .fields
            .iter()
            .map(|f| format_single_field_decl(f))
            .collect();
        name.append(RcDoc::text(" { "))
            .append(RcDoc::intersperse(fields, RcDoc::text(", ")))
            .append(RcDoc::text(" }"))
    }
}

fn format_field_decls(fields: &[FieldDecl]) -> RcDoc<'static> {
    let field_docs: Vec<RcDoc<'static>> = fields
        .iter()
        .map(|f| format_single_field_decl(f).append(RcDoc::text(",")))
        .collect();
    RcDoc::intersperse(field_docs, RcDoc::hardline())
}

fn format_single_field_decl(f: &FieldDecl) -> RcDoc<'static> {
    RcDoc::text(f.name.value.as_str().to_string())
        .append(RcDoc::text(": "))
        .append(format_type_expr_inline(&f.type_ann))
}

/// `fn name<...>(...) -> RetType = expr;` or `fn name<...>(...) -> RetType { ... }`
fn format_fn_decl(fmt: &mut Formatter<'_>, d: &FnDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("fn ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(&d.generic_params));
    }

    // Parameters
    let params = format_fn_params(&d.params);
    header = header.append(params);

    // Return type
    header = header
        .append(RcDoc::text(" -> "))
        .append(format_type_expr_inline(&d.return_type));

    match &d.body {
        FnBody::Short(expr) => header
            .append(RcDoc::text(" = "))
            .append(format_expr(fmt, expr))
            .append(RcDoc::text(";")),
        FnBody::Block { stmts, expr } => {
            let body = format_block_body(fmt, stmts, expr);
            header
                .append(RcDoc::text(" {"))
                .append(RcDoc::hardline().append(body).nest(INDENT))
                .append(RcDoc::hardline())
                .append(RcDoc::text("}"))
        }
    }
}

fn format_fn_params(params: &[FnParam]) -> RcDoc<'static> {
    if params.is_empty() {
        return RcDoc::text("()");
    }
    let param_docs: Vec<RcDoc<'static>> = params
        .iter()
        .map(|p| {
            RcDoc::text(p.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_type_expr_inline(&p.type_ann))
        })
        .collect();

    let sep = RcDoc::text(",").append(RcDoc::line());
    let inner = RcDoc::intersperse(param_docs, sep);

    RcDoc::text("(")
        .append(inner.nest(INDENT).group())
        .append(RcDoc::text(")"))
}

fn format_generic_params(params: &[GenericParam]) -> RcDoc<'static> {
    let param_docs: Vec<RcDoc<'static>> = params
        .iter()
        .map(|p| {
            let constraint = match p.constraint {
                GenericConstraint::Dim => "Dim",
                GenericConstraint::Index => "Index",
                GenericConstraint::Type => "Type",
            };
            let mut doc = RcDoc::text(p.name.value.as_str().to_string())
                .append(RcDoc::text(": "))
                .append(RcDoc::text(constraint));
            if let Some(ref default) = p.default {
                doc = doc
                    .append(RcDoc::text(" = "))
                    .append(format_type_expr_inline(default));
            }
            doc
        })
        .collect();
    RcDoc::text("<")
        .append(RcDoc::intersperse(param_docs, RcDoc::text(", ")))
        .append(RcDoc::text(">"))
}

/// `index Name = { V1, V2, V3 }` or `index Name = range(...)`
fn format_index_decl(fmt: &mut Formatter<'_>, d: &IndexDecl) -> RcDoc<'static> {
    let header = RcDoc::text("index ").append(RcDoc::text(d.name.value.as_str().to_string()));

    match &d.kind {
        IndexDeclKind::Named { variants } => {
            let variant_docs: Vec<RcDoc<'static>> = variants
                .iter()
                .map(|v| RcDoc::text(v.value.as_str().to_string()))
                .collect();
            let sep = RcDoc::text(",").append(RcDoc::line());
            let inner = RcDoc::intersperse(variant_docs, sep);
            header
                .append(RcDoc::text(" = { "))
                .append(inner.group())
                .append(RcDoc::text(" }"))
        }
        IndexDeclKind::Range { start, end, step } => header
            .append(RcDoc::text(" = range("))
            .append(format_expr(fmt, start))
            .append(RcDoc::text(", "))
            .append(format_expr(fmt, end))
            .append(RcDoc::text(", step: "))
            .append(format_expr(fmt, step))
            .append(RcDoc::text(");")),
    }
}

/// `use "path" { name1, name2 };`
fn format_use_decl(_fmt: &Formatter<'_>, d: &UseDecl) -> RcDoc<'static> {
    let name_docs: Vec<RcDoc<'static>> = d
        .names
        .iter()
        .map(|item| {
            let mut doc = RcDoc::text(item.name.name.clone());
            if let Some(ref alias) = item.alias {
                doc = doc
                    .append(RcDoc::text(" as "))
                    .append(RcDoc::text(alias.name.clone()));
            }
            doc
        })
        .collect();
    RcDoc::text(format!("use \"{}\" {{ ", d.path))
        .append(RcDoc::intersperse(name_docs, RcDoc::text(", ")))
        .append(RcDoc::text(" };"))
}

/// `assert name = expr;`
fn format_assert_decl(fmt: &mut Formatter<'_>, d: &AssertDecl) -> RcDoc<'static> {
    match &d.body {
        AssertBody::Expr(body_expr) => RcDoc::text(format!("assert {} = ", d.name.value))
            .append(format_expr(fmt, body_expr))
            .append(RcDoc::text(";")),
        AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
        } => {
            let mut doc = RcDoc::text(format!("assert {} = ", d.name.value))
                .append(format_expr(fmt, actual))
                .append(RcDoc::text(" ~= "))
                .append(format_expr(fmt, expected))
                .append(RcDoc::text(" +/- "))
                .append(format_expr(fmt, tolerance));
            if *is_relative {
                doc = doc.append(RcDoc::text("%"));
            }
            doc.append(RcDoc::text(";"))
        }
    }
}

// ---------------------------------------------------------------------------
// Type expressions
// ---------------------------------------------------------------------------

/// Format a type expression.
fn format_type_expr_inline(te: &TypeExpr) -> RcDoc<'static> {
    match &te.kind {
        TypeExprKind::Dimensionless => RcDoc::text("Dimensionless"),
        TypeExprKind::Bool => RcDoc::text("Bool"),
        TypeExprKind::Int => RcDoc::text("Int"),
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

fn format_dim_expr_inline(de: &DimExpr) -> RcDoc<'static> {
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

fn format_unit_expr_inline(unit_expr: &UnitExpr) -> RcDoc<'static> {
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

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[expect(clippy::too_many_lines, reason = "match on ExprKind variants")]
fn format_expr(fmt: &mut Formatter<'_>, expr: &Expr) -> RcDoc<'static> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Integer(_) => {
            // Recover original text from source to preserve formatting (e.g. 1_000, 3.98e5)
            RcDoc::text(fmt.slice(expr.span).to_string())
        }
        ExprKind::Bool(b) => RcDoc::text(if *b { "true" } else { "false" }),
        ExprKind::GraphRef(name) => RcDoc::text(format!("@{}", name.value.as_str())),
        ExprKind::ConstRef(name) => RcDoc::text(name.value.as_str().to_string()),
        ExprKind::LocalRef(ident) => RcDoc::text(ident.name.clone()),
        ExprKind::BinOp { op, lhs, rhs } => format_binop(fmt, *op, lhs, rhs),
        ExprKind::UnaryOp { op, operand } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            RcDoc::text(op_str).append(format_expr(fmt, operand))
        }
        ExprKind::FnCall { name, args } => {
            let arg_docs: Vec<RcDoc<'static>> = args.iter().map(|a| format_expr(fmt, a)).collect();
            let sep = RcDoc::text(",").append(RcDoc::line());
            let inner = RcDoc::intersperse(arg_docs, sep);
            RcDoc::text(name.value.as_str().to_string())
                .append(RcDoc::text("("))
                .append(inner.nest(INDENT).group())
                .append(RcDoc::text(")"))
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => format_if(fmt, condition, then_branch, else_branch),
        ExprKind::UnitLiteral { value: _, unit } => {
            // Recover the full literal from source to preserve number formatting
            let unit_start = unit.span.offset;
            let lit_source = &fmt.source[expr.span.offset..unit_start];
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
    format_binop_child(fmt, lhs, op, false)
        .append(RcDoc::text(op_str(op)))
        .append(format_binop_child(fmt, rhs, op, true))
}

fn format_if(
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

    single_line.flat_alt(multi_line).group()
}

fn format_block_body(fmt: &mut Formatter<'_>, stmts: &[LetBinding], tail: &Expr) -> RcDoc<'static> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    for stmt in stmts {
        docs.push(format_let_binding(fmt, stmt));
    }
    docs.push(format_expr(fmt, tail));
    RcDoc::intersperse(docs, RcDoc::hardline())
}

fn format_let_binding(fmt: &mut Formatter<'_>, lb: &LetBinding) -> RcDoc<'static> {
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

fn format_struct_construction(
    fmt: &mut Formatter<'_>,
    type_name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::StructTypeName>,
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

    let field_docs: Vec<RcDoc<'static>> = fields
        .iter()
        .map(|f| {
            let name = RcDoc::text(f.name.value.as_str().to_string());
            match &f.value {
                Some(val) => name.append(RcDoc::text(": ")).append(format_expr(fmt, val)),
                None => name, // shorthand
            }
        })
        .collect();

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

fn format_map_literal(fmt: &mut Formatter<'_>, entries: &[MapEntry]) -> RcDoc<'static> {
    let entry_docs: Vec<RcDoc<'static>> = entries
        .iter()
        .map(|e| {
            RcDoc::text(format!(
                "{}::{}",
                e.index.value.as_str(),
                e.variant.value.as_str()
            ))
            .append(RcDoc::text(": "))
            .append(format_expr(fmt, &e.value))
        })
        .collect();

    let sep = RcDoc::text(",").append(RcDoc::hardline());
    RcDoc::text("{")
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(entry_docs, sep))
                .append(RcDoc::text(","))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

fn format_for_comp(
    fmt: &mut Formatter<'_>,
    bindings: &[ForBinding],
    body: &Expr,
) -> RcDoc<'static> {
    let binding_docs: Vec<RcDoc<'static>> = bindings
        .iter()
        .map(|b| {
            RcDoc::text(b.var.name.clone())
                .append(RcDoc::text(": "))
                .append(RcDoc::text(b.index.value.as_str().to_string()))
        })
        .collect();
    RcDoc::text("for ")
        .append(RcDoc::intersperse(binding_docs, RcDoc::text(", ")))
        .append(RcDoc::text(" { "))
        .append(format_expr(fmt, body))
        .append(RcDoc::text(" }"))
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

fn format_match(fmt: &mut Formatter<'_>, scrutinee: &Expr, arms: &[MatchArm]) -> RcDoc<'static> {
    let arm_docs: Vec<RcDoc<'static>> = arms
        .iter()
        .map(|arm| {
            let pattern = format_match_pattern(&arm.pattern);
            let body = format_expr(fmt, &arm.body);
            pattern
                .append(RcDoc::text(" => "))
                .append(body)
                .append(RcDoc::text(","))
        })
        .collect();

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

fn format_match_pattern(p: &MatchPattern) -> RcDoc<'static> {
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
