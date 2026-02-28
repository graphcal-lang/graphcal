//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;

use graphcal_syntax::ast::{
    AssertDecl, ConstDecl, DeclKind, DimDecl, DimExpr, DomainBound, ExprKind, FigureDecl, FnBody,
    FnDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamDecl, PatternBinding,
    PlotDecl, TypeDecl, TypeExpr, TypeExprKind, UnitDecl, UnitExpr,
};
use graphcal_syntax::span::Span;

use graphcal_eval::builtins::{builtin_constants, builtin_functions};
use graphcal_eval::eval::format_number;
use graphcal_eval::format::format_unit_expr_with_config;
use graphcal_eval::registry::{IndexKind, Registry};
use graphcal_eval::tir::{ResolvedFnSig, ResolvedIndex, ResolvedTypeExpr, TIR};

/// The kind of expression scope that introduces local variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprScopeKind {
    Block,
    For,
    Scan,
    Unfold,
    Match,
}

/// A typed key for symbol table entries.
///
/// Replaces ad-hoc `String` keys like `"fn_name::param"` or `"field::name"`
/// with a structured enum that can be pattern-matched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SymbolKey {
    /// Top-level declaration: param, node, const, dim, unit, type, fn, index,
    /// assert, plot, figure, builtins.
    TopLevel(String),
    /// Variant of a type or index: e.g., `Season::Winter`.
    Variant { parent: String, variant: String },
    /// Field reference: e.g., `field::thrust`.
    Field(String),
    /// Function-scoped: parameter or block-local in a function body.
    FnScoped { fn_name: String, local: String },
    /// Expression-scoped local variable (block, for, scan, unfold, match).
    ExprScoped {
        kind: ExprScopeKind,
        offset: usize,
        local: String,
    },
}

impl std::fmt::Display for SymbolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TopLevel(name) => write!(f, "{name}"),
            Self::Variant { parent, variant } => write!(f, "{parent}::{variant}"),
            Self::Field(name) => write!(f, "field::{name}"),
            Self::FnScoped { fn_name, local } => write!(f, "{fn_name}::{local}"),
            Self::ExprScoped {
                kind,
                offset,
                local,
            } => {
                let kind_str = match kind {
                    ExprScopeKind::Block => "block",
                    ExprScopeKind::For => "for",
                    ExprScopeKind::Scan => "scan",
                    ExprScopeKind::Unfold => "unfold",
                    ExprScopeKind::Match => "match",
                };
                write!(f, "{kind_str}@{offset}::{local}")
            }
        }
    }
}

impl SymbolKey {
    /// Returns `true` if this is a `Field` key.
    pub const fn is_field(&self) -> bool {
        matches!(self, Self::Field(_))
    }

    /// Returns `true` if this is a `Variant` whose parent matches the given name.
    pub fn is_variant_of(&self, parent_name: &str) -> bool {
        matches!(self, Self::Variant { parent, .. } if parent == parent_name)
    }

    /// Returns the top-level name if this is a `TopLevel` key.
    pub fn top_level_name(&self) -> Option<&str> {
        match self {
            Self::TopLevel(name) => Some(name),
            _ => None,
        }
    }
}

/// The category of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Param,
    Node,
    Const,
    Dimension,
    Unit,
    StructType,
    Function,
    Index,
    IndexVariant,
    Field,
    LocalVar,
    BuiltinFn,
    BuiltinConst,
    Assert,
    Plot,
    Figure,
    Layer,
}

/// Information about a symbol definition.
#[derive(Debug, Clone)]
pub struct DefinitionInfo {
    pub name: String,
    pub category: SymbolCategory,
    /// Span of just the name token.
    pub name_span: Span,
    /// Span of the full declaration.
    pub decl_span: Span,
    /// Human-readable type/signature for hover (populated from TIR).
    pub type_description: Option<String>,
    /// Additional detail for hover.
    pub detail: Option<String>,
}

/// A reference occurrence: a name that refers to a definition.
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// Byte-offset span of this reference in the current file.
    pub span: Span,
    /// Key into `definitions` that this reference points to.
    pub target: SymbolKey,
}

/// The complete symbol table for one file.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// All symbol definitions keyed by a typed `SymbolKey`.
    pub definitions: HashMap<SymbolKey, DefinitionInfo>,
    /// All reference occurrences sorted by span offset.
    pub references: Vec<ReferenceInfo>,
}

