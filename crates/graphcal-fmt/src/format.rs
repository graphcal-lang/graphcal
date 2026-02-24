use graphcal_syntax::ast::{
    AssertBody, AssertDecl, Attribute, BinOp, ConstDecl, DeclKind, Declaration, DeriveOp, DimDecl,
    DimExpr, DimTerm, Expr, ExprKind, FieldDecl, FieldInit, File, FnBody, FnDecl, FnParam,
    ForBinding, GenericConstraint, GenericParam, Ident, ImportDecl, IndexArg, IndexDecl,
    IndexDeclKind, LetBinding, MapEntry, MatchArm, MatchPattern, MulDivOp, NodeDecl, ParamDecl,
    PatternBinding, TypeDecl, TypeExpr, TypeExprKind, UnaryOp, UnitDecl, UnitDef, UnitExpr,
    VariantDecl,
};
use graphcal_syntax::comments::SourceMetadata;
use graphcal_syntax::names::{IndexName, Spanned};
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
        &self.source[span.offset()..span.offset() + span.len()]
    }

    /// Drain all comments whose span starts before `before_offset`,
    /// returning them as a Doc with hardlines.
    fn drain_comments_before(&mut self, before_offset: usize) -> RcDoc<'static> {
        let mut docs: Vec<RcDoc<'static>> = Vec::new();
        while self.next_comment < self.metadata.comments.len() {
            let comment = &self.metadata.comments[self.next_comment];
            if comment.span.offset() >= before_offset {
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
        if comment.span.offset() > line_end_offset {
            // Check there's no newline between node end and comment start
            let between =
                &self.source[line_end_offset..comment.span.offset().min(self.source.len())];
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
        let leading = fmt.drain_comments_before(decl.span.offset());
        let has_leading_comments = !is_nil(&leading);

        if i > 0 {
            docs.push(RcDoc::hardline());
            // Extra blank line before comments or when original had a blank line
            if has_leading_comments || fmt.has_blank_line_between(prev_end, decl.span.offset()) {
                docs.push(RcDoc::hardline());
            }
        }
        if has_leading_comments {
            docs.push(leading);
        }

        let decl_doc = format_decl(&mut fmt, decl);
        let decl_end = decl.span.offset() + decl.span.len();
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

/// Prepend leading comments before a doc. Returns the doc unchanged if
/// there are no comments. Like Gleam's `commented()` helper.
fn prepend_comments(leading: RcDoc<'static>, doc: RcDoc<'static>) -> RcDoc<'static> {
    if is_nil(&leading) {
        doc
    } else {
        leading.append(doc)
    }
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
        DeclKind::Import(d) => format_import_decl(fmt, d),
        DeclKind::Assert(d) => format_assert_decl(fmt, d),
    };

    // Collect attribute lines: real attributes + synthetic #[derive(...)] for type decls
    let has_attrs = !decl.attributes.is_empty();
    let has_derives = matches!(&decl.kind, DeclKind::Type(t) if !t.derives.is_empty());

    if !has_attrs && !has_derives {
        body
    } else {
        let mut parts: Vec<RcDoc<'static>> = Vec::new();

        // Emit #[derive(...)] first if present
        if let DeclKind::Type(t) = &decl.kind
            && !t.derives.is_empty()
        {
            let derives: Vec<RcDoc<'static>> = t
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
            parts.push(
                RcDoc::text("#[derive(")
                    .append(RcDoc::intersperse(derives, RcDoc::text(", ")))
                    .append(RcDoc::text(")]")),
            );
            parts.push(RcDoc::hardline());
        }

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
            .map(format_attribute_arg)
            .collect::<Vec<_>>();
        doc = doc
            .append(RcDoc::text("("))
            .append(RcDoc::intersperse(args, RcDoc::text(", ")))
            .append(RcDoc::text(")"));
    }
    doc.append(RcDoc::text("]"))
}

fn format_attribute_arg(arg: &graphcal_syntax::ast::AttributeArg) -> RcDoc<'static> {
    match arg {
        graphcal_syntax::ast::AttributeArg::Path { segments, .. } => {
            let parts: Vec<RcDoc<'static>> = segments
                .iter()
                .map(|s| RcDoc::text(s.name.clone()))
                .collect();
            RcDoc::intersperse(parts, RcDoc::text("::"))
        }
        graphcal_syntax::ast::AttributeArg::Group { elements, .. } => {
            let inner: Vec<RcDoc<'static>> = elements.iter().map(format_attribute_arg).collect();
            RcDoc::text("(")
                .append(RcDoc::intersperse(inner, RcDoc::text(", ")))
                .append(RcDoc::text(")"))
        }
    }
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
        def.span.offset(),
        // Find where the unit expr starts
        def.unit_expr.span.offset() - def.span.offset(),
    ));
    // The scale text includes the number and trailing space; trim it
    let scale_text = scale_text.trim_end();
    RcDoc::text(scale_text.to_string())
        .append(RcDoc::text(" "))
        .append(format_unit_expr_inline(&def.unit_expr))
}

