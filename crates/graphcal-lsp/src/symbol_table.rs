//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;

use graphcal_compiler::syntax::ast::{
    AssertDecl, BaseDimDecl, DagDecl, DeclKind, DimDecl, DimExpr, DomainBound, ExprKind,
    FigureDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamDecl,
    PatternBinding, PlotDecl, TableIndexSpec, TypeDecl, TypeExpr, TypeExprKind, UnionTypeDecl,
    UnitDecl, UnitExpr, Visibility,
};
use graphcal_compiler::syntax::span::Span;

use graphcal_eval::builtins::{builtin_constants, builtin_functions};
use graphcal_eval::eval::format_number;
use graphcal_eval::format::format_unit_expr_with_config;
use graphcal_eval::registry::{IndexKind, Registry, UnitScale};
use graphcal_eval::tir::{ResolvedIndex, ResolvedTypeExpr, TIR};

/// The kind of expression scope that introduces local variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprScopeKind {
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
    /// Top-level declaration: param, node, const, dim, unit, type, index,
    /// assert, plot, figure, builtins.
    TopLevel(String),
    /// Module-qualified reference: e.g., `@params::dry_mass` or `math::PI`.
    /// Preserves the module namespace so that two modules exporting the same
    /// member name are distinguished.
    Qualified { module: String, name: String },
    /// Variant of a type or index: e.g., `Season::Winter`.
    Variant { parent: String, variant: String },
    /// Field reference: e.g., `field::thrust`.
    Field(String),
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
            Self::Qualified { module, name } => write!(f, "{module}::{name}"),
            Self::Variant { parent, variant } => write!(f, "{parent}::{variant}"),
            Self::Field(name) => write!(f, "field::{name}"),
            Self::ExprScoped {
                kind,
                offset,
                local,
            } => {
                let kind_str = match kind {
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
    Dag,
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
    /// Visibility / bindability of the declaration.
    ///
    /// `None` for builtins, fields, and local variables (concepts that have
    /// no surface annotation).
    pub visibility: Option<Visibility>,
}

impl DefinitionInfo {
    /// Returns `true` if this is a built-in function or constant.
    pub const fn is_builtin(&self) -> bool {
        matches!(
            self.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
        )
    }

    /// Returns `true` if this definition has a navigable source location.
    ///
    /// Builtins and definitions with empty name spans have no source to navigate to.
    pub const fn is_navigable(&self) -> bool {
        !self.is_builtin() && !self.name_span.is_empty()
    }
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
    /// Secondary index: name-span byte offset → `SymbolKey`.
    ///
    /// Populated alongside `definitions` so that `find_definition_key` is O(1)
    /// instead of a linear reverse scan.
    name_span_to_key: HashMap<usize, SymbolKey>,
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

    /// Look up the key for a definition by its name-span byte offset.
    ///
    /// O(1) via the `name_span_to_key` secondary index — no linear scan needed.
    pub fn find_definition_key(&self, definition: &DefinitionInfo) -> SymbolKey {
        if let Some(key) = self.name_span_to_key.get(&definition.name_span.offset()) {
            return key.clone();
        }
        // Fallback: should not happen if the definition came from this table.
        debug_assert!(
            false,
            "find_definition_key: no matching definition for {:?} at offset {}",
            definition.name,
            definition.name_span.offset()
        );
        SymbolKey::TopLevel(definition.name.clone())
    }

    /// Insert a definition and update the secondary `name_span_to_key` index.
    fn insert_definition(&mut self, key: SymbolKey, definition: DefinitionInfo) {
        // Only index non-empty spans (builtins and fields have empty spans).
        if !definition.name_span.is_empty() {
            self.name_span_to_key
                .insert(definition.name_span.offset(), key.clone());
        }
        self.definitions.insert(key, definition);
    }

    /// Register a top-level definition from a named declaration.
    ///
    /// This is a convenience helper that handles the common pattern of:
    /// 1. Converting the name to a `String`
    /// 2. Creating a `SymbolKey::TopLevel`
    /// 3. Inserting a `DefinitionInfo` with the given category and spans
    #[expect(
        clippy::too_many_arguments,
        reason = "declaration fields line up with DefinitionInfo — splitting would just shuffle them across a struct"
    )]
    fn register_top_level(
        &mut self,
        name: impl AsRef<str>,
        name_span: Span,
        decl_span: Span,
        category: SymbolCategory,
        type_description: Option<String>,
        detail: Option<String>,
        visibility: Visibility,
    ) {
        let name = name.as_ref().to_string();
        self.insert_definition(
            SymbolKey::TopLevel(name.clone()),
            DefinitionInfo {
                name,
                category,
                name_span,
                decl_span,
                type_description,
                detail,
                visibility: Some(visibility),
            },
        );
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
        debug_assert!(
            self.scopes.len() > 1,
            "ScopeStack::pop called with only the root scope remaining"
        );
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
pub fn build_from_ast(ast: &graphcal_compiler::syntax::ast::File) -> SymbolTable {
    let mut table = SymbolTable::default();
    let mut scopes = ScopeStack::new();

    register_builtins(&mut table);

    for decl in &ast.declarations {
        collect_attribute_refs(&decl.attributes, &mut table);

        let vis = decl.visibility;
        match &decl.kind {
            DeclKind::Param(p) => collect_param_decl(p, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Node(n) => collect_node_decl(n, decl.span, vis, &mut table, &mut scopes),
            DeclKind::ConstNode(c) => {
                collect_const_node_decl(c, decl.span, vis, &mut table, &mut scopes);
            }
            DeclKind::BaseDimension(d) => collect_base_dim_decl(d, decl.span, vis, &mut table),
            DeclKind::Dimension(d) => collect_dim_decl(d, decl.span, vis, &mut table),
            DeclKind::Unit(u) => collect_unit_decl(u, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Type(t) => collect_type_decl(t, decl.span, vis, &mut table),
            DeclKind::UnionType(u) => collect_union_type_decl(u, decl.span, vis, &mut table),
            DeclKind::Index(idx) => collect_index_decl(idx, decl.span, vis, &mut table),
            DeclKind::Assert(a) => collect_assert_decl(a, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Plot(p) => collect_plot_decl(p, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Figure(f) => collect_figure_decl(f, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Layer(l) => collect_layer_decl(l, decl.span, vis, &mut table, &mut scopes),
            DeclKind::Import(u) => collect_import_decl(u, &mut table),
            DeclKind::Include(u) => collect_include_decl(u, &mut table),
            DeclKind::Dag(d) => collect_dag_decl(d, decl.span, vis, &mut table),
        }
    }

    // Sort references by offset for binary search.
    table.references.sort_by_key(|r| r.span.offset());
    table
}

fn register_builtins(table: &mut SymbolTable) {
    for name in builtin_constants().keys() {
        table.insert_definition(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
                visibility: None,
            },
        );
    }

    for (name, f) in builtin_functions() {
        table.insert_definition(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(format!("builtin, arity {}", f.arity())),
                visibility: None,
            },
        );
    }
}

fn collect_attribute_refs(
    attributes: &[graphcal_compiler::syntax::ast::Attribute],
    table: &mut SymbolTable,
) {
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
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &p.name.value,
        p.name.span,
        decl_span,
        SymbolCategory::Param,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&p.type_ann, table);
    if let Some(ref value) = p.value {
        collect_expr_refs(value, table, scopes);
    }
}

fn collect_node_decl(
    n: &NodeDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &n.name.value,
        n.name.span,
        decl_span,
        SymbolCategory::Node,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&n.type_ann, table);
    collect_expr_refs(&n.value, table, scopes);
}

fn collect_const_node_decl(
    c: &graphcal_compiler::syntax::ast::ConstNodeDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &c.name.value,
        c.name.span,
        decl_span,
        SymbolCategory::Const,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&c.type_ann, table);
    collect_expr_refs(&c.value, table, scopes);
}

fn collect_base_dim_decl(
    d: &BaseDimDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &d.name.value,
        d.name.span,
        decl_span,
        SymbolCategory::Dimension,
        None,
        None,
        visibility,
    );
}