impl SymbolTable {
    /// Find the reference at a given byte offset, if any.
    pub fn find_reference_at(&self, offset: usize) -> Option<&ReferenceInfo> {
        // Binary search for a reference whose span contains the offset.
        let idx = self
            .references
            .binary_search_by(|r| {
                if offset < r.span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= r.span.offset() + r.span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.references[idx])
    }

    /// Find the definition whose name span contains the given byte offset, if any.
    pub fn find_definition_at(&self, offset: usize) -> Option<&DefinitionInfo> {
        self.definitions.values().find(|d| {
            offset >= d.name_span.offset() && offset < d.name_span.offset() + d.name_span.len()
        })
    }

    /// Find all references that point to the given target key.
    pub fn find_all_references(&self, target: &SymbolKey) -> Vec<&ReferenceInfo> {
        self.references
            .iter()
            .filter(|r| &r.target == target)
            .collect()
    }
}

/// Scope stack for tracking local variable bindings.
struct ScopeStack {
    /// Each scope maps local name -> definition key in the symbol table.
    scopes: Vec<HashMap<String, SymbolKey>>,
}

impl ScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, key: SymbolKey) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, key);
        }
    }

    fn resolve(&self, name: &str) -> Option<&SymbolKey> {
        for scope in self.scopes.iter().rev() {
            if let Some(key) = scope.get(name) {
                return Some(key);
            }
        }
        None
    }
}

/// Build a symbol table from a parsed AST file.
pub fn build_from_ast(ast: &graphcal_syntax::ast::File) -> SymbolTable {
    let mut table = SymbolTable::default();
    let mut scopes = ScopeStack::new();

    register_builtins(&mut table);

    for decl in &ast.declarations {
        collect_attribute_refs(&decl.attributes, &mut table);

        match &decl.kind {
            DeclKind::Param(p) => collect_param_decl(p, decl.span, &mut table, &mut scopes),
            DeclKind::Node(n) => collect_node_decl(n, decl.span, &mut table, &mut scopes),
            DeclKind::Const(c) => collect_const_decl(c, decl.span, &mut table, &mut scopes),
            DeclKind::Dimension(d) => collect_dim_decl(d, decl.span, &mut table),
            DeclKind::Unit(u) => collect_unit_decl(u, decl.span, &mut table, &mut scopes),
            DeclKind::Type(t) => collect_type_decl(t, decl.span, &mut table),
            DeclKind::Fn(f) => collect_fn_decl(f, decl.span, &mut table, &mut scopes),
            DeclKind::Index(idx) => collect_index_decl(idx, decl.span, &mut table),
            DeclKind::Assert(a) => collect_assert_decl(a, decl.span, &mut table, &mut scopes),
            DeclKind::Plot(p) => collect_plot_decl(p, decl.span, &mut table, &mut scopes),
            DeclKind::Figure(f) => collect_figure_decl(f, decl.span, &mut table, &mut scopes),
            DeclKind::Layer(l) => collect_layer_decl(l, decl.span, &mut table, &mut scopes),
            DeclKind::Import(u) => collect_import_decl(u, &mut table),
        }
    }

    // Sort references by offset for binary search.
    table.references.sort_by_key(|r| r.span.offset());
    table
}

fn register_builtins(table: &mut SymbolTable) {
    for name in builtin_constants().keys() {
        table.definitions.insert(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
            },
        );
    }

    for (name, f) in builtin_functions() {
        table.definitions.insert(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(format!("builtin, arity {}", f.arity())),
            },
        );
    }
}

fn collect_attribute_refs(attributes: &[graphcal_syntax::ast::Attribute], table: &mut SymbolTable) {
    for attr in attributes {
        if attr.name.name == "assumes" {
            for arg in &attr.args {
                if let Some(ident) = arg.as_single_ident() {
                    table.references.push(ReferenceInfo {
                        span: ident.span,
                        target: SymbolKey::TopLevel(ident.name.clone()),
                    });
                }
            }
        }
    }
}

fn collect_param_decl(
    p: &ParamDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = p.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Param,
            name_span: p.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    collect_type_expr_refs(&p.type_ann, table);
    if let Some(ref value) = p.value {
        collect_expr_refs(value, table, scopes);
    }
}

fn collect_node_decl(
    n: &NodeDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = n.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Node,
            name_span: n.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    collect_type_expr_refs(&n.type_ann, table);
    collect_expr_refs(&n.value, table, scopes);
}

fn collect_const_decl(
    c: &ConstDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = c.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Const,
            name_span: c.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    collect_type_expr_refs(&c.type_ann, table);
    collect_expr_refs(&c.value, table, scopes);
}

fn collect_dim_decl(d: &DimDecl, decl_span: Span, table: &mut SymbolTable) {
    let name = d.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Dimension,
            name_span: d.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    if let Some(dim_expr) = &d.definition {
        collect_dim_expr_refs(dim_expr, table);
    }
}

fn collect_unit_decl(
    u: &UnitDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = u.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Unit,
            name_span: u.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    collect_dim_expr_refs(&u.dim_type, table);
    if let Some(unit_def) = &u.definition {
        collect_expr_refs(&unit_def.scale_expr, table, scopes);
        collect_unit_expr_refs(&unit_def.unit_expr, table);
    }
}

