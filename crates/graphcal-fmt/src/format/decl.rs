use graphcal_syntax::ast::{
    AssertBody, AssertDecl, Attribute, ConstDecl, DeclKind, Declaration, DimDecl, Encoding,
    FieldDecl, FigureDecl, FnBody, FnDecl, FnParam, GenericConstraint, GenericParam, ImportDecl,
    IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamBinding, ParamDecl, PlotDecl, TypeDecl,
    TypeExpr, UnitDecl, UnitDef, UnionTypeDecl,
};
use pretty::RcDoc;

use super::{
    Formatter, INDENT, format_block_body, format_dim_expr_inline, format_expr,
    format_type_expr_inline, format_unit_expr_inline,
};

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

pub fn format_decl(fmt: &mut Formatter<'_>, decl: &Declaration) -> RcDoc<'static> {
    let body = match &decl.kind {
        DeclKind::Param(d) => format_param_decl(fmt, d),
        DeclKind::Node(d) => format_node_decl(fmt, d),
        DeclKind::Const(d) => format_const_decl(fmt, d),
        DeclKind::Dimension(d) => format_dim_decl(fmt, d),
        DeclKind::Unit(d) => format_unit_decl(fmt, d),
        DeclKind::Type(d) => format_type_decl(fmt, d),
        DeclKind::UnionType(d) => format_union_type_decl(fmt, d),
        DeclKind::Fn(d) => format_fn_decl(fmt, d),
        DeclKind::Index(d) => format_index_decl(fmt, d),
        DeclKind::Import(d) => format_import_decl(fmt, d),
        DeclKind::Assert(d) => format_assert_decl(fmt, d),
        DeclKind::Plot(d) => format_plot_decl(fmt, d),
        DeclKind::Figure(d) => format_figure_decl(fmt, d),
        DeclKind::Layer(d) => format_layer_decl(fmt, d),
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

/// `param name: Type = expr;` or `param name: Type;` (required param)
fn format_param_decl(fmt: &mut Formatter<'_>, d: &ParamDecl) -> RcDoc<'static> {
    d.value.as_ref().map_or_else(
        || {
            RcDoc::text("param")
                .append(RcDoc::text(" "))
                .append(RcDoc::text(d.name.value.as_str().to_string()))
                .append(RcDoc::text(": "))
                .append(format_type_expr_inline(&d.type_ann))
                .append(RcDoc::text(";"))
        },
        |value| format_value_decl(fmt, "param", &d.name.value, &d.type_ann, value),
    )
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
    value: &graphcal_syntax::ast::Expr,
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
fn format_unit_decl(fmt: &mut Formatter<'_>, d: &UnitDecl) -> RcDoc<'static> {
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

fn format_unit_def(fmt: &mut Formatter<'_>, def: &UnitDef) -> RcDoc<'static> {
    use graphcal_syntax::ast::ExprKind;
    // Simple numeric literals are formatted directly (preserving original text);
    // complex expressions are wrapped in parentheses.
    let scale_doc = match &def.scale_expr.kind {
        ExprKind::Number(_) | ExprKind::Integer(_) => format_expr(fmt, &def.scale_expr),
        _ => RcDoc::text("(")
            .append(format_expr(fmt, &def.scale_expr))
            .append(RcDoc::text(")")),
    };
    scale_doc
        .append(RcDoc::text(" "))
        .append(format_unit_expr_inline(&def.unit_expr))
}

/// `type Name { ... }` or `type Name;` or `#[derive(...)] type Name<...> { ... }`
fn format_type_decl(_fmt: &mut Formatter<'_>, d: &TypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(&d.generic_params));
    }

    if d.fields.is_empty() {
        // Unit type or empty record type — format as `type Name;`
        return header.append(RcDoc::text(";"));
    }

    // Record type with fields
    let fields = format_field_decls(&d.fields);
    header
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(fields).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// `type Name = A | B | C;` or `type Name<D: Dim> = Ok<D> | Err;`
fn format_union_type_decl(_fmt: &mut Formatter<'_>, d: &UnionTypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(&d.generic_params));
    }

    let member_docs: Vec<RcDoc<'static>> = d
        .members
        .iter()
        .map(|m| {
            let mut doc = RcDoc::text(m.name.value.as_str().to_string());
            if !m.type_args.is_empty() {
                let args: Vec<RcDoc<'static>> = m
                    .type_args
                    .iter()
                    .map(|a| format_type_expr_inline(a))
                    .collect();
                doc = doc
                    .append(RcDoc::text("<"))
                    .append(RcDoc::intersperse(args, RcDoc::text(", ")))
                    .append(RcDoc::text(">"));
            }
            doc
        })
        .collect();

    header
        .append(RcDoc::text(" = "))
        .append(RcDoc::intersperse(member_docs, RcDoc::text(" | ")))
        .append(RcDoc::text(";"))
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

/// `cat Name { V1, V2, V3 }` or `cat Name;` (required)
/// or `range Name(start, end, step: step);` or `range Name: Dim;` (required)
fn format_index_decl(fmt: &mut Formatter<'_>, d: &IndexDecl) -> RcDoc<'static> {
    match &d.kind {
        IndexDeclKind::RequiredNamed => RcDoc::text("cat ")
            .append(RcDoc::text(d.name.value.as_str().to_string()))
            .append(RcDoc::text(";")),
        IndexDeclKind::RequiredRange { dimension } => RcDoc::text("range ")
            .append(RcDoc::text(d.name.value.as_str().to_string()))
            .append(RcDoc::text(": "))
            .append(format_dim_expr_inline(dimension))
            .append(RcDoc::text(";")),
        IndexDeclKind::Named { variants } => {
            let header = RcDoc::text("cat ").append(RcDoc::text(d.name.value.as_str().to_string()));

            let variant_docs: Vec<RcDoc<'static>> = variants
                .iter()
                .map(|v| RcDoc::text(v.value.as_str().to_string()))
                .collect();

            let single_sep = RcDoc::text(", ");
            let single_line = header
                .clone()
                .append(RcDoc::text(" { "))
                .append(RcDoc::intersperse(variant_docs.clone(), single_sep))
                .append(RcDoc::text(" }"));

            let multi_sep = RcDoc::text(",").append(RcDoc::hardline());
            let multi_line = header
                .append(RcDoc::text(" {"))
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
        IndexDeclKind::Range { start, end, step } => {
            let header =
                RcDoc::text("range ").append(RcDoc::text(d.name.value.as_str().to_string()));

            header
                .append(RcDoc::text("("))
                .append(format_expr(fmt, start))
                .append(RcDoc::text(", "))
                .append(format_expr(fmt, end))
                .append(RcDoc::text(", step: "))
                .append(format_expr(fmt, step))
                .append(RcDoc::text(");"))
        }
    }
}

/// `import "path" { name1, name2 };` or `import "path";` or `import "path" as alias;`
/// Optionally with param bindings: `import "path"(x = 1.0 km) { ... };`
fn format_import_decl(fmt: &mut Formatter<'_>, d: &ImportDecl) -> RcDoc<'static> {
    let bindings_doc = format_import_param_bindings(fmt, &d.param_bindings);

    let path_doc = match &d.path {
        graphcal_syntax::ast::ImportPath::FilePath { path, .. } => {
            RcDoc::text(format!("import \"{path}\""))
        }
        graphcal_syntax::ast::ImportPath::ModulePath { segments, .. } => {
            let path_str = segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("/");
            RcDoc::text(format!("import {path_str}"))
        }
    };

    match &d.kind {
        graphcal_syntax::ast::ImportKind::Selective(names) => {
            let name_docs: Vec<RcDoc<'static>> = names
                .iter()
                .map(|item| {
                    let mut doc = RcDoc::nil();
                    for attr in &item.attributes {
                        doc = doc.append(format_attribute(attr)).append(RcDoc::text(" "));
                    }
                    doc = doc.append(RcDoc::text(item.name.name.clone()));
                    if let Some(ref alias) = item.alias {
                        doc = doc
                            .append(RcDoc::text(" as "))
                            .append(RcDoc::text(alias.name.clone()));
                    }
                    doc
                })
                .collect();
            path_doc
                .append(bindings_doc)
                .append(RcDoc::text(" { "))
                .append(RcDoc::intersperse(name_docs, RcDoc::text(", ")))
                .append(RcDoc::text(" };"))
        }
        graphcal_syntax::ast::ImportKind::Module { alias: None } => {
            path_doc.append(bindings_doc).append(RcDoc::text(";"))
        }
        graphcal_syntax::ast::ImportKind::Module { alias: Some(a) } => path_doc
            .append(bindings_doc)
            .append(RcDoc::text(format!(" as {};", a.name))),
    }
}

/// Format param bindings: `(name: expr, ...)` or empty if no bindings.
fn format_import_param_bindings(
    fmt: &mut Formatter<'_>,
    bindings: &[ParamBinding],
) -> RcDoc<'static> {
    if bindings.is_empty() {
        return RcDoc::nil();
    }
    let binding_docs: Vec<RcDoc<'static>> = bindings
        .iter()
        .map(|b| {
            RcDoc::text(b.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &b.value))
        })
        .collect();
    RcDoc::text("(")
        .append(RcDoc::intersperse(binding_docs, RcDoc::text(", ")))
        .append(RcDoc::text(")"))
}

/// `plot name = { mark: type, encode: { x: ..., y: ... }, title: "..." };`
fn format_plot_decl(fmt: &mut Formatter<'_>, d: &PlotDecl) -> RcDoc<'static> {
    let header = RcDoc::text(format!("plot {} = ", d.name.value));

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();

    // Emit mark field
    field_docs.push(format_mark_spec(fmt, &d.mark));

    // Emit encode block
    if !d.encodings.is_empty() {
        field_docs.push(format_encode_block(fmt, &d.encodings));
    }

    // Emit other properties
    for f in &d.properties {
        field_docs.push(
            RcDoc::text(f.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &f.value))
                .append(RcDoc::text(",")),
        );
    }

    if field_docs.is_empty() {
        return header.append(RcDoc::text("{};"));
    }

    header
        .append(RcDoc::text("{"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(field_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("};"))
}

/// Format a mark specification: `mark: point,` or `mark: line { stroke_width: 2.0, },`
fn format_mark_spec(
    fmt: &mut Formatter<'_>,
    mark: &graphcal_syntax::ast::MarkSpec,
) -> RcDoc<'static> {
    if mark.properties.is_empty() {
        RcDoc::text(format!("mark: {},", mark.mark_type))
    } else {
        let prop_docs: Vec<RcDoc<'static>> = mark
            .properties
            .iter()
            .map(|f| {
                RcDoc::text(f.name.name.clone())
                    .append(RcDoc::text(": "))
                    .append(format_expr(fmt, &f.value))
                    .append(RcDoc::text(","))
            })
            .collect();

        RcDoc::text(format!("mark: {} ", mark.mark_type))
            .append(RcDoc::text("{"))
            .append(
                RcDoc::hardline()
                    .append(RcDoc::intersperse(prop_docs, RcDoc::hardline()))
                    .nest(INDENT),
            )
            .append(RcDoc::hardline())
            .append(RcDoc::text("},"))
    }
}

/// Format an encode block: `encode: { x: ..., y: ..., },`
fn format_encode_block(fmt: &mut Formatter<'_>, encodings: &[Encoding]) -> RcDoc<'static> {
    let channel_docs: Vec<RcDoc<'static>> = encodings
        .iter()
        .map(|e| {
            RcDoc::text(e.channel.to_string())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &e.value))
                .append(RcDoc::text(","))
        })
        .collect();

    RcDoc::text("encode: {")
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(channel_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("},"))
}

/// `figure name = { plots: [a, b], title: "...", };`
fn format_figure_decl(fmt: &mut Formatter<'_>, d: &FigureDecl) -> RcDoc<'static> {
    let header = RcDoc::text(format!("figure {} = ", d.name.value));

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();

    // Emit `plots: [name1, name2],`
    if !d.plot_names.is_empty() {
        let names: Vec<RcDoc<'static>> = d
            .plot_names
            .iter()
            .map(|p| RcDoc::text(p.value.as_str().to_string()))
            .collect();
        field_docs.push(
            RcDoc::text("plots: [")
                .append(RcDoc::intersperse(names, RcDoc::text(", ")))
                .append(RcDoc::text("],")),
        );
    }

    // Emit other fields
    for f in &d.fields {
        field_docs.push(
            RcDoc::text(f.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &f.value))
                .append(RcDoc::text(",")),
        );
    }

    if field_docs.is_empty() {
        return header.append(RcDoc::text("{};"));
    }

    header
        .append(RcDoc::text("{"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(field_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("};"))
}

fn format_layer_decl(fmt: &mut Formatter<'_>, d: &LayerDecl) -> RcDoc<'static> {
    let header = RcDoc::text(format!("layer {} = ", d.name.value));

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();

    // Emit `plots: [name1, name2],`
    if !d.plot_names.is_empty() {
        let names: Vec<RcDoc<'static>> = d
            .plot_names
            .iter()
            .map(|p| RcDoc::text(p.value.as_str().to_string()))
            .collect();
        field_docs.push(
            RcDoc::text("plots: [")
                .append(RcDoc::intersperse(names, RcDoc::text(", ")))
                .append(RcDoc::text("],")),
        );
    }

    // Emit other fields
    for f in &d.fields {
        field_docs.push(
            RcDoc::text(f.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &f.value))
                .append(RcDoc::text(",")),
        );
    }

    if field_docs.is_empty() {
        return header.append(RcDoc::text("{};"));
    }

    header
        .append(RcDoc::text("{"))
        .append(
            RcDoc::hardline()
                .append(RcDoc::intersperse(field_docs, RcDoc::hardline()))
                .nest(INDENT),
        )
        .append(RcDoc::hardline())
        .append(RcDoc::text("};"))
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