fn collect_dim_decl(d: &DimDecl, decl_span: Span, visibility: Visibility, table: &mut SymbolTable) {
    table.register_top_level(
        &d.name.value,
        d.name.span,
        decl_span,
        SymbolCategory::Dimension,
        None,
        None,
        visibility,
    );
    if let Some(definition) = &d.definition {
        collect_dim_expr_refs(definition, table);
    }
}

fn collect_unit_decl(
    u: &UnitDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &u.name.value,
        u.name.span,
        decl_span,
        SymbolCategory::Unit,
        None,
        None,
        visibility,
    );
    collect_dim_expr_refs(&u.dim_type, table);
    if let Some(unit_def) = &u.definition {
        collect_expr_refs(&unit_def.scale_expr, table, scopes);
        collect_unit_expr_refs(&unit_def.unit_expr, table);
    }
}

fn collect_type_decl(
    t: &TypeDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &t.name.value,
        t.name.span,
        decl_span,
        SymbolCategory::StructType,
        None,
        None,
        visibility,
    );
    // Walk field type annotations (required types have no fields).
    if let Some(fields) = &t.fields {
        for field in fields {
            collect_type_expr_refs(&field.type_ann, table);
        }
    }
}

fn collect_union_type_decl(
    u: &UnionTypeDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &u.name.value,
        u.name.span,
        decl_span,
        SymbolCategory::StructType,
        None,
        None,
        visibility,
    );
    // Register each union member as a reference to the member type.
    for member in &u.members {
        table.references.push(ReferenceInfo {
            span: member.name.span,
            target: SymbolKey::TopLevel(member.name.value.to_string()),
        });
        // Walk type arguments in the member.
        for type_arg in &member.type_args {
            collect_type_expr_refs(type_arg, table);
        }
    }
}