fn collect_type_decl(t: &TypeDecl, decl_span: Span, table: &mut SymbolTable) {
    let name = t.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name: name.clone(),
            category: SymbolCategory::StructType,
            name_span: t.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    // Add variants (only if more than one, i.e., tagged union, not struct sugar).
    if t.variants.len() > 1 {
        for variant in &t.variants {
            let vname = variant.name.value.to_string();
            let key = SymbolKey::Variant {
                parent: name.clone(),
                variant: vname.clone(),
            };
            table.definitions.insert(
                key,
                DefinitionInfo {
                    name: vname,
                    category: SymbolCategory::IndexVariant,
                    name_span: variant.name.span,
                    decl_span: variant.span,
                    type_description: None,
                    detail: Some(format!("variant of {name}")),
                },
            );
        }
    }
    // Walk field type annotations.
    for variant in &t.variants {
        for field in &variant.fields {
            collect_type_expr_refs(&field.type_ann, table);
        }
    }
}

fn collect_fn_decl(f: &FnDecl, decl_span: Span, table: &mut SymbolTable, scopes: &mut ScopeStack) {
    let fname = f.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(fname.clone()),
        DefinitionInfo {
            name: fname.clone(),
            category: SymbolCategory::Function,
            name_span: f.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );

    // Push scope for function params.
    scopes.push();
    for param in &f.params {
        let pname = param.name.name.clone();
        let key = SymbolKey::FnScoped {
            fn_name: fname.clone(),
            local: pname.clone(),
        };
        table.definitions.insert(
            key.clone(),
            DefinitionInfo {
                name: pname.clone(),
                category: SymbolCategory::LocalVar,
                name_span: param.name.span,
                decl_span: param.name.span,
                type_description: None,
                detail: Some(format!("parameter of fn {fname}")),
            },
        );
        scopes.insert(pname, key);
        collect_type_expr_refs(&param.type_ann, table);
    }

    collect_type_expr_refs(&f.return_type, table);

    match &f.body {
        FnBody::Short(expr) => {
            collect_expr_refs(expr, table, scopes);
        }
        FnBody::Block { stmts, expr } => {
            scopes.push();
            for stmt in stmts {
                collect_expr_refs(&stmt.value, table, scopes);
                let lname = stmt.name.name.clone();
                let key = SymbolKey::FnScoped {
                    fn_name: fname.clone(),
                    local: lname.clone(),
                };
                table.definitions.insert(
                    key.clone(),
                    DefinitionInfo {
                        name: lname.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: stmt.name.span,
                        decl_span: stmt.span,
                        type_description: None,
                        detail: None,
                    },
                );
                scopes.insert(lname, key);
                if let Some(type_ann) = &stmt.type_ann {
                    collect_type_expr_refs(type_ann, table);
                }
            }
            collect_expr_refs(expr, table, scopes);
            scopes.pop();
        }
    }
    scopes.pop();
}

fn collect_index_decl(idx: &IndexDecl, decl_span: Span, table: &mut SymbolTable) {
    let name = idx.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name: name.clone(),
            category: SymbolCategory::Index,
            name_span: idx.name.span,
            decl_span,
            type_description: None,
            detail: None,
        },
    );
    if let IndexDeclKind::Named { variants } = &idx.kind {
        for variant in variants {
            let vname = variant.value.to_string();
            let key = SymbolKey::Variant {
                parent: name.clone(),
                variant: vname.clone(),
            };
            table.definitions.insert(
                key,
                DefinitionInfo {
                    name: vname,
                    category: SymbolCategory::IndexVariant,
                    name_span: variant.span,
                    decl_span: variant.span,
                    type_description: None,
                    detail: Some(format!("label/value variant of index {name}")),
                },
            );
        }
    }
}

fn collect_assert_decl(
    a: &AssertDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = a.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Assert,
            name_span: a.name.span,
            decl_span,
            type_description: Some("Bool".to_string()),
            detail: Some("assert".to_string()),
        },
    );
    match &a.body {
        graphcal_syntax::ast::AssertBody::Expr(expr) => {
            collect_expr_refs(expr, table, scopes);
        }
        graphcal_syntax::ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            ..
        } => {
            collect_expr_refs(actual, table, scopes);
            collect_expr_refs(expected, table, scopes);
            collect_expr_refs(tolerance, table, scopes);
        }
    }
}

fn collect_plot_decl(
    p: &PlotDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = p.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Plot,
            name_span: p.name.span,
            decl_span,
            type_description: Some(format!("plot (mark: {})", p.mark.mark_type)),
            detail: Some("plot".to_string()),
        },
    );
    for encoding in &p.encodings {
        collect_expr_refs(&encoding.value, table, scopes);
    }
    for prop in &p.mark.properties {
        collect_expr_refs(&prop.value, table, scopes);
    }
    for prop in &p.properties {
        collect_expr_refs(&prop.value, table, scopes);
    }
}

