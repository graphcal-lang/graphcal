//! AST declaration collection: builds a [`SymbolTable`](super::SymbolTable) from a file's declarations.

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{
    AssertDecl, AttributeArg, BaseDimDecl, BindableVisibility, DagDecl, DeclKind, DimDecl, DimExpr,
    FigureDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamDecl, PlotDecl,
    TypeDecl, TypeDeclBody, TypeExprKind, UnitDecl, UnitExpr,
};
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::resolve::builder::SymbolTables;
use graphcal_compiler::resolve::symbols::SymbolRef;
use graphcal_compiler::syntax::ast::InputBindingCategory;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::names::{NameAtom, NamePath};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::GenericParamName;

use graphcal_compiler::builtin::{BuiltinConst, BuiltinFn};

use crate::nominal_type_index::NominalTypeIndex;
use crate::symbol_identity::{
    ExternFunctionId, FieldId, GenericParamId, IndexVariantId, ReferenceTarget, SourceSymbolPath,
    SymbolId, UnresolvedSymbol,
};

use super::hir_refs::HirRefCollector;
use super::index::{
    DefinitionInfo, ReferenceInfo, SymbolCategory, SymbolTable, TopLevelDefinition,
    dag_declaration_name,
};

/// Build a symbol table for a standalone buffer with no project context.
///
/// Constructs a file-local resolver (including inline dag child modules);
/// references into unloaded imports surface through the tolerant-lowering
/// fallback, keyed by their written spelling.
pub fn build_for_buffer(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
    source: &str,
) -> SymbolTable {
    let dag_id = DagId::root_in_package("test", "buffer");
    let resolver = file_local_resolver(ast, &dag_id);
    build_from_ast(ast, source, &dag_id, &resolver)
}

/// A resolver of one file and its inline DAGs alone, without import edges.
///
/// Editor features need every declaration of the file even while it has
/// errors, so each module keeps the declarations it can record and skips
/// only duplicates; a module that cannot be registered at all (and, when no
/// edge-free resolver can be completed, every module) falls back to the
/// spelling-keyed references of tolerant lowering.
pub fn file_local_resolver(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
    root: &DagId,
) -> ModuleResolver {
    fn add_modules<'a>(
        tables: &mut SymbolTables<'a>,
        owner: &DagId,
        declarations: &'a [graphcal_compiler::desugar::desugared_ast::Declaration],
    ) {
        // Skipped duplicates are diagnosed by compilation, not here.
        let _ = tables.add_module_lenient(owner.clone(), declarations);
        for decl in declarations {
            if let DeclKind::Dag(dag) = &decl.kind {
                add_modules(
                    tables,
                    &owner.inline_dag_child(dag.name.value.clone()),
                    &dag.body,
                );
            }
        }
    }

    let mut tables = SymbolTables::default();
    add_modules(&mut tables, root, &ast.declarations);
    tables.freeze_without_edges().unwrap_or_default()
}

/// Build a symbol table from a parsed AST file.
///
/// `source` is the text the `ast` was parsed from — it is used to precompute
/// LSP `Position`s for inlay-hint candidates so the request path avoids
/// O(source) scans.
pub fn build_from_ast(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
    source: &str,
    dag_id: &DagId,
    resolver: &ModuleResolver,
) -> SymbolTable {
    let mut table = SymbolTable::new(dag_id.clone());
    register_builtins(&mut table);
    let nominal_types = NominalTypeIndex::build(&ast.declarations, dag_id, resolver);
    collect_declarations(
        &ast.declarations,
        source,
        dag_id,
        resolver,
        &nominal_types,
        &mut table,
    );

    // Sort references by offset for binary search, and drop duplicates:
    // every row of a desugared `table[Axis] { ... }` records an index
    // reference at the same axis-token span, but each occurrence should
    // appear once in find-references/rename results.
    table
        .references
        .sort_by_key(|r| (r.span.offset(), r.span.len()));
    let mut seen = std::collections::HashSet::new();
    table
        .references
        .retain(|r| seen.insert((r.span.offset(), r.span.len(), r.target.clone())));
    // Build the sorted `defs_by_name_span` index for O(log n) lookups and
    // precompute inlay-hint candidate positions.
    table.finalize(source);
    apply_doc_comments(&ast.declarations, &mut table);
    table
}

