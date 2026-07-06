use graphcal_compiler::syntax::ast::{
    AssertBody, AssertDecl, Attribute, BaseDimDecl, DagDecl, DeclKind, Declaration, DimDecl,
    Encoding, Expr, FieldDecl, FigureDecl, GenericConstraint, GenericParam, ImportDecl,
    IncludeDecl, IndexDecl, IndexDeclKind, LayerDecl, MultiDecl, MultiHeaderCell, MultiSlotAxis,
    MultiSlotKind, NodeDecl, ParamBinding, ParamDecl, PlotDecl, TableIndexSpec, TypeDecl,
    TypeDeclBody, TypeExpr, UnitConstness, UnitDecl, UnitDef, Visibility,
};
use pretty::RcDoc;

use super::{
    Formatter, INDENT, display_width, flat_alt_group, format_dim_expr_inline, format_expr,
    format_type_expr_inline, format_unit_expr_inline, pad_left_to_width, pad_right_to_width,
    render_doc_to_string, soft_parenthesized, soft_parenthesized_list, text_with_hardlines,
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
        DeclKind::Dimension(d) => format_dim_decl(d),
        DeclKind::Unit(d) => format_unit_decl(fmt, d),
        DeclKind::Type(d) => format_type_decl(fmt, d),
        DeclKind::Index(d) => format_index_decl(fmt, d),
        DeclKind::Import(d) => format_import_decl(fmt, d),
        DeclKind::PluginImport(d) => format_plugin_import_decl(fmt, d),
        DeclKind::Include(d) => format_include_decl(fmt, d),
        DeclKind::Dag(d) => format_dag_decl(fmt, d),
        DeclKind::Assert(d) => format_assert_decl(fmt, d),
        DeclKind::Plot(d) => format_plot_decl(fmt, d),
        DeclKind::Figure(d) => format_figure_decl(fmt, d),
        DeclKind::Layer(d) => format_layer_decl(fmt, d),
        DeclKind::Sugar(graphcal_compiler::syntax::ast::RawDeclSugar::Multi(d)) => {
            format_multi_decl(fmt, d)
        }
    };

    // Prepend the visibility annotation (if any).
    let body = format_decl_visibility(&decl.kind).append(body);

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

fn format_decl_visibility(kind: &DeclKind) -> RcDoc<'static> {
    match kind {
        // Plugin imports carry no visibility annotation.
        DeclKind::Param(_) | DeclKind::Sugar(_) | DeclKind::PluginImport(_) => RcDoc::nil(),
        DeclKind::Dimension(d) => bindable_visibility_prefix(d.visibility),
        DeclKind::Type(d) => bindable_visibility_prefix(d.visibility),
        DeclKind::Index(d) => bindable_visibility_prefix(d.visibility),
        DeclKind::Node(d) | DeclKind::ConstNode(d) => visibility_prefix(d.visibility),
        DeclKind::BaseDimension(d) => visibility_prefix(d.visibility),
        DeclKind::Unit(d) => visibility_prefix(d.visibility),
        DeclKind::Import(d) => visibility_prefix(d.visibility),
        DeclKind::Include(d) => visibility_prefix(d.visibility),
        DeclKind::Dag(d) => visibility_prefix(d.visibility),
        DeclKind::Assert(d) => visibility_prefix(d.visibility),
        DeclKind::Plot(d) => visibility_prefix(d.visibility),
        DeclKind::Figure(d) => visibility_prefix(d.visibility),
        DeclKind::Layer(d) => visibility_prefix(d.visibility),
    }
}

fn bindable_visibility_prefix(
    visibility: graphcal_compiler::syntax::ast::BindableVisibility,
) -> RcDoc<'static> {
    match visibility {
        graphcal_compiler::syntax::ast::BindableVisibility::Private => RcDoc::nil(),
        graphcal_compiler::syntax::ast::BindableVisibility::Public => RcDoc::text("pub "),
        graphcal_compiler::syntax::ast::BindableVisibility::PublicBind => RcDoc::text("pub(bind) "),
    }
}