fn collect_figure_decl(
    f: &FigureDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = f.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Figure,
            name_span: f.name.span,
            decl_span,
            type_description: Some("figure".to_string()),
            detail: Some("figure".to_string()),
        },
    );
    for field in &f.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_layer_decl(
    l: &LayerDecl,
    decl_span: Span,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    let name = l.name.value.to_string();
    table.definitions.insert(
        SymbolKey::TopLevel(name.clone()),
        DefinitionInfo {
            name,
            category: SymbolCategory::Layer,
            name_span: l.name.span,
            decl_span,
            type_description: Some("layer".to_string()),
            detail: Some("layer".to_string()),
        },
    );
    for field in &l.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_import_decl(u: &ImportDecl, table: &mut SymbolTable) {
    // Each imported name is a reference; target resolution for cross-file
    // go-to-definition is handled separately.
    if let graphcal_syntax::ast::ImportKind::Selective(names) = &u.kind {
        for import_item in names {
            table.references.push(ReferenceInfo {
                span: import_item.name.span,
                target: SymbolKey::TopLevel(import_item.name.name.clone()),
            });
            // If aliased, the alias also resolves to the same target.
            if let Some(alias) = &import_item.alias {
                table.references.push(ReferenceInfo {
                    span: alias.span,
                    target: SymbolKey::TopLevel(import_item.name.name.clone()),
                });
            }
        }
    }
}

/// Collect references from an expression, tracking local scopes.
#[expect(
    clippy::too_many_lines,
    reason = "expression walker needs to handle every ExprKind variant"
)]
fn collect_expr_refs(
    expr: &graphcal_syntax::ast::Expr,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    match &expr.kind {
        ExprKind::GraphRef(name)
        | ExprKind::QualifiedGraphRef { name, .. }
        | ExprKind::ConstRef(name)
        | ExprKind::QualifiedConstRef { name, .. } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::TopLevel(name.value.to_string()),
            });
        }
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::TopLevel(name.value.to_string()),
            });
            for arg in args {
                collect_expr_refs(arg, table, scopes);
            }
        }
        ExprKind::LocalRef(ident) => {
            let target = scopes
                .resolve(&ident.name)
                .cloned()
                .unwrap_or_else(|| SymbolKey::TopLevel(ident.name.clone()));
            table.references.push(ReferenceInfo {
                span: ident.span,
                target,
            });
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_expr_refs(lhs, table, scopes);
            collect_expr_refs(rhs, table, scopes);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_expr_refs(operand, table, scopes);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_refs(condition, table, scopes);
            collect_expr_refs(then_branch, table, scopes);
            collect_expr_refs(else_branch, table, scopes);
        }
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::Convert { expr, target } => {
            collect_expr_refs(expr, table, scopes);
            collect_unit_expr_refs(target, table);
        }
        ExprKind::DisplayTimezone { expr, .. } => {
            collect_expr_refs(expr, table, scopes);
        }
        ExprKind::AsCast { expr, target_type } => {
            collect_expr_refs(expr, table, scopes);
            collect_type_expr_refs(target_type, table);
        }
        ExprKind::Block { stmts, expr } => {
            scopes.push();
            let scope_offset = expr.span.offset();
            for stmt in stmts {
                collect_expr_refs(&stmt.value, table, scopes);
                let lname = stmt.name.name.clone();
                let key = SymbolKey::ExprScoped {
                    kind: ExprScopeKind::Block,
                    offset: scope_offset,
                    local: lname.clone(),
                };
                table.definitions.insert(
                    key.clone(),
                    DefinitionInfo {
                        name: lname.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: stmt.name.span,
                        decl_span: stmt.span,
                        type_description: None,
                        detail: None,
                    },
                );
                scopes.insert(lname, key);
                if let Some(type_ann) = &stmt.type_ann {
                    collect_type_expr_refs(type_ann, table);
                }
            }
            collect_expr_refs(expr, table, scopes);
            scopes.pop();
        }
        ExprKind::FieldAccess { expr, field } => {
            collect_expr_refs(expr, table, scopes);
            // Field reference -- target is approximate without type info.
            table.references.push(ReferenceInfo {
                span: field.span,
                target: SymbolKey::Field(field.value.to_string()),
            });
        }
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } => {
            table.references.push(ReferenceInfo {
                span: type_name.span,
                target: SymbolKey::TopLevel(type_name.value.to_string()),
            });
            for type_arg in type_args {
                collect_type_expr_refs(type_arg, table);
            }
            for field in fields {
                if let Some(value) = &field.value {
                    collect_expr_refs(value, table, scopes);
                } else {
                    // Shorthand: `{ dv1 }` -- the field name is also a local reference.
                    let target = scopes
                        .resolve(field.name.value.as_str())
                        .cloned()
                        .unwrap_or_else(|| SymbolKey::TopLevel(field.name.value.to_string()));
                    table.references.push(ReferenceInfo {
                        span: field.name.span,
                        target,
                    });
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            // For TableLiteral, also add references for the index names in table[...].
            if let ExprKind::TableLiteral { indexes, .. } = &expr.kind {
                for idx in indexes {
                    table.references.push(ReferenceInfo {
                        span: idx.span,
                        target: SymbolKey::TopLevel(idx.value.to_string()),
                    });
                }
            }
            for entry in entries {
                for key in &entry.keys {
                    table.references.push(ReferenceInfo {
                        span: key.index.span,
                        target: SymbolKey::TopLevel(key.index.value.to_string()),
                    });
                    table.references.push(ReferenceInfo {
                        span: key.variant.span,
                        target: SymbolKey::Variant {
                            parent: key.index.value.to_string(),
                            variant: key.variant.value.to_string(),
                        },
                    });
                }
                collect_expr_refs(&entry.value, table, scopes);
            }
        }
        ExprKind::ForComp { bindings, body } => {
            scopes.push();
            for binding in bindings {
                table.references.push(ReferenceInfo {
                    span: binding.index.span,
                    target: SymbolKey::TopLevel(binding.index.value.to_string()),
                });
                let var_name = binding.var.name.clone();
                let key = SymbolKey::ExprScoped {
                    kind: ExprScopeKind::For,
                    offset: binding.var.span.offset(),
                    local: var_name.clone(),
                };
                table.definitions.insert(
                    key.clone(),
                    DefinitionInfo {
                        name: var_name.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: binding.var.span,
                        decl_span: binding.var.span,
                        type_description: None,
                        detail: Some(format!("loop variable over {}", binding.index.value)),
                    },
                );
                scopes.insert(var_name, key);
            }
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::IndexAccess { expr, args } => {
            collect_expr_refs(expr, table, scopes);
            for arg in args {
                match arg {
                    graphcal_syntax::ast::IndexArg::Variant { index, variant } => {
                        table.references.push(ReferenceInfo {
                            span: index.span,
                            target: SymbolKey::TopLevel(index.value.to_string()),
                        });
                        table.references.push(ReferenceInfo {
                            span: variant.span,
                            target: SymbolKey::Variant {
                                parent: index.value.to_string(),
                                variant: variant.value.to_string(),
                            },
                        });
                    }
                    graphcal_syntax::ast::IndexArg::Var(ident) => {
                        let target = scopes
                            .resolve(&ident.name)
                            .cloned()
                            .unwrap_or_else(|| SymbolKey::TopLevel(ident.name.clone()));
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target,
                        });
                    }
                }
            }
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            collect_expr_refs(source, table, scopes);
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let scan_offset = expr.span.offset();
            let acc_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Scan,
                offset: scan_offset,
                local: "acc".to_string(),
            };
            let val_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Scan,
                offset: scan_offset,
                local: "val".to_string(),
            };
            table.definitions.insert(
                acc_key.clone(),
                DefinitionInfo {
                    name: acc_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: acc_name.span,
                    decl_span: acc_name.span,
                    type_description: None,
                    detail: Some("scan accumulator".to_string()),
                },
            );
            table.definitions.insert(
                val_key.clone(),
                DefinitionInfo {
                    name: val_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: val_name.span,
                    decl_span: val_name.span,
                    type_description: None,
                    detail: Some("scan value".to_string()),
                },
            );
            scopes.insert(acc_name.name.clone(), acc_key);
            scopes.insert(val_name.name.clone(), val_key);
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let unfold_offset = expr.span.offset();
            let prev_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Unfold,
                offset: unfold_offset,
                local: "prev".to_string(),
            };
            let curr_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Unfold,
                offset: unfold_offset,
                local: "curr".to_string(),
            };
            table.definitions.insert(
                prev_key.clone(),
                DefinitionInfo {
                    name: prev_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: prev_name.span,
                    decl_span: prev_name.span,
                    type_description: None,
                    detail: Some("unfold previous step".to_string()),
                },
            );
            table.definitions.insert(
                curr_key.clone(),
                DefinitionInfo {
                    name: curr_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: curr_name.span,
                    decl_span: curr_name.span,
                    type_description: None,
                    detail: Some("unfold current step".to_string()),
                },
            );
            scopes.insert(prev_name.name.clone(), prev_key);
            scopes.insert(curr_name.name.clone(), curr_key);
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::VariantLiteral { index, variant } => {
            // Reference to the index name
            table.references.push(ReferenceInfo {
                span: index.span,
                target: SymbolKey::TopLevel(index.value.to_string()),
            });
            // Reference to the qualified variant: Index::Variant
            table.references.push(ReferenceInfo {
                span: variant.span,
                target: SymbolKey::Variant {
                    parent: index.value.to_string(),
                    variant: variant.value.to_string(),
                },
            });
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_expr_refs(scrutinee, table, scopes);
            for arm in arms {
                let variant_name = arm.pattern.variant_name.value.to_string();

                // If the pattern has a qualified index (e.g., Maneuver::Departure),
                // add a reference for the index name too.
                if let Some(qi) = &arm.pattern.qualified_index {
                    table.references.push(ReferenceInfo {
                        span: qi.span,
                        target: SymbolKey::TopLevel(qi.value.to_string()),
                    });
                    // Reference the qualified variant: Index::Variant
                    table.references.push(ReferenceInfo {
                        span: arm.pattern.variant_name.span,
                        target: SymbolKey::Variant {
                            parent: qi.value.to_string(),
                            variant: variant_name.clone(),
                        },
                    });
                } else {
                    // Try to resolve variant as Type::Variant (tagged union).
                    table.references.push(ReferenceInfo {
                        span: arm.pattern.variant_name.span,
                        target: SymbolKey::TopLevel(variant_name.clone()),
                    });
                }

                scopes.push();
                for binding in &arm.pattern.bindings {
                    match binding {
                        PatternBinding::Bind { field, var } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: SymbolKey::Field(field.value.to_string()),
                            });
                            let var_key = SymbolKey::ExprScoped {
                                kind: ExprScopeKind::Match,
                                offset: arm.span.offset(),
                                local: var.name.clone(),
                            };
                            table.definitions.insert(
                                var_key.clone(),
                                DefinitionInfo {
                                    name: var.name.clone(),
                                    category: SymbolCategory::LocalVar,
                                    name_span: var.span,
                                    decl_span: var.span,
                                    type_description: None,
                                    detail: Some(format!("bound from {variant_name}")),
                                },
                            );
                            scopes.insert(var.name.clone(), var_key);
                        }
                        PatternBinding::Wildcard { field, .. } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: SymbolKey::Field(field.value.to_string()),
                            });
                        }
                    }
                }
                collect_expr_refs(&arm.body, table, scopes);
                scopes.pop();
            }
        }
        ExprKind::TupleMatch { scrutinees, arms } => {
            for s in scrutinees {
                collect_expr_refs(s, table, scopes);
            }
            for arm in arms {
                if let Some(patterns) = &arm.patterns {
                    for p in patterns {
                        collect_expr_refs(p, table, scopes);
                    }
                }
                collect_expr_refs(&arm.body, table, scopes);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_) => {}
    }
}

