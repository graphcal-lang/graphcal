//! Name resolution pass: rewrites [`ExprKind::NameRef`] and [`ExprKind::QualifiedNameRef`]
//! into concrete expression kinds.
//!
//! This pass runs after parsing and desugaring but before the rest of the compilation
//! pipeline. It resolves bare identifiers and qualified references using:
//!
//! - Builtin constants (PI, E, TAU, etc.)
//! - Time scale names (UTC, TAI, etc.)
//! - Local scope (for/scan/unfold/match bindings)
//! - Struct/union type names (declared in the file)
//! - Index names and their variants (declared in the file)
//!
//! After this pass, no `NameRef` or `QualifiedNameRef` nodes remain in the AST.

use std::collections::{HashMap, HashSet};

use crate::registry::builtins::builtin_constants;
use crate::registry::resolve_types::is_time_scale_name;
use crate::syntax::ast::{
    AssertBody, DagDecl, DeclKind, Expr, ExprKind, File, IndexArg, IndexDeclKind,
};
use crate::syntax::names::{DeclName, IndexName, Spanned, StructTypeName, VariantName};

/// Context for name resolution: what names are in scope.
struct ResolveContext {
    /// Builtin constants: PI, E, TAU, SQRT2, etc.
    builtin_consts: &'static HashMap<&'static str, f64>,
    /// Struct and union type names declared in the file.
    type_names: HashSet<String>,
    /// Index name → set of variant names.
    index_variants: HashMap<String, HashSet<String>>,
    /// Module aliases from imports (for qualified const refs).
    module_names: HashSet<String>,
    /// Stack of local scopes (for/scan/unfold/match bindings).
    local_scopes: Vec<HashSet<String>>,
}

impl ResolveContext {
    fn is_local(&self, name: &str) -> bool {
        self.local_scopes.iter().rev().any(|s| s.contains(name))
    }

    fn push_scope(&mut self, names: HashSet<String>) {
        self.local_scopes.push(names);
    }

    fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }
}

/// Resolve all `NameRef` and `QualifiedNameRef` nodes in a file.
///
/// This modifies the file's AST in place. After this function returns, no
/// `NameRef` or `QualifiedNameRef` nodes remain.
pub fn resolve_name_refs(file: &mut File) {
    let builtin_consts = builtin_constants();

    // Scan declarations to build context
    let mut type_names = HashSet::new();
    let mut index_variants: HashMap<String, HashSet<String>> = HashMap::new();
    let mut module_names = HashSet::new();

    collect_names_from_decls(
        &file.declarations,
        &mut type_names,
        &mut index_variants,
        &mut module_names,
    );

    let mut ctx = ResolveContext {
        builtin_consts,
        type_names,
        index_variants,
        module_names,
        local_scopes: Vec::new(),
    };

    for decl in &mut file.declarations {
        resolve_decl(decl, &mut ctx);
    }
}