/// Copy each declaration's attached `///` doc block onto the definitions
/// registered for it. Value declarations also match their name span: expanded
/// multi-decl slots share a surface span but have independent documentation.
fn apply_doc_comments(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    table: &mut SymbolTable,
) {
    for declaration in declarations {
        if let Some(doc) = &declaration.doc {
            let name_span = match &declaration.kind {
                DeclKind::Param(param) => Some(param.name.span),
                DeclKind::Node(node) | DeclKind::ConstNode(node) => Some(node.name.span),
                _ => None,
            };
            for definition in table.definitions.values_mut() {
                if definition.decl_span == declaration.span
                    && name_span.is_none_or(|span| definition.name_span == span)
                    && definition.doc.is_none()
                {
                    definition.doc = Some(doc.text().to_string());
                }
            }
        }
        if let DeclKind::Dag(dag) = &declaration.kind {
            apply_doc_comments(&dag.body, table);
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive declaration dispatch keeps owner recursion visibly complete"
)]
fn collect_declarations(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    source: &str,
    owner: &DagId,
    resolver: &ModuleResolver,
    nominal_types: &NominalTypeIndex,
    table: &mut SymbolTable,
) {
    let mut refs = HirRefCollector::new(owner, resolver, nominal_types);
    for declaration in declarations {
        collect_attribute_refs(&declaration.attributes, table, &refs);
        match &declaration.kind {
            DeclKind::Param(param) => collect_param_decl(
                param,
                declaration.span,
                BindableVisibility::PublicBind,
                table,
                &mut refs,
            ),
            DeclKind::Node(node) => collect_node_decl(
                node,
                declaration.span,
                node.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::ConstNode(constant) => collect_const_node_decl(
                constant,
                declaration.span,
                constant.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::BaseDimension(dimension) => collect_base_dim_decl(
                dimension,
                declaration.span,
                dimension.visibility.into(),
                table,
                &refs,
            ),
            DeclKind::Dimension(dimension) => collect_dim_decl(
                dimension,
                declaration.span,
                dimension.visibility,
                table,
                &refs,
            ),
            DeclKind::Unit(unit) => collect_unit_decl(
                unit,
                declaration.span,
                unit.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Type(type_decl) => collect_type_decl(
                type_decl,
                declaration.span,
                type_decl.visibility,
                table,
                &mut refs,
            ),
            DeclKind::Index(index) => {
                collect_index_decl(index, declaration.span, index.visibility, table, &mut refs);
            }
            DeclKind::Assert(assertion) => collect_assert_decl(
                assertion,
                declaration.span,
                assertion.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Plot(plot) => collect_plot_decl(
                plot,
                declaration.span,
                plot.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Figure(figure) => collect_figure_decl(
                figure,
                declaration.span,
                figure.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Layer(layer) => collect_layer_decl(
                layer,
                declaration.span,
                layer.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Import(import) => collect_import_decl(import, table),
            DeclKind::PluginImport(plugin) => {
                collect_plugin_import_decl(plugin, source, table, &mut refs);
            }
            DeclKind::Include(include) => collect_include_decl(include, table, &mut refs),
            DeclKind::Dag(dag) => {
                collect_dag_decl(dag, declaration.span, dag.visibility.into(), table, &refs);
                collect_declarations(
                    &dag.body,
                    source,
                    &owner.inline_dag_child(dag.name.value.clone()),
                    resolver,
                    nominal_types,
                    table,
                );
            }
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(s) => match *s {},
        }
    }
}

pub(super) fn register_builtins(table: &mut SymbolTable) {
    for constant in BuiltinConst::ALL {
        let name = constant.as_str();
        table.insert_definition(
            SymbolId::BuiltinConstant(*constant),
            DefinitionInfo {
                doc: None,
                name: name.to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
                visibility: None,
            },
        );
    }

    for function in BuiltinFn::all() {
        let detail = function.entry().documented_arity().map_or_else(
            || "builtin".to_string(),
            |arity| format!("builtin, arity {arity}"),
        );
        table.insert_definition(
            SymbolId::BuiltinFunction(function),
            DefinitionInfo {
                doc: None,
                name: function.as_str().to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(detail),
                visibility: None,
            },
        );
    }
}

fn collect_attribute_refs(
    attributes: &[graphcal_compiler::desugar::desugared_ast::Attribute],
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    fn collect_argument(
        argument: &AttributeArg,
        table: &mut SymbolTable,
        refs: &HirRefCollector<'_>,
    ) {
        match argument {
            AttributeArg::Path { path } => {
                table.references.push(ReferenceInfo {
                    span: path.span,
                    target: refs.declaration_target(&path.value),
                });
            }
            AttributeArg::Group { elements, .. } => {
                for element in elements {
                    collect_argument(element, table, refs);
                }
            }
            AttributeArg::IndexLabel { index, label, .. } => {
                refs.attribute_label(index, label, table);
            }
            AttributeArg::FinitePosition { .. } => {}
        }
    }

    for attribute in attributes {
        if matches!(
            attribute.name.name.as_str().parse::<AttributeName>(),
            Ok(AttributeName::Assumes | AttributeName::ExpectedFail)
        ) {
            for argument in &attribute.args {
                collect_argument(argument, table, refs);
            }
        }
    }
}

fn collect_param_decl(
    p: &ParamDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&p.name.value) {
        table.register_definition(
            TopLevelDefinition::Param(identity),
            p.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
    collect_type_expr_refs(&p.type_ann, table, refs);
    if let Some(ref value) = p.value {
        refs.collect_body(value, table);
    }
}

fn collect_node_decl(
    n: &NodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&n.name.value) {
        table.register_definition(
            TopLevelDefinition::Node(identity),
            n.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
    collect_type_expr_refs(&n.type_ann, table, refs);
    for hole in graphcal_compiler::syntax::visitor::collect_todo_holes(&n.value) {
        collect_type_expr_refs(&hole.value.ty, table, refs);
    }
    refs.collect_formula_body(&n.value, table);
}

fn collect_const_node_decl(
    c: &graphcal_compiler::desugar::desugared_ast::ConstNodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&c.name.value) {
        table.register_definition(
            TopLevelDefinition::Const(identity),
            c.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
    collect_type_expr_refs(&c.type_ann, table, refs);
    for hole in graphcal_compiler::syntax::visitor::collect_todo_holes(&c.value) {
        collect_type_expr_refs(&hole.value.ty, table, refs);
    }
    refs.collect_formula_body(&c.value, table);
}

fn collect_base_dim_decl(
    d: &BaseDimDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&d.name.value) {
        table.register_definition(
            TopLevelDefinition::Dimension(identity),
            d.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
}

fn collect_dim_decl(
    d: &DimDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&d.name.value) {
        table.register_definition(
            TopLevelDefinition::Dimension(identity),
            d.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
    if let Some(definition) = &d.definition {
        collect_dim_expr_refs(definition, table);
    }
}

fn collect_unit_decl(
    u: &UnitDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&u.name.value) {
        table.register_definition(
            TopLevelDefinition::Unit(identity),
            u.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
    collect_dim_expr_refs(&u.dim_type, table);
    if let Some(unit_def) = u.definition() {
        refs.collect_body(&unit_def.scale_expr, table);
        collect_unit_expr_refs(&unit_def.unit_expr, table);
    }
}

fn collect_type_decl(
    t: &TypeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    let Some(type_id) = refs.declared(&t.name.value) else {
        return;
    };
    table.register_definition(
        TopLevelDefinition::StructType(type_id.clone()),
        t.name.span,
        decl_span,
        None,
        None,
        visibility,
    );

    let generic_scope = t
        .generic_params
        .iter()
        .map(|param| {
            let key = SymbolId::GenericParam(GenericParamId::new(
                type_id.clone(),
                param.name.value.clone(),
            ));
            table.insert_definition(
                key.clone(),
                DefinitionInfo {
                    doc: None,
                    name: param.name.value.to_string(),
                    category: SymbolCategory::GenericParam,
                    name_span: param.name.span,
                    decl_span: param.name.span,
                    type_description: Some(generic_constraint_name(param.constraint).to_string()),
                    detail: Some(format!("generic parameter of {}", t.name.value)),
                    visibility: Some(t.visibility),
                },
            );
            (param.name.value.clone(), key)
        })
        .collect::<GenericParamSymbolScope>();

    let mut earlier_params = GenericParamSymbolScope::new();
    for param in &t.generic_params {
        if let Some(default) = &param.default {
            collect_generic_arg_refs(default, Some(&earlier_params), table, refs);
        }
        if let Some(key) = generic_scope.get(&param.name.value) {
            earlier_params.insert(param.name.value.clone(), key.clone());
        }
    }

    // Register constructor and payload-field definitions, then walk payload
    // type annotations (required types have no fields). Constructors are a
    // separate namespace from type names, so they must not reuse the type's
    // `TopLevel` key.
    if let TypeDeclBody::Constructors(members) = &t.body {
        for member in members {
            let constructor_name = member.name.value.to_string();
            let constructor_id = type_id.constructor(member.name.value.clone());
            table.insert_definition(
                SymbolId::Constructor(constructor_id.clone()),
                DefinitionInfo {
                    doc: None,
                    name: constructor_name.clone(),
                    category: SymbolCategory::Constructor,
                    name_span: member.name.span,
                    decl_span: member.span,
                    type_description: Some(format!("constructor of {}", t.name.value)),
                    detail: None,
                    visibility: Some(t.visibility),
                },
            );
            if let Some(fields) = &member.payload {
                for field in fields {
                    table.insert_definition(
                        SymbolId::Field(FieldId::new(
                            constructor_id.clone(),
                            field.name.value.clone(),
                        )),
                        DefinitionInfo {
                            doc: None,
                            name: field.name.value.to_string(),
                            category: SymbolCategory::Field,
                            name_span: field.name.span,
                            decl_span: field.name.span,
                            type_description: None,
                            detail: Some(format!("field of {constructor_name}")),
                            visibility: Some(t.visibility),
                        },
                    );
                    collect_type_expr_refs_in_scope(
                        &field.type_ann,
                        Some(&generic_scope),
                        table,
                        refs,
                    );
                }
            }
        }
    }
}

const fn generic_constraint_name(
    constraint: graphcal_compiler::syntax::ast::GenericConstraint,
) -> &'static str {
    match constraint {
        graphcal_compiler::syntax::ast::GenericConstraint::Dim => "Dim",
        graphcal_compiler::syntax::ast::GenericConstraint::Index => "Index",
        graphcal_compiler::syntax::ast::GenericConstraint::Nat => "Nat",
        graphcal_compiler::syntax::ast::GenericConstraint::Type => "Type",
    }
}

fn collect_index_decl(
    idx: &IndexDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    let name = idx.name.value.to_string();
    let Some(index_id) = refs.declared(&idx.name.value) else {
        return;
    };
    table.register_definition(
        TopLevelDefinition::Index(index_id.clone()),
        idx.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
    match &idx.kind {
        IndexDeclKind::Named { variants } => {
            for variant in variants {
                let vname = variant.value.to_string();
                let key = SymbolId::IndexVariant(IndexVariantId::new(
                    index_id.clone(),
                    variant.value.clone(),
                ));
                table.insert_definition(
                    key,
                    DefinitionInfo {
                        doc: None,
                        name: vname,
                        category: SymbolCategory::IndexVariant,
                        name_span: variant.span,
                        decl_span: variant.span,
                        type_description: None,
                        detail: Some(format!("label/value variant of index {name}")),
                        visibility: Some(visibility),
                    },
                );
            }
        }
        IndexDeclKind::Range { start, end, step } => {
            refs.collect_body(start, table);
            refs.collect_body(end, table);
            refs.collect_body(step, table);
        }
        IndexDeclKind::Linspace { start, end, .. } => {
            refs.collect_body(start, table);
            refs.collect_body(end, table);
        }
        IndexDeclKind::RequiredCoordinate { dimension } => {
            collect_dim_expr_refs(dimension, table);
        }
        IndexDeclKind::RequiredNamed => {}
    }
}

fn collect_assert_decl(
    a: &AssertDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&a.name.value) {
        table.register_definition(
            TopLevelDefinition::Assert(identity),
            a.name.span,
            decl_span,
            Some("Bool".to_string()),
            Some("assert".to_string()),
            visibility,
        );
    }
    match &a.body {
        graphcal_compiler::desugar::desugared_ast::AssertBody::Expr(expr) => {
            refs.collect_body(expr, table);
        }
        graphcal_compiler::desugar::desugared_ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            ..
        } => {
            refs.collect_body(actual, table);
            refs.collect_body(expected, table);
            refs.collect_body(tolerance, table);
        }
    }
}

fn collect_plot_decl(
    p: &PlotDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&p.name.value) {
        table.register_definition(
            TopLevelDefinition::Plot(identity),
            p.name.span,
            decl_span,
            Some(format!("plot (mark: {})", p.mark.mark_type)),
            Some("plot".to_string()),
            visibility,
        );
    }
    for encoding in &p.encodings {
        refs.collect_body(&encoding.value, table);
    }
    for prop in &p.mark.properties {
        refs.collect_body(&prop.value, table);
    }
    for prop in &p.properties {
        refs.collect_body(&prop.value, table);
    }
}

fn collect_figure_decl(
    f: &FigureDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&f.name.value) {
        table.register_definition(
            TopLevelDefinition::Figure(identity),
            f.name.span,
            decl_span,
            Some("figure".to_string()),
            Some("figure".to_string()),
            visibility,
        );
    }
    for plot in &f.plot_names {
        let Some(path) = plot.value.to_name_path() else {
            continue;
        };
        table.references.push(ReferenceInfo {
            span: plot.span,
            target: refs.declaration_target(&path),
        });
    }
    for field in &f.fields {
        refs.collect_body(&field.value, table);
    }
}

fn collect_layer_decl(
    l: &LayerDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&l.name.value) {
        table.register_definition(
            TopLevelDefinition::Layer(identity),
            l.name.span,
            decl_span,
            Some("layer".to_string()),
            Some("layer".to_string()),
            visibility,
        );
    }
    for plot in &l.plot_names {
        let Some(path) = plot.value.to_name_path() else {
            continue;
        };
        table.references.push(ReferenceInfo {
            span: plot.span,
            target: refs.declaration_target(&path),
        });
    }
    for field in &l.fields {
        refs.collect_body(&field.value, table);
    }
}

fn collect_dag_decl(
    d: &DagDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    if let Some(identity) = refs.declared(&d.name.value) {
        table.register_definition(
            TopLevelDefinition::Dag(identity),
            d.name.span,
            decl_span,
            None,
            None,
            visibility,
        );
    }
}

fn collect_import_decl(u: &ImportDecl, table: &mut SymbolTable) {
    // Each imported name is a reference; target resolution for cross-file
    // go-to-definition is handled separately.
    if let ImportDecl::Selective { items, .. } = u {
        collect_selected_names(items, table);
    }
}

/// Register each extern function of an `import plugin` block as an
/// alias-qualified definition, so completion offers `alias::fn` and
/// go-to-definition lands on the `fn` name inside the block.
fn collect_plugin_import_decl(
    p: &graphcal_compiler::desugar::desugared_ast::PluginImportDecl,
    source: &str,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    for function in &p.functions {
        // The declared signature text, verbatim, is the best hover/detail
        // rendering: it is already in graphcal vocabulary.
        let signature = source
            .get(function.span.offset()..function.span.offset() + function.span.len())
            .map(|text| text.trim_end_matches(';').trim().to_string());
        table.insert_definition(
            SymbolId::ExternFunction(ExternFunctionId::new(
                refs.dag_id.clone(),
                p.alias.value.clone(),
                function.name.value.clone(),
            )),
            DefinitionInfo {
                doc: None,
                // The leaf alone: the alias is the typed `plugin` part of
                // the key, and display labels qualify it at the boundary.
                name: function.name.value.to_string(),
                category: SymbolCategory::ExternFn,
                name_span: function.name.span,
                decl_span: function.span,
                type_description: signature,
                detail: Some(format!("extern function (plugin \"{}\")", p.path.value)),
                visibility: None,
            },
        );
        for param in &function.params {
            collect_type_expr_refs(&param.type_ann, table, refs);
        }
        collect_type_expr_refs(&function.result, table, refs);
    }
}

fn collect_include_decl(
    include: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    let target = refs
        .resolver
        .resolve_module_path(refs.dag_id, &include.path)
        .ok();
    if let Some(target) = &target {
        if let Some(declaration) = dag_declaration_name(target) {
            table.references.push(ReferenceInfo {
                span: include.path.segments.last().span,
                target: SymbolId::Declaration(declaration).into(),
            });
        }
        // Bound inputs and selected items name what the template's own
        // scope binds under that spelling.
        let template_decl = |atom: &graphcal_compiler::syntax::names::NameAtom| {
            refs.resolver
                .resolve_decl_path(target, &NamePath::local(atom.clone()))
                .ok()
                .map(SymbolRef::into_resolved)
        };
        for binding in &include.param_bindings {
            let input = NamePath::local(binding.name.name.atom().clone());
            let input_target = match binding.category() {
                InputBindingCategory::Unmarked => {
                    template_decl(binding.name.name.atom()).map(SymbolId::Declaration)
                }
                InputBindingCategory::Index => refs
                    .resolver
                    .resolve_index_path(target, &input)
                    .ok()
                    .map(|index| SymbolId::Index(index.into_resolved())),
                InputBindingCategory::Dimension => refs
                    .resolver
                    .resolve_dimension_path(target, &input)
                    .ok()
                    .map(|dimension| SymbolId::Dimension(dimension.into_resolved())),
                InputBindingCategory::Type => refs
                    .resolver
                    .resolve_struct_type_path(target, &input)
                    .ok()
                    .map(|nominal| SymbolId::StructType(nominal.into_resolved())),
            };
            if let Some(input_target) = input_target {
                table.references.push(ReferenceInfo {
                    span: binding.name.span,
                    target: input_target.into(),
                });
            }
        }
        if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
            &include.kind
        {
            for item in items {
                if let Some(declaration) = template_decl(item.name.name.atom()) {
                    table.references.push(ReferenceInfo {
                        span: item.name.span,
                        target: SymbolId::Declaration(declaration).into(),
                    });
                }
                if let Some(alias) = &item.alias {
                    table.references.push(ReferenceInfo {
                        span: alias.span,
                        target: ReferenceTarget::Unresolved(UnresolvedSymbol::Declaration(
                            SourceSymbolPath::local(alias.name.atom().clone()),
                        )),
                    });
                }
            }
        }
    } else {
        match &include.kind {
            graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => {
                collect_selected_names(items, table);
            }
            graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => {}
        }
    }
    for binding in &include.param_bindings {
        collect_binding_value_refs(binding, table, refs);
    }
}

/// References in an include binding's value: an expression for a value
/// input, and a name of the bound category for a static input
/// (`index T: Step`, `dim D: Length`, `type T: Item`).
fn collect_binding_value_refs(
    binding: &graphcal_compiler::syntax::ast::ParamBinding<
        graphcal_compiler::syntax::phase::Desugared,
    >,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    use graphcal_compiler::syntax::ast::BindingValue;
    let unresolved: fn(SourceSymbolPath) -> UnresolvedSymbol = match &binding.value {
        BindingValue::Value(value) => {
            refs.collect_body(value, table);
            return;
        }
        BindingValue::Type(type_expr) => {
            if type_expr.as_lone_name().is_none() {
                collect_type_expr_refs(type_expr, table, refs);
                return;
            }
            UnresolvedSymbol::StructType
        }
        BindingValue::Index(_) => UnresolvedSymbol::Index,
        BindingValue::Dimension(dim) => {
            collect_dim_expr_refs(dim, table);
            return;
        }
    };
    if let Some(name) = binding.value.static_name() {
        table.references.push(ReferenceInfo {
            span: name.span,
            target: ReferenceTarget::Unresolved(unresolved(SourceSymbolPath::from_name_path(
                &name.value,
            ))),
        });
    }
}

fn collect_selected_names(
    names: &[graphcal_compiler::desugar::desugared_ast::ImportItem],
    table: &mut SymbolTable,
) {
    for import_item in names {
        let path = SourceSymbolPath::local(import_item.local_name_atom().clone());
        let unresolved = match import_item.namespace {
            graphcal_compiler::syntax::ast::ImportItemNamespace::Term => {
                UnresolvedSymbol::Term(path)
            }
            graphcal_compiler::syntax::ast::ImportItemNamespace::Type => {
                UnresolvedSymbol::StructType(path)
            }
            graphcal_compiler::syntax::ast::ImportItemNamespace::Dimension => {
                UnresolvedSymbol::Dimension(path)
            }
            graphcal_compiler::syntax::ast::ImportItemNamespace::Unit => {
                UnresolvedSymbol::Unit(path)
            }
            graphcal_compiler::syntax::ast::ImportItemNamespace::Index => {
                UnresolvedSymbol::Index(path)
            }
        };
        let target = ReferenceTarget::Unresolved(unresolved);
        table.references.push(ReferenceInfo {
            span: import_item.name.span,
            target: target.clone(),
        });
        if let Some(alias) = &import_item.alias {
            table.references.push(ReferenceInfo {
                span: alias.span,
                target,
            });
        }
    }
}

/// The symbol of each generic parameter in scope, by name.
type GenericParamSymbolScope = HashMap<GenericParamName, SymbolId>;

/// Collect references from a type expression outside a generic declaration.
fn collect_type_expr_refs(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    collect_type_expr_refs_in_scope(type_expr, None, table, refs);
}

fn collect_type_expr_refs_in_scope(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    match &type_expr.element.kind {
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
        TypeExprKind::DimExpr(dim_expr) => {
            collect_dim_expr_refs_in_scope(
                dim_expr,
                generic_scope,
                UnresolvedSymbol::TypeExpression,
                table,
            );
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: reference_target_in_generic_scope(
                    &name.value,
                    generic_scope,
                    UnresolvedSymbol::StructType,
                ),
            });
            for arg in generic_args {
                collect_generic_arg_refs(arg, generic_scope, table, refs);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args.iter().flatten() {
                collect_generic_arg_refs(arg, generic_scope, table, refs);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in; no top-level reference to record.
            // Recurse into args so any user-defined names reachable from the
            // time-scale expression are still picked up by go-to-definition.
            for arg in type_args {
                collect_type_expr_refs_in_scope(arg, generic_scope, table, refs);
            }
        }
    }
    // Collect references from domain constraint bound expressions (e.g., unit names in `100 kg`).
    for bound in &type_expr.element.constraints {
        refs.collect_body(&bound.value, table);
    }
    for idx in type_expr.indexes.iter().flatten() {
        match idx {
            graphcal_compiler::desugar::desugared_ast::IndexExpr::Name(path) => {
                table.references.push(ReferenceInfo {
                    span: path.span,
                    target: reference_target_in_generic_scope(
                        &path.value,
                        generic_scope,
                        UnresolvedSymbol::Index,
                    ),
                });
            }
            graphcal_compiler::desugar::desugared_ast::IndexExpr::Finite {
                cardinality, ..
            }
            | graphcal_compiler::desugar::desugared_ast::IndexExpr::BareNat(cardinality) => {
                collect_nat_generic_param_refs(cardinality, generic_scope, table);
            }
        }
    }
}

fn collect_generic_arg_refs(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    match arg {
        graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
            collect_type_expr_refs_in_scope(type_expr, generic_scope, table, refs);
        }
        graphcal_compiler::syntax::ast::GenericArg::Index(index) => match index {
            graphcal_compiler::syntax::ast::IndexExpr::Name(path) => {
                table.references.push(ReferenceInfo {
                    span: path.span,
                    target: reference_target_in_generic_scope(
                        &path.value,
                        generic_scope,
                        UnresolvedSymbol::Index,
                    ),
                });
            }
            graphcal_compiler::syntax::ast::IndexExpr::Finite { cardinality, .. }
            | graphcal_compiler::syntax::ast::IndexExpr::BareNat(cardinality) => {
                collect_nat_generic_param_refs(cardinality, generic_scope, table);
            }
        },
        graphcal_compiler::syntax::ast::GenericArg::Nat(expr) => {
            collect_nat_generic_param_refs(expr, generic_scope, table);
        }
        graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
            collect_ambiguous_generic_arg_refs(ambiguous, generic_scope, table);
        }
    }
}

fn collect_ambiguous_generic_arg_refs(
    arg: &graphcal_compiler::syntax::ast::AmbiguousGenericArg,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
) {
    match arg {
        graphcal_compiler::syntax::ast::AmbiguousGenericArg::Name(ident) => {
            table.references.push(ReferenceInfo {
                span: ident.span,
                target: generic_param_symbol(ident.name.atom(), generic_scope).map_or_else(
                    || {
                        ReferenceTarget::Unresolved(UnresolvedSymbol::GenericArgument(
                            SourceSymbolPath::local(ident.name.atom().clone()),
                        ))
                    },
                    ReferenceTarget::Resolved,
                ),
            });
        }
        graphcal_compiler::syntax::ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_arg_refs(operand, generic_scope, table);
            }
        }
    }
}

fn collect_nat_generic_param_refs(
    expr: &graphcal_compiler::syntax::ast::NatExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
) {
    match expr {
        graphcal_compiler::syntax::ast::NatExpr::Literal(..) => {}
        graphcal_compiler::syntax::ast::NatExpr::Var(ident) => {
            if let Some(target) = generic_param_symbol(ident.name.atom(), generic_scope) {
                table.references.push(ReferenceInfo {
                    span: ident.span,
                    target: target.into(),
                });
            }
        }
        graphcal_compiler::syntax::ast::NatExpr::Add(operands, _)
        | graphcal_compiler::syntax::ast::NatExpr::Mul(operands, _) => {
            for operand in operands {
                collect_nat_generic_param_refs(operand, generic_scope, table);
            }
        }
    }
}

fn generic_param_symbol(
    name: &NameAtom,
    generic_scope: Option<&GenericParamSymbolScope>,
) -> Option<SymbolId> {
    generic_scope?
        .get(&GenericParamName::classify(name.clone()))
        .cloned()
}

fn reference_target_in_generic_scope(
    path: &NamePath,
    generic_scope: Option<&GenericParamSymbolScope>,
    unresolved: fn(SourceSymbolPath) -> UnresolvedSymbol,
) -> ReferenceTarget {
    if path.qualifier_and_leaf().is_none()
        && let Some(target) = generic_param_symbol(path.leaf(), generic_scope)
    {
        return ReferenceTarget::Resolved(target);
    }
    ReferenceTarget::Unresolved(unresolved(SourceSymbolPath::from_name_path(path)))
}

/// Collect references from a dimension expression.
fn collect_dim_expr_refs(dim_expr: &DimExpr, table: &mut SymbolTable) {
    collect_dim_expr_refs_in_scope(dim_expr, None, UnresolvedSymbol::Dimension, table);
}

fn collect_dim_expr_refs_in_scope(
    dim_expr: &DimExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    unresolved: fn(SourceSymbolPath) -> UnresolvedSymbol,
    table: &mut SymbolTable,
) {
    for item in &dim_expr.terms {
        // The built-in `Dimensionless` term references no declaration.
        let Some(path) = item.term.name.as_path() else {
            continue;
        };
        table.references.push(ReferenceInfo {
            span: item.term.span,
            target: reference_target_in_generic_scope(&path.value, generic_scope, unresolved),
        });
    }
}

/// Collect references from a syntax-layer unit expression.
fn collect_unit_expr_refs(unit_expr: &UnitExpr, table: &mut SymbolTable) {
    for item in &unit_expr.terms {
        let qualifier = item.name.value.qualifier().to_vec();
        let path =
            SourceSymbolPath::module_member(qualifier, item.name.value.leaf().atom().clone());
        table.references.push(ReferenceInfo {
            span: item.name.span,
            target: ReferenceTarget::Unresolved(UnresolvedSymbol::Unit(path)),
        });
    }
}