fn visibility_prefix(visibility: Visibility) -> RcDoc<'static> {
    match visibility {
        Visibility::Private => RcDoc::nil(),
        Visibility::Public => RcDoc::text("pub "),
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
            RcDoc::intersperse(parts, RcDoc::text("."))
        }
        graphcal_compiler::syntax::ast::AttributeArg::RangeStep { step, .. } => {
            RcDoc::text(format!("#{step}"))
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
    name: &graphcal_compiler::syntax::decl_name::DeclName,
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

/// `dim Name = DimExpr;` or `dim Name;` (required).
fn format_dim_decl(d: &DimDecl) -> RcDoc<'static> {
    let head = RcDoc::text("dim ").append(RcDoc::text(d.name.value.as_str().to_string()));
    match &d.definition {
        Some(def) => head
            .append(RcDoc::text(" = "))
            .append(format_dim_expr_inline(def))
            .append(RcDoc::text(";")),
        None => head.append(RcDoc::text(";")),
    }
}

/// `const unit name: Dim = scale unit_expr;`, `unit name: Dim = ...`, or `base unit name: Dim;`.
fn format_unit_decl(fmt: &mut Formatter<'_>, d: &UnitDecl) -> RcDoc<'static> {
    let head = match (&d.definition, d.constness) {
        (None, _) => RcDoc::text("base unit "),
        (Some(_), UnitConstness::Const) => RcDoc::text("const unit "),
        (Some(_), UnitConstness::Dynamic) => RcDoc::text("unit "),
    };
    let mut doc = head
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
        _ => soft_parenthesized(format_expr(fmt, &def.scale_expr)),
    };
    scale_doc
        .append(RcDoc::text(" "))
        .append(format_unit_expr_inline(&def.unit_expr))
}

/// `type Name;` (required) or `type Name { Ctor(field: Type, ...), Ctor, ... }`.
fn format_type_decl(fmt: &mut Formatter<'_>, d: &TypeDecl) -> RcDoc<'static> {
    let mut header = RcDoc::text("type ").append(RcDoc::text(d.name.value.as_str().to_string()));

    if !d.generic_params.is_empty() {
        header = header.append(format_generic_params(fmt, &d.generic_params));
    }

    let TypeDeclBody::Constructors(members) = &d.body else {
        return header.append(RcDoc::text(";"));
    };

    let member_docs: Vec<RcDoc<'static>> = members
        .iter()
        .map(|m| {
            let mut doc = RcDoc::text(m.name.value.as_str().to_string());
            if let Some(fields) = &m.payload {
                if fields.is_empty() {
                    doc = doc.append(RcDoc::text("()"));
                } else {
                    let field_docs: Vec<RcDoc<'static>> = fields
                        .iter()
                        .map(|f| format_single_field_decl(fmt, f))
                        .collect();
                    doc = doc
                        .append(RcDoc::text("("))
                        .append(RcDoc::intersperse(field_docs, RcDoc::text(", ")))
                        .append(RcDoc::text(")"));
                }
            }
            doc.append(RcDoc::text(","))
        })
        .collect();

    let body = RcDoc::intersperse(member_docs, RcDoc::hardline());
    header
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(body).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// `import plugin "path" as alias { fn name<D: Dim>(p: T, ...) -> T; ... }`
fn format_plugin_import_decl(
    fmt: &mut Formatter<'_>,
    d: &graphcal_compiler::syntax::ast::PluginImportDecl,
) -> RcDoc<'static> {
    let header = RcDoc::text("import plugin \"")
        .append(RcDoc::text(d.path.value.as_str().to_string()))
        .append(RcDoc::text("\" as "))
        .append(RcDoc::text(d.alias.value.as_str().to_string()));

    if d.functions.is_empty() {
        return header.append(RcDoc::text(" {}"));
    }

    let fn_docs: Vec<RcDoc<'static>> = d
        .functions
        .iter()
        .map(|f| format_extern_fn_decl(fmt, f))
        .collect();
    let body = RcDoc::intersperse(fn_docs, RcDoc::hardline());
    header
        .append(RcDoc::text(" {"))
        .append(RcDoc::hardline().append(body).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// `fn smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I];`
fn format_extern_fn_decl(
    fmt: &mut Formatter<'_>,
    f: &graphcal_compiler::syntax::ast::ExternFnDecl,
) -> RcDoc<'static> {
    use graphcal_compiler::syntax::ast::ExternGenericBinder;

    let mut doc = RcDoc::text("fn ").append(RcDoc::text(f.name.value.as_str().to_string()));
    if !f.generics.is_empty() {
        let var_docs: Vec<RcDoc<'static>> = f
            .generics
            .iter()
            .map(|binder| {
                let constraint = match binder {
                    ExternGenericBinder::Dim(_) => "Dim",
                    ExternGenericBinder::Index(_) => "Index",
                };
                RcDoc::text(format!("{}: {constraint}", binder.name_str()))
            })
            .collect();
        doc = doc
            .append(RcDoc::text("<"))
            .append(RcDoc::intersperse(var_docs, RcDoc::text(", ")))
            .append(RcDoc::text(">"));
    }
    let param_docs: Vec<RcDoc<'static>> = f
        .params
        .iter()
        .map(|p| {
            RcDoc::text(p.name.value.as_str().to_string())
                .append(RcDoc::text(": "))
                .append(format_type_expr_inline(fmt, &p.type_ann))
        })
        .collect();
    doc.append(RcDoc::text("("))
        .append(RcDoc::intersperse(param_docs, RcDoc::text(", ")))
        .append(RcDoc::text(") -> "))
        .append(format_type_expr_inline(fmt, &f.result))
        .append(RcDoc::text(";"))
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

            flat_alt_group(single_line, multi_line)
        }
        IndexDeclKind::Range { start, end, step } => {
            let header =
                RcDoc::text("index ").append(RcDoc::text(d.name.value.as_str().to_string()));

            let args = vec![
                format_expr(fmt, start),
                format_expr(fmt, end),
                RcDoc::text("step: ").append(format_expr(fmt, step)),
            ];

            header
                .append(RcDoc::text(" = linspace"))
                .append(soft_parenthesized_list(args, false))
                .append(RcDoc::text(";"))
        }
    }
}

