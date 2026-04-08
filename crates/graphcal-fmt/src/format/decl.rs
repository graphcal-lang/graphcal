use graphcal_compiler::syntax::ast::{
    AssertBody, AssertDecl, Attribute, BaseDimDecl, DagDecl, DeclKind, Declaration, DimDecl,
    Encoding, FieldDecl, FigureDecl, GenericConstraint, GenericParam, ImportDecl, IncludeDecl,
    IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamBinding, ParamDecl, PlotDecl, TypeDecl,
    TypeExpr, UnionTypeDecl, UnitDecl, UnitDef,
};
use pretty::RcDoc;

use super::{
    Formatter, INDENT, format_dim_expr_inline, format_expr, format_type_expr_inline,
    format_unit_expr_inline,
};

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

pub fn format_decl(fmt: &mut Formatter<'_>, decl: &Declaration) -> RcDoc<'static> {
    let body = match &decl.kind {
        DeclKind::Param(d) => format_param_decl(fmt, d),
        DeclKind::Node(d) => format_node_decl(fmt, d),
        DeclKind::ConstNode(d) => format_const_node_decl(fmt, d),
        DeclKind::BaseDimension(d) => format_base_dim_decl(d),
        DeclKind::Dimension(d) => format_dim_decl(fmt, d),
        DeclKind::Unit(d) => format_unit_decl(fmt, d),
        DeclKind::Type(d) => format_type_decl(fmt, d),
        DeclKind::UnionType(d) => format_union_type_decl(fmt, d),
        DeclKind::Index(d) => format_index_decl(fmt, d),
        DeclKind::Import(d) => format_import_decl(fmt, d),
        DeclKind::Include(d) => format_include_decl(fmt, d),
        DeclKind::Dag(d) => format_dag_decl(fmt, d),
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

fn format_attribute_arg(arg: &graphcal_compiler::syntax::ast::AttributeArg) -> RcDoc<'static> {
    match arg {
        graphcal_compiler::syntax::ast::AttributeArg::Path { segments, .. } => {
            let parts: Vec<RcDoc<'static>> = segments
                .iter()
                .map(|s| RcDoc::text(s.name.clone()))
                .collect();
            RcDoc::intersperse(parts, RcDoc::text("::"))
        }
        graphcal_compiler::syntax::ast::AttributeArg::Group { elements, .. } => {
            let inner: Vec<RcDoc<'static>> = elements.iter().map(format_attribute_arg).collect();
            RcDoc::text("(")
                .append(RcDoc::intersperse(inner, RcDoc::text(", ")))
                .append(RcDoc::text(")"))
        }
    }
}

/// `param name: Type = expr;` or `param name: Type;` (required param)
fn format_param_decl(fmt: &mut Formatter<'_>, d: &ParamDecl) -> RcDoc<'static> {
    match d.value.as_ref() {
        None => RcDoc::text("param")
            .append(RcDoc::text(" "))
            .append(RcDoc::text(d.name.value.as_str().to_string()))
            .append(RcDoc::text(": "))
            .append(format_type_expr_inline(fmt, &d.type_ann))
            .append(RcDoc::text(";")),
        Some(value) => format_value_decl(fmt, "param", &d.name.value, &d.type_ann, value),
    }
}

/// `node name: Type = expr;`
fn format_node_decl(fmt: &mut Formatter<'_>, d: &NodeDecl) -> RcDoc<'static> {
    format_value_decl(fmt, "node", &d.name.value, &d.type_ann, &d.value)
}

/// `const node name: Type = expr;`
fn format_const_node_decl(
    fmt: &mut Formatter<'_>,
    d: &graphcal_compiler::syntax::ast::ConstNodeDecl,
) -> RcDoc<'static> {
    format_value_decl(fmt, "const node", &d.name.value, &d.type_ann, &d.value)
}

/// Shared logic for param/node/const declarations.
fn format_value_decl(
    fmt: &mut Formatter<'_>,
    keyword: &str,
    name: &graphcal_compiler::syntax::names::DeclName,
    type_ann: &TypeExpr,
    value: &graphcal_compiler::syntax::ast::Expr,
) -> RcDoc<'static> {
    let header = RcDoc::text(keyword.to_string())
        .append(RcDoc::text(" "))
        .append(RcDoc::text(name.as_str().to_string()))
        .append(RcDoc::text(": "))
        .append(format_type_expr_inline(fmt, type_ann))
        .append(RcDoc::text(" = "));

    let val = format_expr(fmt, value);
    header.append(val).append(RcDoc::text(";"))
}

/// `base dim Name;`
fn format_base_dim_decl(d: &BaseDimDecl) -> RcDoc<'static> {
    RcDoc::text("base dim ")
        .append(RcDoc::text(d.name.value.as_str().to_string()))
        .append(RcDoc::text(";"))
}

/// `dim Name = DimExpr;`
fn format_dim_decl(_fmt: &Formatter<'_>, d: &DimDecl) -> RcDoc<'static> {
    RcDoc::text("dim ")
        .append(RcDoc::text(d.name.value.as_str().to_string()))
        .append(RcDoc::text(" = "))
        .append(format_dim_expr_inline(&d.definition))
        .append(RcDoc::text(";"))
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
    use graphcal_compiler::syntax::ast::ExprKind;
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
fn format_type_decl(fmt: &mut Formatter<'_>, d: &TypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(fmt, &d.generic_params));
    }

    if d.fields.is_empty() {
        // Unit type or empty record type — format as `type Name;`
        return header.append(RcDoc::text(";"));
    }

    // Record type with fields
    let fields = format_field_decls(fmt, &d.fields);
    header
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(fields).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// `type Name = A | B | C;` or `type Name<D: Dim> = Ok<D> | Err;`
fn format_union_type_decl(fmt: &mut Formatter<'_>, d: &UnionTypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(fmt, &d.generic_params));
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
                    .map(|a| format_type_expr_inline(fmt, a))
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

fn format_field_decls(fmt: &mut Formatter<'_>, fields: &[FieldDecl]) -> RcDoc<'static> {
    let field_docs: Vec<RcDoc<'static>> = fields
        .iter()
        .map(|f| format_single_field_decl(fmt, f).append(RcDoc::text(",")))
        .collect();
    RcDoc::intersperse(field_docs, RcDoc::hardline())
}

fn format_single_field_decl(fmt: &mut Formatter<'_>, f: &FieldDecl) -> RcDoc<'static> {
    RcDoc::text(f.name.value.as_str().to_string())
        .append(RcDoc::text(": "))
        .append(format_type_expr_inline(fmt, &f.type_ann))
}

fn format_generic_params(fmt: &mut Formatter<'_>, params: &[GenericParam]) -> RcDoc<'static> {
    let param_docs: Vec<RcDoc<'static>> = params
        .iter()
        .map(|p| {
            let constraint = match p.constraint {
                GenericConstraint::Dim => "Dim",
                GenericConstraint::Index => "Index",
                GenericConstraint::Nat => "Nat",
                GenericConstraint::Type => "Type",
            };
            let mut doc = RcDoc::text(p.name.value.as_str().to_string())
                .append(RcDoc::text(": "))
                .append(RcDoc::text(constraint));
            if let Some(ref default) = p.default {
                doc = doc
                    .append(RcDoc::text(" = "))
                    .append(format_type_expr_inline(fmt, default));
            }
            doc
        })
        .collect();
    RcDoc::text("<")
        .append(RcDoc::intersperse(param_docs, RcDoc::text(", ")))
        .append(RcDoc::text(">"))
}

/// `index Name = { V1, V2, V3 };` or `index Name;` (required named)
/// or `index Name = linspace(start, end, step: step);` or `index Name: Dim;` (required range)
fn format_index_decl(fmt: &mut Formatter<'_>, d: &IndexDecl) -> RcDoc<'static> {
    match &d.kind {
        IndexDeclKind::RequiredNamed => RcDoc::text("index ")
            .append(RcDoc::text(d.name.value.as_str().to_string()))
            .append(RcDoc::text(";")),
        IndexDeclKind::RequiredRange { dimension } => RcDoc::text("index ")
            .append(RcDoc::text(d.name.value.as_str().to_string()))
            .append(RcDoc::text(": "))
            .append(format_dim_expr_inline(dimension))
            .append(RcDoc::text(";")),
        IndexDeclKind::Named { variants } => {
            let header =
                RcDoc::text("index ").append(RcDoc::text(d.name.value.as_str().to_string()));

            let variant_docs: Vec<RcDoc<'static>> = variants
                .iter()
                .map(|v| RcDoc::text(v.value.as_str().to_string()))
                .collect();

            let single_sep = RcDoc::text(", ");
            let single_line = header
                .clone()
                .append(RcDoc::text(" = { "))
                .append(RcDoc::intersperse(variant_docs.clone(), single_sep))
                .append(RcDoc::text(" };"));

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
                .append(RcDoc::text("};"));

            multi_line.flat_alt(single_line).group()
        }
        IndexDeclKind::Range { start, end, step } => {
            let header =
                RcDoc::text("index ").append(RcDoc::text(d.name.value.as_str().to_string()));

            header
                .append(RcDoc::text(" = linspace("))
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
fn format_import_decl(_fmt: &mut Formatter<'_>, d: &ImportDecl) -> RcDoc<'static> {
    let path_doc = format_import_or_include_path("import", &d.path);
    format_import_or_include_kind(path_doc, RcDoc::nil(), &d.kind)
}

/// `include "path"(x: 1.0 km) { name };` or `include "path" as alias;`
fn format_include_decl(fmt: &mut Formatter<'_>, d: &IncludeDecl) -> RcDoc<'static> {
    let path_doc = format_import_or_include_path("include", &d.path);
    let bindings_doc = format_import_param_bindings(fmt, &d.param_bindings);
    format_import_or_include_kind(path_doc, bindings_doc, &d.kind)
}

/// `dag name { declarations... }`
fn format_dag_decl(fmt: &mut Formatter<'_>, d: &DagDecl) -> RcDoc<'static> {
    let header = RcDoc::text(format!("dag {} {{", d.name.value.as_str()));
    if d.body.is_empty() {
        return RcDoc::text(format!("dag {} {{}}", d.name.value.as_str()));
    }
    let body_parts: Vec<RcDoc<'static>> =
        d.body.iter().map(|decl| format_decl(fmt, decl)).collect();
    let body = RcDoc::intersperse(body_parts, RcDoc::hardline().append(RcDoc::hardline()));
    header
        .append(RcDoc::hardline())
        .append(body.nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// Format the path portion of an import/include declaration.
fn format_import_or_include_path(
    keyword: &str,
    path: &graphcal_compiler::syntax::ast::ImportPath,
) -> RcDoc<'static> {
    match path {
        graphcal_compiler::syntax::ast::ImportPath::FilePath { path, .. } => {
            RcDoc::text(format!("{keyword} \"{path}\""))
        }
        graphcal_compiler::syntax::ast::ImportPath::ModulePath { segments, .. } => {
            let path_str = segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("/");
            RcDoc::text(format!("{keyword} {path_str}"))
        }
        graphcal_compiler::syntax::ast::ImportPath::ParentScope { levels, .. } => {
            let mut path_str = "..".to_string();
            for _ in 1..*levels {
                path_str.push_str("/..");
            }
            RcDoc::text(format!("{keyword} {path_str}"))
        }
        graphcal_compiler::syntax::ast::ImportPath::CrossFileDag {
            file_path,
            dag_name,
            ..
        } => RcDoc::text(format!("{keyword} \"{file_path}\"/{}", dag_name.name)),
    }
}

/// Format the kind portion (selective/module) of an import/include declaration.
fn format_import_or_include_kind(
    path_doc: RcDoc<'static>,
    bindings_doc: RcDoc<'static>,
    kind: &graphcal_compiler::syntax::ast::ImportKind,
) -> RcDoc<'static> {
    match kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
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
        graphcal_compiler::syntax::ast::ImportKind::Module { alias: None } => {
            path_doc.append(bindings_doc).append(RcDoc::text(";"))
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias: Some(a) } => path_doc
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
    mark: &graphcal_compiler::syntax::ast::MarkSpec,
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
    format_composition_decl(
        fmt,
        "figure",
        d.name.value.as_str(),
        &d.plot_names,
        &d.fields,
    )
}

fn format_layer_decl(fmt: &mut Formatter<'_>, d: &LayerDecl) -> RcDoc<'static> {
    format_composition_decl(
        fmt,
        "layer",
        d.name.value.as_str(),
        &d.plot_names,
        &d.fields,
    )
}

/// Shared formatter for `figure` and `layer` declarations (identical structure).
fn format_composition_decl(
    fmt: &mut Formatter<'_>,
    keyword: &str,
    name: &str,
    plot_names: &[graphcal_compiler::syntax::names::Spanned<
        graphcal_compiler::syntax::names::DeclName,
    >],
    fields: &[graphcal_compiler::syntax::ast::PlotField],
) -> RcDoc<'static> {
    let header = RcDoc::text(format!("{keyword} {name} = "));

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();

    // Emit `plots: [name1, name2],`
    if !plot_names.is_empty() {
        let names: Vec<RcDoc<'static>> = plot_names
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
    for f in fields {
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