/// Collect type names, index variants, and module names from declarations.
fn collect_names_from_decls(
    decls: &[crate::syntax::ast::Declaration],
    type_names: &mut HashSet<String>,
    index_variants: &mut HashMap<String, HashSet<String>>,
    module_names: &mut HashSet<String>,
) {
    for decl in decls {
        match &decl.kind {
            DeclKind::Type(t) => {
                type_names.insert(t.name.value.to_string());
            }
            DeclKind::UnionType(u) => {
                type_names.insert(u.name.value.to_string());
                // Union members are also valid type names for bare construction
                for member in &u.members {
                    type_names.insert(member.name.value.to_string());
                }
            }
            DeclKind::Index(idx) => {
                let idx_name = idx.name.value.to_string();
                if let IndexDeclKind::Named { variants } = &idx.kind {
                    let variant_set: HashSet<String> =
                        variants.iter().map(|v| v.value.to_string()).collect();
                    index_variants.insert(idx_name, variant_set);
                } else {
                    index_variants.insert(idx_name, HashSet::new());
                }
            }
            DeclKind::Import(import) => {
                if let crate::syntax::ast::ImportKind::Module { alias: Some(alias) } = &import.kind
                {
                    module_names.insert(alias.name.clone());
                }
                // Selective imports may bring in types/indexes — add them
                if let crate::syntax::ast::ImportKind::Selective(items) = &import.kind {
                    for item in items {
                        let local = item.local_name().to_string();
                        // We don't know if it's a type or index without resolving the import,
                        // so we add it to both sets optimistically. The resolve pass will
                        // validate later.
                        type_names.insert(local);
                    }
                }
            }
            DeclKind::Include(include) => {
                if let crate::syntax::ast::ImportKind::Module { alias: Some(alias) } = &include.kind
                {
                    module_names.insert(alias.name.clone());
                }
                if let crate::syntax::ast::ImportKind::Selective(items) = &include.kind {
                    for item in items {
                        let local = item.local_name().to_string();
                        type_names.insert(local);
                    }
                }
            }
            DeclKind::Dag(dag) => {
                // Recurse into dag body
                collect_names_from_decls(&dag.body, type_names, index_variants, module_names);
            }
            _ => {}
        }
    }
}

/// Resolve names in a single declaration.
fn resolve_decl(decl: &mut crate::syntax::ast::Declaration, ctx: &mut ResolveContext) {
    match &mut decl.kind {
        DeclKind::Param(p) => {
            if let Some(v) = &mut p.value {
                resolve_expr(v, ctx);
            }
        }
        DeclKind::Node(n) => resolve_expr(&mut n.value, ctx),
        DeclKind::ConstNode(c) => resolve_expr(&mut c.value, ctx),
        DeclKind::Unit(u) => {
            if let Some(def) = &mut u.definition {
                resolve_expr(&mut def.scale_expr, ctx);
            }
        }
        DeclKind::Assert(a) => match &mut a.body {
            AssertBody::Expr(e) => resolve_expr(e, ctx),
            AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                resolve_expr(actual, ctx);
                resolve_expr(expected, ctx);
                resolve_expr(tolerance, ctx);
            }
        },
        DeclKind::Plot(p) => {
            for encoding in &mut p.encodings {
                resolve_expr(&mut encoding.value, ctx);
            }
            for prop in &mut p.mark.properties {
                resolve_expr(&mut prop.value, ctx);
            }
            for prop in &mut p.properties {
                resolve_expr(&mut prop.value, ctx);
            }
        }
        DeclKind::Figure(f) => {
            for field in &mut f.fields {
                resolve_expr(&mut field.value, ctx);
            }
        }
        DeclKind::Layer(l) => {
            for field in &mut l.fields {
                resolve_expr(&mut field.value, ctx);
            }
        }
        DeclKind::Dag(dag) => resolve_dag(dag, ctx),
        DeclKind::Index(idx) => {
            // Linspace index has expressions
            if let IndexDeclKind::Range { start, end, step } = &mut idx.kind {
                resolve_expr(start, ctx);
                resolve_expr(end, ctx);
                resolve_expr(step, ctx);
            }
        }
        DeclKind::Include(include) => {
            for binding in &mut include.param_bindings {
                resolve_expr(&mut binding.value, ctx);
            }
        }
        DeclKind::BaseDimension(_)
        | DeclKind::Dimension(_)
        | DeclKind::Type(_)
        | DeclKind::UnionType(_)
        | DeclKind::Import(_) => {}
    }
}