/// Collect references from a type expression.
fn collect_type_expr_refs(type_expr: &graphcal_syntax::ast::TypeExpr, table: &mut SymbolTable) {
    match &type_expr.kind {
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
        TypeExprKind::DimExpr(dim_expr) => {
            collect_dim_expr_refs(dim_expr, table);
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_refs(base, table);
            for idx in indexes {
                table.references.push(ReferenceInfo {
                    span: idx.span,
                    target: SymbolKey::TopLevel(idx.name.clone()),
                });
            }
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::TopLevel(name.name.clone()),
            });
            for arg in type_args {
                collect_type_expr_refs(arg, table);
            }
        }
    }
    // Collect references from domain constraint bound expressions (e.g., unit names in `100 kg`).
    for bound in &type_expr.constraints {
        collect_constraint_expr_refs(&bound.value, table);
    }
}

/// Collect references from a constraint bound expression (limited walk for unit names).
fn collect_constraint_expr_refs(expr: &graphcal_syntax::ast::Expr, table: &mut SymbolTable) {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_constraint_expr_refs(operand, table);
        }
        _ => {}
    }
}

/// Collect references from a dimension expression.
fn collect_dim_expr_refs(dim_expr: &DimExpr, table: &mut SymbolTable) {
    for item in &dim_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.term.span,
            target: SymbolKey::TopLevel(item.term.name.name.clone()),
        });
    }
}