/// `import "path" { name1, name2 };` or `import "path";` or `import "path" as alias;`
fn format_import_decl(_fmt: &mut Formatter<'_>, d: &ImportDecl) -> RcDoc<'static> {
    let path_doc = format_import_or_include_path("import", &d.path);
    format_import_or_include_kind(path_doc, RcDoc::nil(), &d.kind)
}

/// `include path(x: 1.0 km).{ name };` or `include path() as alias;`.
///
/// `include` always emits `(...)` — even when the binding list is empty —
/// because the param-binding parens are part of the include grammar (the
/// parser requires them). `import` uses the same shared bindings helper but
/// never has bindings, so it emits nothing.
fn format_include_decl(fmt: &mut Formatter<'_>, d: &IncludeDecl) -> RcDoc<'static> {
    let path_doc = format_import_or_include_path("include", &d.path);
    let bindings_doc = format_include_param_bindings(fmt, &d.param_bindings);
    format_import_or_include_kind(path_doc, bindings_doc, &d.kind)
}

/// `dag name { declarations... }`
fn format_dag_decl(fmt: &mut Formatter<'_>, d: &DagDecl) -> RcDoc<'static> {
    let header = RcDoc::text(format!("dag {} {{", d.name.value.as_str()));
    if d.body.is_empty() {
        return RcDoc::text(format!("dag {} {{}}", d.name.value.as_str()));
    }
    let body = RcDoc::concat(super::format_decl_sequence(fmt, &d.body));
    header
        .append(RcDoc::hardline().append(body).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("}"))
}

/// Format the path portion of an import/include declaration.
fn format_import_or_include_path(
    keyword: &str,
    path: &graphcal_compiler::syntax::ast::ModulePath,
) -> RcDoc<'static> {
    let path_str = path
        .segments
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    RcDoc::text(format!("{keyword} {path_str}"))
}

/// Format a selective import/include suffix (`.{ ... };`).
///
/// The selector is a suffix of the import/include head, but it must choose its
/// own layout after the head has been rendered. A multiline include binding
/// list should not force a single selected output into a brace block; conversely
/// a long one-line head should still make the selector break when it does not
/// fit in the remaining width. Appending a grouped suffix to the head gives the
/// pretty printer exactly that local decision point.
fn format_selective_import_or_include(
    head: RcDoc<'static>,
    item_docs: Vec<RcDoc<'static>>,
) -> RcDoc<'static> {
    head.append(format_selective_import_or_include_suffix(item_docs))
}

fn format_selective_import_or_include_suffix(item_docs: Vec<RcDoc<'static>>) -> RcDoc<'static> {
    if item_docs.is_empty() {
        return RcDoc::text(".{ };");
    }

    let single_line = RcDoc::text(".{ ")
        .append(RcDoc::intersperse(item_docs.clone(), RcDoc::text(", ")))
        .append(RcDoc::text(" };"));

    let multi_items = RcDoc::intersperse(item_docs, RcDoc::text(",").append(RcDoc::line())).group();
    let multi_line = RcDoc::text(".{")
        .append(RcDoc::hardline().append(multi_items).nest(INDENT))
        .append(RcDoc::hardline())
        .append(RcDoc::text("};"));

    flat_alt_group(single_line, multi_line)
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
                    if item.is_pub {
                        doc = doc.append(RcDoc::text("pub "));
                    }
                    if item.namespace == graphcal_compiler::syntax::ast::ImportItemNamespace::Type {
                        doc = doc.append(RcDoc::text("type "));
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
            format_selective_import_or_include(path_doc.append(bindings_doc), name_docs)
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias: None } => {
            path_doc.append(bindings_doc).append(RcDoc::text(";"))
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias: Some(a) } => path_doc
            .append(bindings_doc)
            .append(RcDoc::text(format!(" as {};", a.value))),
    }
}

/// Format `include` param bindings: always `(name: expr, ...)` or `()`.
///
/// `include` always carries a param-binding list — empty `()` is valid and
/// must round-trip through the formatter. (The parser requires the parens.)
fn format_include_param_bindings(
    fmt: &mut Formatter<'_>,
    bindings: &[ParamBinding],
) -> RcDoc<'static> {
    let binding_docs: Vec<RcDoc<'static>> = bindings
        .iter()
        .map(|b| {
            RcDoc::text(b.name.name.clone())
                .append(RcDoc::text(": "))
                .append(format_expr(fmt, &b.value))
        })
        .collect();
    soft_parenthesized_list(binding_docs, false)
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
            RcDoc::text(f.name.value.to_string())
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
                RcDoc::text(f.name.value.to_string())
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
    plot_names: &[graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::module_name::ScopedName,
    >],
    fields: &[graphcal_compiler::syntax::ast::PlotField],
) -> RcDoc<'static> {
    let header = RcDoc::text(format!("{keyword} {name} = "));

    let mut field_docs: Vec<RcDoc<'static>> = Vec::new();

    // Emit `plots: [name1, name2],`
    if !plot_names.is_empty() {
        let names: Vec<RcDoc<'static>> = plot_names
            .iter()
            .map(|p| RcDoc::text(p.value.to_string()))
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
            RcDoc::text(f.name.value.to_string())
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

// ---------------------------------------------------------------------------
// Multi-declaration (issue #481)
// ---------------------------------------------------------------------------

/// Format a multi-decl surface form as a single declaration with
/// canonicalized column alignment in both the slot header list and the
/// table body. Output is deterministic and idempotent.
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive routine for multi-decl surface rendering"
)]
pub fn format_multi_decl(fmt: &mut Formatter<'_>, info: &MultiDecl) -> RcDoc<'static> {
    use std::fmt::Write as _;
    let mut out = String::new();

    // Slot headers: `<vis_kind_padded> <name:_padded> <type><sep>`
    // Where `name:` is padded so all types line up. Visibility (`pub` /
    // `pub(bind)`) joins the kind keyword for alignment so per-slot prefixes
    // line up with bare slots.
    let kind_strs: Vec<String> = info
        .slots
        .iter()
        .map(|s| {
            let kind = match s.kind {
                MultiSlotKind::Param => "param",
                MultiSlotKind::Node => "node",
                MultiSlotKind::ConstNode => "const node",
            };
            match s.visibility {
                Visibility::Private => kind.to_string(),
                Visibility::Public => format!("pub {kind}"),
            }
        })
        .collect();
    let max_kind = kind_strs.iter().map(String::len).max().unwrap_or(0);

    let name_colon_strs: Vec<String> = info
        .slots
        .iter()
        .map(|s| format!("{}:", s.name.value.as_str()))
        .collect();
    let max_name_colon = name_colon_strs.iter().map(String::len).max().unwrap_or(0);

    let type_strs: Vec<String> = info
        .slots
        .iter()
        .map(|s| render_doc_to_string(&format_type_expr_inline(fmt, &s.type_ann)))
        .collect();

    for idx in 0..info.slots.len() {
        if idx > 0 {
            out.push('\n');
        }
        let sep = if idx + 1 == info.slots.len() { "" } else { "," };
        let _ = write!(
            out,
            "{:<kw$} {:<nw$} {}{}",
            kind_strs[idx],
            name_colon_strs[idx],
            type_strs[idx],
            sep,
            kw = max_kind,
            nw = max_name_colon,
        );
    }

    // Table expression: `= table[shared, (slots)] { ... };`
    let shared_axes_str = info
        .shared_axes
        .iter()
        .map(|spec| match spec {
            TableIndexSpec::Named(s) => s.value.to_string(),
            TableIndexSpec::NatRange(n, _) => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let slot_axes_str = info
        .slot_axes
        .iter()
        .map(|a| match a {
            MultiSlotAxis::Underscore => "_".to_string(),
            MultiSlotAxis::Axis(s) => s.value.as_str().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    out.push('\n');
    out.push_str(&" ".repeat(INDENT as usize));
    let _ = write!(out, "= table[{shared_axes_str}, ({slot_axes_str})] {{");

    // Body.
    let body_indent = " ".repeat((INDENT * 2) as usize);
    let (max_row_label, col_widths, rendered_rows) = compute_multi_decl_layout(fmt, info);

    for (si, slice) in info.slices.iter().enumerate() {
        if si > 0 {
            out.push('\n');
        }
        // Slice prefix `[A.a, B.b]`.
        if !slice.prefix_keys.is_empty() {
            let labels = slice
                .prefix_keys
                .iter()
                .map(|k| format!("{}.{}", k.index.value, k.variant.value.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            out.push('\n');
            out.push_str(&body_indent);
            let _ = write!(out, "[{labels}]");
        }
        // Header row: `<row-label-padding>: <cells>;`
        let header_cells: Vec<String> = slice
            .header_cells
            .iter()
            .enumerate()
            .map(|(ci, cell)| {
                pad_left_to_width(
                    &header_cell_text(cell),
                    col_widths.get(ci).copied().unwrap_or(0),
                )
            })
            .collect();
        out.push('\n');
        out.push_str(&body_indent);
        let _ = write!(
            out,
            "{}: {};",
            " ".repeat(max_row_label),
            header_cells.join(", ")
        );
        for (ri, row) in slice.rows.iter().enumerate() {
            let cells: Vec<String> = rendered_rows[si][ri]
                .iter()
                .enumerate()
                .map(|(ci, text)| pad_left_to_width(text, col_widths.get(ci).copied().unwrap_or(0)))
                .collect();
            out.push('\n');
            out.push_str(&body_indent);
            let _ = write!(
                out,
                "{}: {};",
                pad_right_to_width(row.label.value.as_str(), max_row_label),
                cells.join(", "),
            );
        }
    }

    out.push('\n');
    out.push_str(&" ".repeat(INDENT as usize));
    out.push_str("};");

    text_with_hardlines(&out)
}

fn render_multi_decl_cell_value(fmt: &Formatter<'_>, expr: &Expr) -> String {
    let mut cell_fmt = fmt.fork_skipping_comments_before(expr.span.offset());
    render_doc_to_string(&format_expr(&mut cell_fmt, expr))
}

fn compute_multi_decl_layout(
    fmt: &Formatter<'_>,
    info: &MultiDecl,
) -> (usize, Vec<usize>, Vec<Vec<Vec<String>>>) {
    let num_cols = info.slices.first().map_or(0, |s| s.header_cells.len());
    let mut col_widths: Vec<usize> = vec![0; num_cols];

    // Header cell widths.
    for slice in &info.slices {
        for (ci, cell) in slice.header_cells.iter().enumerate() {
            let text = header_cell_text(cell);
            let width = display_width(&text);
            if ci < col_widths.len() && width > col_widths[ci] {
                col_widths[ci] = width;
            }
        }
    }
    // Data cell widths (render once; reuse strings below).
    let rendered_rows: Vec<Vec<Vec<String>>> = info
        .slices
        .iter()
        .map(|slice| {
            slice
                .rows
                .iter()
                .map(|row| {
                    row.values
                        .iter()
                        .map(|v| render_multi_decl_cell_value(fmt, v))
                        .collect()
                })
                .collect()
        })
        .collect();
    for slice_rows in &rendered_rows {
        for row_cells in slice_rows {
            for (ci, cell) in row_cells.iter().enumerate() {
                let width = display_width(cell);
                if ci < col_widths.len() && width > col_widths[ci] {
                    col_widths[ci] = width;
                }
            }
        }
    }

    let max_row_label = info
        .slices
        .iter()
        .flat_map(|s| s.rows.iter())
        .map(|r| display_width(r.label.value.as_str()))
        .max()
        .unwrap_or(0);

    (max_row_label, col_widths, rendered_rows)
}

fn header_cell_text(cell: &MultiHeaderCell) -> String {
    match cell {
        MultiHeaderCell::Underscore { .. } => "_".to_string(),
        MultiHeaderCell::Variant { variant, .. } => variant.value.as_str().to_string(),
    }
}