/// Resolve names in a dag block.
fn resolve_dag(dag: &mut DagDecl, ctx: &mut ResolveContext) {
    // Dag body may introduce its own types/indexes
    let mut inner_types = HashSet::new();
    let mut inner_indexes = HashMap::new();
    let mut inner_modules = HashSet::new();
    collect_names_from_decls(
        &dag.body,
        &mut inner_types,
        &mut inner_indexes,
        &mut inner_modules,
    );

    // Temporarily extend context
    let orig_types = ctx.type_names.clone();
    let orig_indexes = ctx.index_variants.clone();
    let orig_modules = ctx.module_names.clone();
    ctx.type_names.extend(inner_types);
    ctx.index_variants.extend(inner_indexes);
    ctx.module_names.extend(inner_modules);

    for decl in &mut dag.body {
        resolve_decl(decl, ctx);
    }

    // Restore context
    ctx.type_names = orig_types;
    ctx.index_variants = orig_indexes;
    ctx.module_names = orig_modules;
}

/// Resolve names in an expression tree (recursive).
#[expect(clippy::too_many_lines, reason = "exhaustive ExprKind match")]
fn resolve_expr(expr: &mut Expr, ctx: &mut ResolveContext) {
    // First, recurse into children (some introduce scopes)
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. }
        | ExprKind::QualifiedGraphRef { .. }
        | ExprKind::QualifiedConstRef { .. } => return,

        ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                resolve_expr(&mut arg.value, ctx);
            }
            return;
        }

        // NameRef and QualifiedNameRef are handled below after this match
        ExprKind::NameRef(_) | ExprKind::QualifiedNameRef { .. } => {}

        ExprKind::BinOp { lhs, rhs, .. } => {
            resolve_expr(lhs, ctx);
            resolve_expr(rhs, ctx);
            return;
        }
        ExprKind::UnaryOp { operand, .. } => {
            resolve_expr(operand, ctx);
            return;
        }
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                resolve_expr(arg, ctx);
            }
            return;
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            resolve_expr(condition, ctx);
            resolve_expr(then_branch, ctx);
            resolve_expr(else_branch, ctx);
            return;
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. }
        | ExprKind::FieldAccess { expr: inner, .. } => {
            resolve_expr(inner, ctx);
            return;
        }
        ExprKind::IndexAccess {
            expr: inner, args, ..
        } => {
            resolve_expr(inner, ctx);
            for arg in args {
                if let IndexArg::Expr(e) = arg {
                    resolve_expr(e, ctx);
                }
            }
            return;
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(v) = &mut field.value {
                    resolve_expr(v, ctx);
                }
            }
            return;
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                resolve_expr(&mut entry.value, ctx);
            }
            return;
        }
        ExprKind::ForComp { bindings, body } => {
            let mut scope = HashSet::new();
            for binding in bindings.iter() {
                scope.insert(binding.var.name.clone());
            }
            ctx.push_scope(scope);
            resolve_expr(body, ctx);
            ctx.pop_scope();
            return;
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            resolve_expr(source, ctx);
            resolve_expr(init, ctx);
            let scope = HashSet::from([acc_name.name.clone(), val_name.name.clone()]);
            ctx.push_scope(scope);
            resolve_expr(body, ctx);
            ctx.pop_scope();
            return;
        }
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            resolve_expr(init, ctx);
            let scope = HashSet::from([prev_name.name.clone(), curr_name.name.clone()]);
            ctx.push_scope(scope);
            resolve_expr(body, ctx);
            ctx.pop_scope();
            return;
        }
        ExprKind::Match { scrutinee, arms } => {
            resolve_expr(scrutinee, ctx);
            for arm in arms {
                let mut scope = HashSet::new();
                for binding in &arm.pattern.bindings {
                    match binding {
                        crate::syntax::ast::PatternBinding::Bind { var, .. } => {
                            scope.insert(var.name.clone());
                        }
                        crate::syntax::ast::PatternBinding::Wildcard { .. } => {}
                    }
                }
                ctx.push_scope(scope);
                resolve_expr(&mut arm.body, ctx);
                ctx.pop_scope();
            }
            return;
        }
        ExprKind::TupleMatch { scrutinees, arms } => {
            for s in scrutinees {
                resolve_expr(s, ctx);
            }
            for arm in arms {
                if let Some(patterns) = &mut arm.patterns {
                    for p in patterns {
                        resolve_expr(p, ctx);
                    }
                }
                resolve_expr(&mut arm.body, ctx);
            }
            return;
        }
    }

    // Now resolve NameRef / QualifiedNameRef at this node
    match std::mem::replace(
        &mut expr.kind,
        ExprKind::Bool(false), // placeholder, will be overwritten
    ) {
        ExprKind::NameRef(ident) => {
            expr.kind = resolve_name_ref(ident, ctx);
        }
        ExprKind::QualifiedNameRef { qualifier, member } => {
            expr.kind = resolve_qualified_name_ref(qualifier, member, ctx);
        }
        other => {
            // Put it back — shouldn't happen
            expr.kind = other;
        }
    }
}