/// Collect references from a unit expression.
fn collect_unit_expr_refs(unit_expr: &UnitExpr, table: &mut SymbolTable) {
    for item in &unit_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.name.span,
            target: SymbolKey::TopLevel(item.name.value.to_string()),
        });
    }
}

/// Format a domain bound expression as a human-readable string.
///
/// Handles the common cases: number literals, unit-annotated literals, and negated forms.
fn format_bound_expr(expr: &graphcal_syntax::ast::Expr) -> String {
    match &expr.kind {
        ExprKind::Number(v) => format_number(*v),
        ExprKind::Integer(v) => v.to_string(),
        ExprKind::UnitLiteral { value, unit } => {
            let num = format_number(*value);
            let unit_str = format_unit_expr_with_config(unit, true);
            format!("{num} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_syntax::ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_bound_expr(operand))
        }
        // For complex expressions, fall back to "..."
        _ => "...".to_string(),
    }
}

/// Format a constraint clause from domain bounds.
///
/// Returns a string like `(min: 100 kg, max: 2000 kg)` or an empty string if no constraints.
fn format_constraints(constraints: &[DomainBound]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints
        .iter()
        .map(|b| format!("{}: {}", b.kind, format_bound_expr(&b.value)))
        .collect();
    format!("({})", parts.join(", "))
}

/// Format a resolved type expression with domain constraints.
///
/// For indexed types like `Velocity[Maneuver]`, inserts the constraint clause
/// between the base type and the index suffix: `Velocity(min: 0 m/s)[Maneuver]`.
fn format_type_with_constraints(
    resolved: &ResolvedTypeExpr,
    constraints: &[DomainBound],
    registry: &Registry,
) -> String {
    let constraint_str = format_constraints(constraints);
    if let ResolvedTypeExpr::Indexed { base, indexes } = resolved {
        let base_str = base.format(registry);
        let idx_strs: Vec<String> = indexes
            .iter()
            .map(|i| match i {
                ResolvedIndex::Concrete(name, _) => name.to_string(),
                ResolvedIndex::GenericParam(name, _) => name.to_string(),
            })
            .collect();
        format!("{base_str}{constraint_str}[{}]", idx_strs.join(", "))
    } else {
        let type_str = resolved.format(registry);
        format!("{type_str}{constraint_str}")
    }
}

/// Extract domain constraints from a `TypeExpr`, looking through `Indexed` wrappers.
fn extract_constraints(type_expr: &TypeExpr) -> &[DomainBound] {
    if !type_expr.constraints.is_empty() {
        return &type_expr.constraints;
    }
    // For indexed types, the constraints are on the base type
    if let TypeExprKind::Indexed { base, .. } = &type_expr.kind {
        return &base.constraints;
    }
    &[]
}

/// Format a function signature as `fn name<generics>(params) -> ret`.
pub fn format_fn_signature(fn_name: &str, sig: &ResolvedFnSig, registry: &Registry) -> String {
    let params_str: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.resolved_type.format(registry)))
        .collect();

    let generics = if sig.generic_dim_params.is_empty() && sig.generic_index_params.is_empty() {
        String::new()
    } else {
        let all: Vec<String> = sig
            .generic_dim_params
            .iter()
            .map(|p| format!("{p}: Dim"))
            .chain(
                sig.generic_index_params
                    .iter()
                    .map(|p| format!("{p}: Index")),
            )
            .collect();
        format!("<{}>", all.join(", "))
    };

    let ret = sig.return_type.format(registry);
    format!("fn {fn_name}{generics}({}) -> {ret}", params_str.join(", "))
}