/// `type Name { ... }` or `#[derive(...)] type Name<...> { ... }`
fn format_type_decl(_fmt: &mut Formatter<'_>, d: &TypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(&d.generic_params));
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

            let single_sep = RcDoc::text(", ");
            let single_line = header
                .clone()
                .append(RcDoc::text(" = { "))
                .append(RcDoc::intersperse(variant_docs.clone(), single_sep))
                .append(RcDoc::text(" }"));

            let multi_sep = RcDoc::text(",").append(RcDoc::hardline());
            let multi_line = header
                .append(RcDoc::text(" = {"))
                .append(
                    RcDoc::hardline()
                        .append(RcDoc::intersperse(variant_docs, multi_sep))
                        .append(RcDoc::text(","))
                        .nest(INDENT),
                )
                .append(RcDoc::hardline())
                .append(RcDoc::text("}"));

            multi_line.flat_alt(single_line).group()
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

/// `import "path" { name1, name2 };` or `import "path";` or `import "path" as alias;`
fn format_import_decl(_fmt: &Formatter<'_>, d: &ImportDecl) -> RcDoc<'static> {
    match &d.kind {
        graphcal_syntax::ast::ImportKind::Selective(names) => {
            let name_docs: Vec<RcDoc<'static>> = names
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
            RcDoc::text(format!("import \"{}\" {{ ", d.path))
                .append(RcDoc::intersperse(name_docs, RcDoc::text(", ")))
                .append(RcDoc::text(" };"))
        }
        graphcal_syntax::ast::ImportKind::Module { alias: None } => {
            RcDoc::text(format!("import \"{}\";", d.path))
        }
        graphcal_syntax::ast::ImportKind::Module { alias: Some(a) } => {
            RcDoc::text(format!("import \"{}\" as {};", d.path, a.name))
        }
    }
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
        ExprKind::FnCall { name, args } => format_fn_call_expr(fmt, name.value.as_str(), args),
        ExprKind::QualifiedFnCall { module, name, args } => {
            let fn_name = format!("{}::{}", module.name.as_str(), name.value.as_str());
            format_fn_call_expr(fmt, &fn_name, args)
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
fn format_fn_call_expr(fmt: &mut Formatter<'_>, fn_name: &str, args: &[Expr]) -> RcDoc<'static> {
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
    RcDoc::text(fn_name.to_string())
        .append(RcDoc::text("("))
        .append(inner.nest(INDENT).group())
        .append(RcDoc::text(")"))
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

    multi_line.flat_alt(single_line).group()
}

fn format_block_body(fmt: &mut Formatter<'_>, stmts: &[LetBinding], tail: &Expr) -> RcDoc<'static> {
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

fn format_map_literal(fmt: &mut Formatter<'_>, entries: &[MapEntry]) -> RcDoc<'static> {
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

/// Render an `RcDoc` to a string (for measuring column widths).
fn render_doc_to_string(doc: &RcDoc<'static>) -> String {
    let mut buf = Vec::new();
    // Use a large width so we get single-line rendering for cell values.
    let _ = doc.render(1000, &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}

/// Format a table literal expression: `table[Index1, Index2] { ... }`
///
/// Handles 1D, 2D, and 3D+ tables with column-aligned output.
fn format_table_literal(
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

fn format_match(fmt: &mut Formatter<'_>, scrutinee: &Expr, arms: &[MatchArm]) -> RcDoc<'static> {
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