fn collect_index_decl(
    idx: &IndexDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
) {
    let name = idx.name.value.to_string();
    table.register_top_level(
        &name,
        idx.name.span,
        decl_span,
        SymbolCategory::Index,
        None,
        None,
        visibility,
    );
    match &idx.kind {
        IndexDeclKind::Named { variants } => {
            for variant in variants {
                let vname = variant.value.to_string();
                let key = SymbolKey::Variant {
                    parent: name.clone(),
                    variant: vname.clone(),
                };
                table.insert_definition(
                    key,
                    DefinitionInfo {
                        name: vname,
                        category: SymbolCategory::IndexVariant,
                        name_span: variant.span,
                        decl_span: variant.span,
                        type_description: None,
                        detail: Some(format!("label/value variant of index {name}")),
                        visibility: None,
                    },
                );
            }
        }
        IndexDeclKind::RequiredRange { dimension } => {
            collect_dim_expr_refs(dimension, table);
        }
        IndexDeclKind::Range { .. } | IndexDeclKind::RequiredNamed => {}
    }
}

fn collect_assert_decl(
    a: &AssertDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &a.name.value,
        a.name.span,
        decl_span,
        SymbolCategory::Assert,
        Some("Bool".to_string()),
        Some("assert".to_string()),
        visibility,
    );
    match &a.body {
        graphcal_compiler::syntax::ast::AssertBody::Expr(expr) => {
            collect_expr_refs(expr, table, scopes);
        }
        graphcal_compiler::syntax::ast::AssertBody::Tolerance {
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
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &p.name.value,
        p.name.span,
        decl_span,
        SymbolCategory::Plot,
        Some(format!("plot (mark: {})", p.mark.mark_type)),
        Some("plot".to_string()),
        visibility,
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
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &f.name.value,
        f.name.span,
        decl_span,
        SymbolCategory::Figure,
        Some("figure".to_string()),
        Some("figure".to_string()),
        visibility,
    );
    for field in &f.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_layer_decl(
    l: &LayerDecl,
    decl_span: Span,
    visibility: Visibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &l.name.value,
        l.name.span,
        decl_span,
        SymbolCategory::Layer,
        Some("layer".to_string()),
        Some("layer".to_string()),
        visibility,
    );
    for field in &l.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_dag_decl(d: &DagDecl, decl_span: Span, visibility: Visibility, table: &mut SymbolTable) {
    table.register_top_level(
        d.name.value.as_str(),
        d.name.span,
        decl_span,
        SymbolCategory::Dag,
        None,
        None,
        visibility,
    );
}

fn collect_import_decl(u: &ImportDecl, table: &mut SymbolTable) {
    // Each imported name is a reference; target resolution for cross-file
    // go-to-definition is handled separately.
    collect_import_or_include_names(&u.kind, table);
}

fn collect_include_decl(u: &graphcal_compiler::syntax::ast::IncludeDecl, table: &mut SymbolTable) {
    collect_import_or_include_names(&u.kind, table);
}

fn collect_import_or_include_names(
    kind: &graphcal_compiler::syntax::ast::ImportKind,
    table: &mut SymbolTable,
) {
    if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = kind {
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
    expr: &graphcal_compiler::syntax::ast::Expr,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    match &expr.kind {
        ExprKind::GraphRef(name) | ExprKind::ConstRef(name) => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::TopLevel(name.value.to_string()),
            });
        }
        ExprKind::QualifiedGraphRef { module, name }
        | ExprKind::QualifiedConstRef { module, name } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::Qualified {
                    module: module.name.clone(),
                    name: name.value.to_string(),
                },
            });
        }
        ExprKind::InlineDagRef {
            module,
            dag,
            args,
            output: _,
        } => {
            let dag_target = module.as_ref().map_or_else(
                || SymbolKey::TopLevel(dag.value.to_string()),
                |m| SymbolKey::Qualified {
                    module: m.name.clone(),
                    name: dag.value.to_string(),
                },
            );
            table.references.push(ReferenceInfo {
                span: dag.span,
                target: dag_target,
            });
            for binding in args {
                collect_expr_refs(&binding.value, table, scopes);
            }
        }
        ExprKind::FnCall { name, args, .. } => {
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
                    if let TableIndexSpec::Named(spanned) = idx {
                        table.references.push(ReferenceInfo {
                            span: spanned.span,
                            target: SymbolKey::TopLevel(spanned.value.to_string()),
                        });
                    }
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
                let (detail, ref_info) = match &binding.index {
                    graphcal_compiler::syntax::ast::ForBindingIndex::Named(spanned) => {
                        let detail = format!("loop variable over {}", spanned.value);
                        let ref_info = Some(ReferenceInfo {
                            span: spanned.span,
                            target: SymbolKey::TopLevel(spanned.value.to_string()),
                        });
                        (detail, ref_info)
                    }
                    graphcal_compiler::syntax::ast::ForBindingIndex::Range { arg, .. } => {
                        let detail = format!("loop variable over range({arg})");
                        (detail, None)
                    }
                };
                if let Some(ri) = ref_info {
                    table.references.push(ri);
                }
                let var_name = binding.var.name.clone();
                let key = SymbolKey::ExprScoped {
                    kind: ExprScopeKind::For,
                    offset: binding.var.span.offset(),
                    local: var_name.clone(),
                };
                table.insert_definition(
                    key.clone(),
                    DefinitionInfo {
                        name: var_name.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: binding.var.span,
                        decl_span: binding.var.span,
                        type_description: None,
                        detail: Some(detail),
                        visibility: None,
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
                    graphcal_compiler::syntax::ast::IndexArg::Variant { index, variant } => {
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
                    graphcal_compiler::syntax::ast::IndexArg::Var(ident) => {
                        let target = scopes
                            .resolve(&ident.name)
                            .cloned()
                            .unwrap_or_else(|| SymbolKey::TopLevel(ident.name.clone()));
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target,
                        });
                    }
                    graphcal_compiler::syntax::ast::IndexArg::Expr(e) => {
                        collect_expr_refs(e, table, scopes);
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
                local: acc_name.name.clone(),
            };
            let val_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Scan,
                offset: scan_offset,
                local: val_name.name.clone(),
            };
            table.insert_definition(
                acc_key.clone(),
                DefinitionInfo {
                    name: acc_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: acc_name.span,
                    decl_span: acc_name.span,
                    type_description: None,
                    detail: Some("scan accumulator".to_string()),
                    visibility: None,
                },
            );
            table.insert_definition(
                val_key.clone(),
                DefinitionInfo {
                    name: val_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: val_name.span,
                    decl_span: val_name.span,
                    type_description: None,
                    detail: Some("scan value".to_string()),
                    visibility: None,
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
                local: prev_name.name.clone(),
            };
            let curr_key = SymbolKey::ExprScoped {
                kind: ExprScopeKind::Unfold,
                offset: unfold_offset,
                local: curr_name.name.clone(),
            };
            table.insert_definition(
                prev_key.clone(),
                DefinitionInfo {
                    name: prev_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: prev_name.span,
                    decl_span: prev_name.span,
                    type_description: None,
                    detail: Some("unfold previous step".to_string()),
                    visibility: None,
                },
            );
            table.insert_definition(
                curr_key.clone(),
                DefinitionInfo {
                    name: curr_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: curr_name.span,
                    decl_span: curr_name.span,
                    type_description: None,
                    detail: Some("unfold current step".to_string()),
                    visibility: None,
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
                            table.insert_definition(
                                var_key.clone(),
                                DefinitionInfo {
                                    name: var.name.clone(),
                                    category: SymbolCategory::LocalVar,
                                    name_span: var.span,
                                    decl_span: var.span,
                                    type_description: None,
                                    detail: Some(format!("bound from {variant_name}")),
                                    visibility: None,
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
        ExprKind::NameRef(ident) => {
            // Unresolved name reference -- treat as a reference to a top-level symbol.
            table.references.push(ReferenceInfo {
                span: ident.span,
                target: SymbolKey::TopLevel(ident.name.clone()),
            });
        }
        ExprKind::QualifiedNameRef { qualifier, member } => {
            // Unresolved qualified name reference.
            table.references.push(ReferenceInfo {
                span: member.span,
                target: SymbolKey::Qualified {
                    module: qualifier.name.clone(),
                    name: member.name.clone(),
                },
            });
        }
    }
}

/// Collect references from a type expression.
fn collect_type_expr_refs(
    type_expr: &graphcal_compiler::syntax::ast::TypeExpr,
    table: &mut SymbolTable,
) {
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
                match idx {
                    graphcal_compiler::syntax::ast::IndexExpr::Name(ident) => {
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target: SymbolKey::TopLevel(ident.name.clone()),
                        });
                    }
                    graphcal_compiler::syntax::ast::IndexExpr::NatLiteral(_, _)
                    | graphcal_compiler::syntax::ast::IndexExpr::NatExpr(_) => {
                        // No reference to resolve for literal integers or nat expressions
                    }
                }
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
fn collect_constraint_expr_refs(
    expr: &graphcal_compiler::syntax::ast::Expr,
    table: &mut SymbolTable,
) {
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
fn format_bound_expr(expr: &graphcal_compiler::syntax::ast::Expr) -> String {
    match &expr.kind {
        ExprKind::Number(v) => format_number(*v),
        ExprKind::Integer(v) => v.to_string(),
        ExprKind::UnitLiteral { value, unit } => {
            let num = format_number(*value);
            let unit_str = format_unit_expr_with_config(unit, true);
            format!("{num} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::syntax::ast::UnaryOp::Neg,
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
                ResolvedIndex::NatExpr(form, _) => form.format(),
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
                        "dim {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            SymbolCategory::Unit => {
                if let Some(unit_info) = registry.units.get_unit(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let scale_str = match &unit_info.scale {
                        UnitScale::Static(s) => format!("{s}"),
                        UnitScale::Dynamic { .. } => "dynamic".to_string(),
                    };
                    def_mut.type_description = Some(format!(
                        "{}, scale = {scale_str}",
                        registry.dimensions.format_dimension(&unit_info.dimension),
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
                                .map(graphcal_compiler::syntax::names::VariantName::as_str)
                                .collect();
                            def_mut.type_description = Some(format!("{{ {} }}", vs.join(", ")));
                        }
                        IndexKind::Range(data) => {
                            def_mut.type_description = Some(format!(
                                "range({}, {}, step: {})",
                                data.start, data.end, data.step
                            ));
                        }
                        IndexKind::RequiredNamed => {
                            def_mut.type_description = Some("(required)".to_string());
                        }
                        IndexKind::RequiredRange { dimension } => {
                            def_mut.type_description = Some(format!(
                                "(required, dim: {})",
                                registry.dimensions.format_dimension(dimension)
                            ));
                        }
                        IndexKind::NatRange { size } => {
                            def_mut.type_description = Some(format!("range({size})"));
                        }
                    }
                }
            }
            SymbolCategory::StructType => {
                if let Some(type_def) = registry.types.get_type(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let desc = type_def.union_members().map_or_else(
                        || {
                            // Record or unit type: show fields
                            let fields = type_def.fields();
                            if fields.is_empty() {
                                type_def.name.to_string()
                            } else {
                                let field_descs: Vec<String> =
                                    fields.iter().map(|f| f.name.to_string()).collect();
                                format!("{} {{ {} }}", type_def.name, field_descs.join(", "))
                            }
                        },
                        |members| {
                            // Union type: show members separated by |
                            let member_descs: Vec<String> =
                                members.iter().map(|m| m.name.to_string()).collect();
                            member_descs.join(" | ")
                        },
                    );
                    def_mut.type_description = Some(desc);
                }
            }
            _ => {}
        }
    }

    // Register field definitions from struct types so that `field::name`
    // references resolve to a definition with hover info.
    for type_def in registry.types.all_types() {
        for field in type_def.fields() {
            let field_key = SymbolKey::Field(field.name.to_string());
            if !table.definitions.contains_key(&field_key) {
                table.insert_definition(
                    field_key,
                    DefinitionInfo {
                        name: field.name.to_string(),
                        category: SymbolCategory::Field,
                        name_span: Span::new(0, 0),
                        decl_span: Span::new(0, 0),
                        type_description: None,
                        detail: Some(format!("field of {}", type_def.name)),
                        visibility: None,
                    },
                );
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
        let file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
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
    fn find_reference_at_offset() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
        let file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
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
            SymbolKey::Qualified {
                module: "params".to_string(),
                name: "dry_mass".to_string()
            }
            .to_string(),
            "params::dry_mass"
        );
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
            SymbolKey::ExprScoped {
                kind: ExprScopeKind::For,
                offset: 42,
                local: "temp".to_string()
            }
            .to_string(),
            "for@42::temp"
        );
    }

    #[test]
    fn symbol_key_helpers() {
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

    #[test]
    fn qualified_graph_ref_creates_qualified_key() {
        // When source has `@mod::name`, the reference target should be
        // SymbolKey::Qualified, not SymbolKey::TopLevel.
        let source = concat!(
            "import \"./lib.gcl\";\n",
            "node y: Dimensionless = @lib::x + 1.0;\n",
        );
        let ast = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&ast);

        let expected_key = SymbolKey::Qualified {
            module: "lib".to_string(),
            name: "x".to_string(),
        };
        let refs = table.find_all_references(&expected_key);
        assert_eq!(
            refs.len(),
            1,
            "should find one qualified reference to lib::x"
        );

        // Ensure no TopLevel("x") reference was created for the qualified ref.
        let top_level_refs = table.find_all_references(&SymbolKey::TopLevel("x".to_string()));
        assert!(
            top_level_refs.is_empty(),
            "qualified ref should not create a TopLevel key"
        );
    }
}