/// Enrich a symbol table with type information from a TIR.
#[expect(
    clippy::too_many_lines,
    reason = "linear match over all symbol categories"
)]
pub fn enrich_from_tir(table: &mut SymbolTable, tir: &TIR) {
    let registry = &tir.registry;

    // Build a map from declaration name to its AST TypeExpr constraints.
    let mut decl_constraints: HashMap<String, &[DomainBound]> = HashMap::new();
    for e in &tir.params {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }
    for e in &tir.nodes {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }
    for e in &tir.consts {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }

    // Enrich param/node/const declarations with resolved types + constraints.
    for (name, resolved_type) in &tir.resolved_decl_types {
        let name_str = name.to_string();
        let key = SymbolKey::TopLevel(name_str.clone());
        if let Some(def) = table.definitions.get_mut(&key) {
            let type_desc = decl_constraints.get(&name_str).map_or_else(
                || resolved_type.format(registry),
                |constraints| format_type_with_constraints(resolved_type, constraints, registry),
            );
            def.type_description = Some(type_desc);
        }
    }

    // Enrich function definitions with signatures.
    for (fn_name, sig) in &tir.resolved_fn_sigs {
        let key = SymbolKey::TopLevel(fn_name.as_str().to_string());
        if let Some(def) = table.definitions.get_mut(&key) {
            def.type_description = Some(format_fn_signature(fn_name.as_str(), sig, registry));
        }
    }

    // Enrich dimension/unit/index/struct definitions from registry.
    // Collect keys and categories first to avoid cloning the entire HashMap.
    let definition_keys: Vec<(SymbolKey, SymbolCategory)> = table
        .definitions
        .iter()
        .map(|(k, d)| (k.clone(), d.category))
        .collect();
    for (key, category) in &definition_keys {
        let Some(name) = key.top_level_name() else {
            continue;
        };
        match category {
            SymbolCategory::Dimension => {
                if let Some(dim) = registry.dimensions.get_dimension(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    def_mut.type_description = Some(format!(
                        "dimension {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            SymbolCategory::Unit => {
                if let Some(unit_info) = registry.units.get_unit(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    def_mut.type_description = Some(format!(
                        "{}, scale = {}",
                        registry.dimensions.format_dimension(&unit_info.dimension),
                        unit_info.scale
                    ));
                }
            }
            SymbolCategory::Index => {
                if let Some(idx_def) = registry.indexes.get_index(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    match &idx_def.kind {
                        IndexKind::Named { variants } => {
                            let vs: Vec<&str> = variants
                                .iter()
                                .map(graphcal_syntax::names::VariantName::as_str)
                                .collect();
                            def_mut.type_description = Some(format!("{{ {} }}", vs.join(", ")));
                        }
                        IndexKind::Range {
                            start, end, step, ..
                        } => {
                            def_mut.type_description =
                                Some(format!("range({start}, {end}, step: {step})"));
                        }
                    }
                }
            }
            SymbolCategory::StructType => {
                if let Some(type_def) = registry.types.get_type(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let variants_desc: Vec<String> = type_def
                        .variants
                        .iter()
                        .map(|v| {
                            if v.fields.is_empty() {
                                v.name.to_string()
                            } else {
                                let fields: Vec<String> =
                                    v.fields.iter().map(|f| f.name.to_string()).collect();
                                format!("{} {{ {} }}", v.name, fields.join(", "))
                            }
                        })
                        .collect();
                    def_mut.type_description = Some(variants_desc.join(" | "));
                }
            }
            _ => {}
        }
    }

    // Register field definitions from struct types so that `field::name`
    // references resolve to a definition with hover info.
    for type_def in registry.types.all_types() {
        for variant in &type_def.variants {
            for field in &variant.fields {
                let field_key = SymbolKey::Field(field.name.to_string());
                table
                    .definitions
                    .entry(field_key)
                    .or_insert_with(|| DefinitionInfo {
                        name: field.name.to_string(),
                        category: SymbolCategory::Field,
                        name_span: Span::new(0, 0),
                        decl_span: Span::new(0, 0),
                        type_description: None,
                        detail: Some(format!("field of {}", type_def.name)),
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn build_symbol_table_basic() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        let x_key = SymbolKey::TopLevel("x".to_string());
        let y_key = SymbolKey::TopLevel("y".to_string());
        assert!(table.definitions.contains_key(&x_key));
        assert!(table.definitions.contains_key(&y_key));
        assert_eq!(table.definitions[&x_key].category, SymbolCategory::Param);
        assert_eq!(table.definitions[&y_key].category, SymbolCategory::Node);

        // @x is a reference
        assert!(
            table.references.iter().any(|r| r.target == x_key),
            "expected @x reference"
        );
    }

    #[test]
    fn build_symbol_table_with_function() {
        let source = "fn double<D: Dim>(x: D) -> D = x + x;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        let double_key = SymbolKey::TopLevel("double".to_string());
        let double_x_key = SymbolKey::FnScoped {
            fn_name: "double".to_string(),
            local: "x".to_string(),
        };
        assert!(table.definitions.contains_key(&double_key));
        assert_eq!(
            table.definitions[&double_key].category,
            SymbolCategory::Function
        );
        assert!(table.definitions.contains_key(&double_x_key));
        assert_eq!(
            table.definitions[&double_x_key].category,
            SymbolCategory::LocalVar
        );
    }

    #[test]
    fn find_reference_at_offset() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        // Find the @x reference -- it should be near the end of the source
        let at_x_offset = source.find("@x").unwrap() + 1; // offset of 'x' in '@x'
        let reference = table.find_reference_at(at_x_offset);
        assert!(reference.is_some(), "expected to find reference at @x");
        assert_eq!(
            reference.unwrap().target,
            SymbolKey::TopLevel("x".to_string())
        );
    }

    #[test]
    fn symbol_key_display() {
        assert_eq!(SymbolKey::TopLevel("x".to_string()).to_string(), "x");
        assert_eq!(
            SymbolKey::Variant {
                parent: "Phase".to_string(),
                variant: "Launch".to_string()
            }
            .to_string(),
            "Phase::Launch"
        );
        assert_eq!(
            SymbolKey::Field("thrust".to_string()).to_string(),
            "field::thrust"
        );
        assert_eq!(
            SymbolKey::FnScoped {
                fn_name: "sqrt".to_string(),
                local: "x".to_string()
            }
            .to_string(),
            "sqrt::x"
        );
        assert_eq!(
            SymbolKey::ExprScoped {
                kind: ExprScopeKind::Block,
                offset: 42,
                local: "temp".to_string()
            }
            .to_string(),
            "block@42::temp"
        );
    }

    #[test]
    fn symbol_key_helpers() {
        assert!(SymbolKey::Field("x".to_string()).is_field());
        assert!(!SymbolKey::TopLevel("x".to_string()).is_field());

        let variant = SymbolKey::Variant {
            parent: "Phase".to_string(),
            variant: "Launch".to_string(),
        };
        assert!(variant.is_variant_of("Phase"));
        assert!(!variant.is_variant_of("Other"));

        assert_eq!(
            SymbolKey::TopLevel("x".to_string()).top_level_name(),
            Some("x")
        );
        assert_eq!(SymbolKey::Field("x".to_string()).top_level_name(), None);
    }
}