/// Resolve a bare `NameRef` to a concrete [`ExprKind`].
///
/// Priority:
/// 1. Local scope (for/scan/unfold/match bindings) → `LocalRef`
/// 2. Builtin constants (PI, E, etc.) → `ConstRef`
/// 3. Time scale names (UTC, TAI, etc.) → `ConstRef`
/// 4. Struct/union type names → `StructConstruction` (bare, no fields)
/// 5. Index variant names → `StructConstruction` (bare, for backward compat)
/// 6. Fallback → `LocalRef` (will be caught later by semantic validation)
fn resolve_name_ref(ident: crate::syntax::ast::Ident, ctx: &ResolveContext) -> ExprKind {
    let name = &ident.name;

    // 1. Local scope takes priority (shadowing)
    if ctx.is_local(name) {
        return ExprKind::LocalRef(ident);
    }

    // 2. Builtin constant
    if ctx.builtin_consts.contains_key(name.as_str()) {
        return ExprKind::ConstRef(Spanned::new(DeclName::new(name), ident.span));
    }

    // 3. Time scale name
    if is_time_scale_name(name) {
        return ExprKind::ConstRef(Spanned::new(DeclName::new(name), ident.span));
    }

    // 4. Struct/union type name → bare construction
    if ctx.type_names.contains(name.as_str()) {
        return ExprKind::StructConstruction {
            type_name: Spanned::new(StructTypeName::new(name), ident.span),
            type_args: Vec::new(),
            fields: Vec::new(),
        };
    }

    // 5. Check if it's a known index variant name (bare variant, e.g., `Nominal`)
    for variants in ctx.index_variants.values() {
        if variants.contains(name.as_str()) {
            // Treat as bare struct construction (same as current PascalCase behavior)
            return ExprKind::StructConstruction {
                type_name: Spanned::new(StructTypeName::new(name), ident.span),
                type_args: Vec::new(),
                fields: Vec::new(),
            };
        }
    }

    // 6. Fallback: treat as local ref (include aliases, etc.)
    // Semantic validation in resolve/deps will catch truly unknown names.
    ExprKind::LocalRef(ident)
}

/// Resolve a `QualifiedNameRef` (`a::b`) to a concrete [`ExprKind`].
///
/// Priority:
/// 1. If `qualifier` is a known index name → `VariantLiteral`
/// 2. Otherwise → `QualifiedConstRef` (module-qualified constant, validated later)
#[expect(
    clippy::needless_pass_by_value,
    reason = "Ident is small and consumed in some branches"
)]
fn resolve_qualified_name_ref(
    qualifier: crate::syntax::ast::Ident,
    member: crate::syntax::ast::Ident,
    ctx: &ResolveContext,
) -> ExprKind {
    // 1. Known index → variant literal
    if ctx.index_variants.contains_key(qualifier.name.as_str()) {
        return ExprKind::VariantLiteral {
            index: Spanned::new(IndexName::new(&qualifier.name), qualifier.span),
            variant: Spanned::new(VariantName::new(&member.name), member.span),
        };
    }

    // 2. Fallback: qualified const ref (e.g., module::CONST)
    ExprKind::QualifiedConstRef {
        module: qualifier,
        name: Spanned::new(DeclName::new(&member.name), member.span),
    }
}
