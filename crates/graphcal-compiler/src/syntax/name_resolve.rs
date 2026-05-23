//! Name resolution pass.
//!
//! Rewrites [`UnresolvedRef`] payloads in `ExprKind::UnresolvedRef` into
//! concrete expression kinds, producing a
//! [`File<Resolved>`](crate::syntax::ast::File) in which the unresolved-ref
//! slot is `Infallible` (so unresolved references are statically impossible
//! to construct downstream).
//!
//! Runs after parsing and desugaring but before the rest of the compilation
//! pipeline. Resolves bare identifiers and qualified references using:
//!
//! - Builtin constants (PI, E, TAU, etc.)
//! - Time scale names (UTC, TAI, etc.)
//! - Local scope (for/scan/unfold/match bindings)
//! - Struct/union type names (declared in the file)
//! - Index names and their variants (declared in the file)
//!
//! The pass is a pure transformation from
//! [`crate::desugar::desugared_ast::File`] (= `File<Desugared>`) to
//! [`crate::desugar::resolved_ast::File`] (= `File<Resolved>`) — the input
//! is consumed and a new value is returned. Mutation in place is intentionally
//! avoided so name resolution behaves as a functional core transform.

use std::collections::{HashMap, HashSet};

use crate::desugar::desugared_ast as src_ast;
use crate::desugar::resolved_ast as dst_ast;
use crate::registry::builtins::builtin_constants;
use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::UnresolvedRef;
use crate::syntax::ast::{ImportItemNamespace, ImportKind, TypeSystemRefKind};
use crate::syntax::names::{
    ConstructorName, DimName, GenericParamName, IndexName, IndexVariantName, LocalName,
    ModuleAliasName, ScopedName, StructTypeName, TimeScaleName,
};
use crate::syntax::phase::never;
use crate::syntax::span::Spanned;

/// Context for name resolution: what names are in scope.
struct ResolveContext {
    /// Builtin constants: PI, E, TAU, SQRT2, etc.
    builtin_consts: &'static HashMap<&'static str, f64>,
    /// Struct and union type names declared in the file.
    type_names: HashSet<StructTypeName>,
    /// Dimension names declared in the file.
    dim_names: HashSet<DimName>,
    /// Constructor names declared in the file.
    constructor_names: HashSet<ConstructorName>,
    /// Imported type-system names whose concrete category is resolved later.
    imported_type_system_names: HashSet<StructTypeName>,
    /// Index name → set of variant names.
    index_variants: HashMap<IndexName, HashSet<IndexVariantName>>,
    /// Module aliases from imports (for qualified const refs).
    module_names: HashSet<ModuleAliasName>,
    /// Stack of local scopes (for/scan/unfold/match bindings).
    local_scopes: Vec<HashSet<LocalName>>,
    /// Stack of generic parameter scopes for type-level name resolution.
    generic_scopes: Vec<HashMap<GenericParamName, src_ast::GenericConstraint>>,
}

impl ResolveContext {
    fn is_local(&self, name: &str) -> bool {
        self.local_scopes.iter().rev().any(|s| s.contains(name))
    }

    fn push_scope(&mut self, names: HashSet<LocalName>) {
        self.local_scopes.push(names);
    }

    fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }

    fn generic_constraint(&self, name: &str) -> Option<src_ast::GenericConstraint> {
        self.generic_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn push_generic_scope(&mut self, params: &[src_ast::GenericParam]) {
        self.generic_scopes.push(
            params
                .iter()
                .map(|p| (p.name.value.clone(), p.constraint))
                .collect(),
        );
    }

    fn pop_generic_scope(&mut self) {
        self.generic_scopes.pop();
    }
}

/// Resolve a standalone expression with no file-level scope.
///
/// Used for inputs that the CLI parses outside of any file context — e.g.
/// `--set name=expr` override values. Bare identifiers resolve only against
/// builtin constants and time-scale names; everything else falls back to
/// `LocalRef`. Qualified `a.b` references resolve to module-qualified
/// `ConstRef` (there are no in-scope indexes to disambiguate against).
///
/// For expressions that need a file's resolution context (e.g. references to
/// an index variant declared in the target file), feed them through a
/// synthetic file and call [`resolve_name_refs`] instead.
#[must_use]
pub fn resolve_standalone_expr(expr: src_ast::Expr) -> dst_ast::Expr {
    let mut ctx = ResolveContext {
        builtin_consts: builtin_constants(),
        type_names: HashSet::new(),
        dim_names: HashSet::new(),
        constructor_names: HashSet::new(),
        imported_type_system_names: HashSet::new(),
        index_variants: HashMap::new(),
        module_names: HashSet::new(),
        local_scopes: Vec::new(),
        generic_scopes: Vec::new(),
    };
    lift_expr(expr, &mut ctx)
}

/// Resolve all unresolved-ref nodes in a file.
///
/// Consumes a [`File<Desugared>`](src_ast::File) and produces a
/// [`File<Resolved>`](dst_ast::File). The returned file has no
/// `ExprKind::UnresolvedRef` nodes (`RefSugar = Infallible` for `Resolved`),
/// so downstream consumers — IR lowering, TIR, evaluation — can assume the
/// AST is free of unresolved references at the type level.
#[must_use]
pub fn resolve_name_refs(file: src_ast::File) -> dst_ast::File {
    let builtin_consts = builtin_constants();

    // First pass: scan declarations to build the resolution context.
    let mut type_names = HashSet::new();
    let mut dim_names = HashSet::new();
    let mut constructor_names = HashSet::new();
    let mut imported_type_system_names = HashSet::new();
    let mut index_variants: HashMap<IndexName, HashSet<IndexVariantName>> = HashMap::new();
    let mut module_names = HashSet::new();
    collect_names_from_decls(
        &file.declarations,
        &mut type_names,
        &mut dim_names,
        &mut constructor_names,
        &mut imported_type_system_names,
        &mut index_variants,
        &mut module_names,
    );

    let mut ctx = ResolveContext {
        builtin_consts,
        type_names,
        dim_names,
        constructor_names,
        imported_type_system_names,
        index_variants,
        module_names,
        local_scopes: Vec::new(),
        generic_scopes: Vec::new(),
    };

    // Second pass: lift every declaration into the resolved AST.
    let declarations = file
        .declarations
        .into_iter()
        .map(|decl| lift_decl(decl, &mut ctx))
        .collect();

    dst_ast::File { declarations }
}

/// Collect type names, index variants, and module names from declarations.
fn collect_names_from_decls(
    decls: &[src_ast::Declaration],
    type_names: &mut HashSet<StructTypeName>,
    dim_names: &mut HashSet<DimName>,
    constructor_names: &mut HashSet<ConstructorName>,
    imported_type_system_names: &mut HashSet<StructTypeName>,
    index_variants: &mut HashMap<IndexName, HashSet<IndexVariantName>>,
    module_names: &mut HashSet<ModuleAliasName>,
) {
    for decl in decls {
        match &decl.kind {
            src_ast::DeclKind::Type(t) => {
                type_names.insert(t.name.value.clone());
                if t.fields.is_some() {
                    constructor_names.insert(ConstructorName::new(t.name.value.as_str()));
                }
            }
            src_ast::DeclKind::UnionType(u) => {
                type_names.insert(u.name.value.clone());
                for member in &u.members {
                    constructor_names.insert(member.name.value.clone());
                }
            }
            // Dim names are recognized so that a bare `Velocity` in an
            // include-binding RHS (`Speed: Velocity`) lowers to a
            // placeholder `StructConstruction` — the same shape the
            // binding-extraction path already accepts for index/type
            // bindings. Resolving "Velocity" elsewhere as a struct
            // construction is benign — downstream type checking
            // rejects the misuse with a precise diagnostic.
            src_ast::DeclKind::BaseDimension(d) => {
                dim_names.insert(d.name.value.clone());
            }
            src_ast::DeclKind::Dimension(d) => {
                dim_names.insert(d.name.value.clone());
            }
            src_ast::DeclKind::Index(idx) => {
                let idx_name = idx.name.value.clone();
                if let src_ast::IndexDeclKind::Named { variants } = &idx.kind {
                    let variant_set: HashSet<IndexVariantName> =
                        variants.iter().map(|v| v.value.clone()).collect();
                    index_variants.insert(idx_name, variant_set);
                } else {
                    index_variants.insert(idx_name, HashSet::new());
                }
            }
            src_ast::DeclKind::Import(import) => {
                if let ImportKind::Module { alias: Some(alias) } = &import.kind {
                    module_names.insert(alias.value.clone());
                }
                if let ImportKind::Selective(items) = &import.kind {
                    for item in items {
                        let local = item.local_name().to_string();
                        match item.namespace {
                            ImportItemNamespace::Type => {
                                imported_type_system_names.insert(StructTypeName::new(local));
                            }
                            ImportItemNamespace::Default => {
                                // The default namespace includes union
                                // constructors. Treating selective default
                                // imports as constructor candidates lets
                                // nullary constructors resolve as values; the
                                // project import pass later verifies the
                                // imported item category and visibility.
                                constructor_names.insert(ConstructorName::new(local));
                            }
                        }
                    }
                }
            }
            src_ast::DeclKind::Include(include) => {
                if let ImportKind::Module { alias: Some(alias) } = &include.kind {
                    module_names.insert(alias.value.clone());
                }
                if let ImportKind::Selective(items) = &include.kind {
                    for item in items {
                        let local = item.local_name().to_string();
                        constructor_names.insert(ConstructorName::new(local));
                    }
                }
            }
            src_ast::DeclKind::Dag(dag) => {
                collect_names_from_decls(
                    &dag.body,
                    type_names,
                    dim_names,
                    constructor_names,
                    imported_type_system_names,
                    index_variants,
                    module_names,
                );
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Declaration lifting
// ---------------------------------------------------------------------------

fn lift_decl(decl: src_ast::Declaration, ctx: &mut ResolveContext) -> dst_ast::Declaration {
    dst_ast::Declaration {
        attributes: decl.attributes,
        visibility: decl.visibility,
        kind: lift_decl_kind(decl.kind, ctx),
        span: decl.span,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive DeclKind match across every variant"
)]
fn lift_decl_kind(kind: src_ast::DeclKind, ctx: &mut ResolveContext) -> dst_ast::DeclKind {
    match kind {
        src_ast::DeclKind::Param(p) => dst_ast::DeclKind::Param(dst_ast::ParamDecl {
            name: p.name,
            type_ann: lift_type_expr(p.type_ann, ctx),
            value: p.value.map(|v| lift_expr(v, ctx)),
        }),
        src_ast::DeclKind::Node(n) => dst_ast::DeclKind::Node(dst_ast::NodeDecl {
            name: n.name,
            type_ann: lift_type_expr(n.type_ann, ctx),
            value: lift_expr(n.value, ctx),
        }),
        src_ast::DeclKind::ConstNode(c) => dst_ast::DeclKind::ConstNode(dst_ast::ConstNodeDecl {
            name: c.name,
            type_ann: lift_type_expr(c.type_ann, ctx),
            value: lift_expr(c.value, ctx),
        }),
        src_ast::DeclKind::BaseDimension(d) => dst_ast::DeclKind::BaseDimension(d),
        src_ast::DeclKind::Dimension(d) => dst_ast::DeclKind::Dimension(dst_ast::DimDecl {
            name: d.name,
            definition: d.definition.map(|def| lift_dim_expr(def, ctx)),
        }),
        src_ast::DeclKind::Unit(u) => dst_ast::DeclKind::Unit(dst_ast::UnitDecl {
            name: u.name,
            dim_type: lift_dim_expr(u.dim_type, ctx),
            definition: u.definition.map(|def| dst_ast::UnitDef {
                scale_expr: lift_expr(def.scale_expr, ctx),
                unit_expr: def.unit_expr,
                span: def.span,
            }),
        }),
        src_ast::DeclKind::Type(t) => dst_ast::DeclKind::Type(lift_type_decl(t, ctx)),
        src_ast::DeclKind::UnionType(u) => {
            dst_ast::DeclKind::UnionType(lift_union_type_decl(u, ctx))
        }
        src_ast::DeclKind::Index(i) => dst_ast::DeclKind::Index(dst_ast::IndexDecl {
            name: i.name,
            kind: match i.kind {
                src_ast::IndexDeclKind::Named { variants } => {
                    dst_ast::IndexDeclKind::Named { variants }
                }
                src_ast::IndexDeclKind::Range { start, end, step } => {
                    dst_ast::IndexDeclKind::Range {
                        start: Box::new(lift_expr(*start, ctx)),
                        end: Box::new(lift_expr(*end, ctx)),
                        step: Box::new(lift_expr(*step, ctx)),
                    }
                }
                src_ast::IndexDeclKind::RequiredNamed => dst_ast::IndexDeclKind::RequiredNamed,
                src_ast::IndexDeclKind::RequiredRange { dimension } => {
                    dst_ast::IndexDeclKind::RequiredRange {
                        dimension: lift_dim_expr(dimension, ctx),
                    }
                }
            },
        }),
        src_ast::DeclKind::Import(i) => dst_ast::DeclKind::Import(i),
        src_ast::DeclKind::Include(i) => dst_ast::DeclKind::Include(dst_ast::IncludeDecl {
            path: i.path,
            param_bindings: i
                .param_bindings
                .into_iter()
                .map(|b| dst_ast::ParamBinding {
                    name: b.name,
                    value: lift_expr(b.value, ctx),
                    span: b.span,
                })
                .collect(),
            kind: i.kind,
        }),
        src_ast::DeclKind::Dag(d) => dst_ast::DeclKind::Dag(lift_dag(d, ctx)),
        src_ast::DeclKind::Assert(a) => dst_ast::DeclKind::Assert(dst_ast::AssertDecl {
            name: a.name,
            body: match a.body {
                src_ast::AssertBody::Expr(e) => dst_ast::AssertBody::Expr(lift_expr(e, ctx)),
                src_ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    is_relative,
                } => dst_ast::AssertBody::Tolerance {
                    actual: Box::new(lift_expr(*actual, ctx)),
                    expected: Box::new(lift_expr(*expected, ctx)),
                    tolerance: Box::new(lift_expr(*tolerance, ctx)),
                    is_relative,
                },
            },
        }),
        src_ast::DeclKind::Plot(p) => dst_ast::DeclKind::Plot(dst_ast::PlotDecl {
            name: p.name,
            mark: dst_ast::MarkSpec {
                mark_type: p.mark.mark_type,
                mark_type_span: p.mark.mark_type_span,
                properties: p
                    .mark
                    .properties
                    .into_iter()
                    .map(|f| lift_plot_field(f, ctx))
                    .collect(),
                span: p.mark.span,
            },
            encodings: p
                .encodings
                .into_iter()
                .map(|e| dst_ast::Encoding {
                    channel: e.channel,
                    channel_span: e.channel_span,
                    value: lift_expr(e.value, ctx),
                    span: e.span,
                })
                .collect(),
            properties: p
                .properties
                .into_iter()
                .map(|f| lift_plot_field(f, ctx))
                .collect(),
        }),
        src_ast::DeclKind::Figure(f) => dst_ast::DeclKind::Figure(dst_ast::FigureDecl {
            name: f.name,
            plot_names: f.plot_names,
            fields: f
                .fields
                .into_iter()
                .map(|fld| lift_plot_field(fld, ctx))
                .collect(),
        }),
        src_ast::DeclKind::Layer(l) => dst_ast::DeclKind::Layer(dst_ast::LayerDecl {
            name: l.name,
            plot_names: l.plot_names,
            fields: l
                .fields
                .into_iter()
                .map(|fld| lift_plot_field(fld, ctx))
                .collect(),
        }),
        // `DeclKind::Sugar(_)` payload is `Infallible` in `Desugared` —
        // statically unreachable.
        src_ast::DeclKind::Sugar(s) => never(s),
    }
}

fn lift_dag(dag: src_ast::DagDecl, ctx: &mut ResolveContext) -> dst_ast::DagDecl {
    // The dag body may introduce its own types/indexes/module aliases that
    // are in scope only inside the body.
    let mut inner_types = HashSet::new();
    let mut inner_dims = HashSet::new();
    let mut inner_ctors = HashSet::new();
    let mut inner_imported_type_system = HashSet::new();
    let mut inner_indexes = HashMap::new();
    let mut inner_modules = HashSet::new();
    collect_names_from_decls(
        &dag.body,
        &mut inner_types,
        &mut inner_dims,
        &mut inner_ctors,
        &mut inner_imported_type_system,
        &mut inner_indexes,
        &mut inner_modules,
    );

    let orig_types = ctx.type_names.clone();
    let orig_dims = ctx.dim_names.clone();
    let orig_ctors = ctx.constructor_names.clone();
    let orig_imported_type_system = ctx.imported_type_system_names.clone();
    let orig_indexes = ctx.index_variants.clone();
    let orig_modules = ctx.module_names.clone();
    ctx.type_names.extend(inner_types);
    ctx.dim_names.extend(inner_dims);
    ctx.constructor_names.extend(inner_ctors);
    ctx.imported_type_system_names
        .extend(inner_imported_type_system);
    ctx.index_variants.extend(inner_indexes);
    ctx.module_names.extend(inner_modules);

    let body = dag.body.into_iter().map(|d| lift_decl(d, ctx)).collect();

    ctx.type_names = orig_types;
    ctx.dim_names = orig_dims;
    ctx.constructor_names = orig_ctors;
    ctx.imported_type_system_names = orig_imported_type_system;
    ctx.index_variants = orig_indexes;
    ctx.module_names = orig_modules;

    dst_ast::DagDecl {
        name: dag.name,
        body,
        span: dag.span,
    }
}

fn lift_plot_field(f: src_ast::PlotField, ctx: &mut ResolveContext) -> dst_ast::PlotField {
    dst_ast::PlotField {
        name: f.name,
        value: lift_expr(f.value, ctx),
        span: f.span,
    }
}

fn lift_type_decl(t: src_ast::TypeDecl, ctx: &mut ResolveContext) -> dst_ast::TypeDecl {
    ctx.push_generic_scope(&t.generic_params);
    let generic_params = t
        .generic_params
        .into_iter()
        .map(|g| lift_generic_param(g, ctx))
        .collect();
    let fields = t
        .fields
        .map(|fs| fs.into_iter().map(|f| lift_field_decl(f, ctx)).collect());
    ctx.pop_generic_scope();
    dst_ast::TypeDecl {
        name: t.name,
        generic_params,
        fields,
    }
}

fn lift_union_type_decl(
    u: src_ast::UnionTypeDecl,
    ctx: &mut ResolveContext,
) -> dst_ast::UnionTypeDecl {
    ctx.push_generic_scope(&u.generic_params);
    let generic_params = u
        .generic_params
        .into_iter()
        .map(|g| lift_generic_param(g, ctx))
        .collect();
    let members = u
        .members
        .into_iter()
        .map(|m| dst_ast::UnionMember {
            name: m.name,
            payload: m
                .payload
                .map(|fs| fs.into_iter().map(|f| lift_field_decl(f, ctx)).collect()),
            span: m.span,
        })
        .collect();
    ctx.pop_generic_scope();
    dst_ast::UnionTypeDecl {
        name: u.name,
        generic_params,
        members,
    }
}

fn lift_field_decl(f: src_ast::FieldDecl, ctx: &mut ResolveContext) -> dst_ast::FieldDecl {
    dst_ast::FieldDecl {
        name: f.name,
        type_ann: lift_type_expr(f.type_ann, ctx),
    }
}

fn lift_generic_param(g: src_ast::GenericParam, ctx: &mut ResolveContext) -> dst_ast::GenericParam {
    dst_ast::GenericParam {
        name: g.name,
        constraint: g.constraint,
        default: g.default.map(|t| lift_type_expr(t, ctx)),
    }
}

// ---------------------------------------------------------------------------
// Type expressions
// ---------------------------------------------------------------------------

fn lift_type_expr(t: src_ast::TypeExpr, ctx: &mut ResolveContext) -> dst_ast::TypeExpr {
    dst_ast::TypeExpr {
        kind: lift_type_expr_kind(t.kind, ctx),
        constraints: t
            .constraints
            .into_iter()
            .map(|c| dst_ast::DomainBound {
                kind: c.kind,
                kind_span: c.kind_span,
                value: lift_expr(c.value, ctx),
                span: c.span,
            })
            .collect(),
        span: t.span,
    }
}

fn lift_type_expr_kind(
    k: src_ast::TypeExprKind,
    ctx: &mut ResolveContext,
) -> dst_ast::TypeExprKind {
    match k {
        src_ast::TypeExprKind::Dimensionless => dst_ast::TypeExprKind::Dimensionless,
        src_ast::TypeExprKind::Bool => dst_ast::TypeExprKind::Bool,
        src_ast::TypeExprKind::Int => dst_ast::TypeExprKind::Int,
        src_ast::TypeExprKind::Datetime => dst_ast::TypeExprKind::Datetime,
        src_ast::TypeExprKind::DimExpr(d) => dst_ast::TypeExprKind::DimExpr(lift_dim_expr(d, ctx)),
        src_ast::TypeExprKind::Indexed { base, indexes } => dst_ast::TypeExprKind::Indexed {
            base: Box::new(lift_type_expr(*base, ctx)),
            indexes: indexes
                .into_iter()
                .map(|idx| lift_index_expr(idx, ctx))
                .collect(),
        },
        src_ast::TypeExprKind::TypeApplication { name, type_args } => {
            dst_ast::TypeExprKind::TypeApplication {
                name: lift_type_application_name(&name, ctx),
                type_args: type_args
                    .into_iter()
                    .map(|t| lift_type_expr(t, ctx))
                    .collect(),
            }
        }
        src_ast::TypeExprKind::DatetimeApplication { type_args } => {
            dst_ast::TypeExprKind::DatetimeApplication {
                type_args: type_args
                    .into_iter()
                    .map(|t| lift_type_expr(t, ctx))
                    .collect(),
            }
        }
    }
}

fn lift_type_application_name(
    name: &Spanned<src_ast::Ident>,
    ctx: &ResolveContext,
) -> Spanned<dst_ast::ResolvedTypeApplicationName> {
    let text = name.value.name.as_str();
    let value = match ctx.generic_constraint(text) {
        Some(src_ast::GenericConstraint::Type) => {
            dst_ast::ResolvedTypeApplicationName::GenericTypeParam(Spanned::new(
                GenericParamName::new(text),
                name.span,
            ))
        }
        _ if ctx.imported_type_system_names.contains(text) => {
            dst_ast::ResolvedTypeApplicationName::ImportedTypeSystem(Spanned::new(
                StructTypeName::new(text),
                name.span,
            ))
        }
        _ => dst_ast::ResolvedTypeApplicationName::Struct(Spanned::new(
            StructTypeName::new(text),
            name.span,
        )),
    };
    Spanned::new(value, name.span)
}

fn lift_dim_expr(d: src_ast::DimExpr, ctx: &ResolveContext) -> dst_ast::DimExpr {
    dst_ast::DimExpr {
        terms: d
            .terms
            .into_iter()
            .map(|item| dst_ast::DimExprItem {
                op: item.op,
                term: lift_dim_term(&item.term, ctx),
            })
            .collect(),
        span: d.span,
    }
}

fn lift_dim_term(term: &src_ast::DimTerm, ctx: &ResolveContext) -> dst_ast::DimTerm {
    let text = term.name.value.name.as_str();
    let name_span = term.name.span;
    let value = match ctx.generic_constraint(text) {
        Some(src_ast::GenericConstraint::Dim) => dst_ast::ResolvedDimTermName::GenericDimParam(
            Spanned::new(GenericParamName::new(text), name_span),
        ),
        _ if ctx.type_names.contains(text) => dst_ast::ResolvedDimTermName::StructType(
            Spanned::new(StructTypeName::new(text), name_span),
        ),
        _ if ctx.index_variants.contains_key(text) => {
            dst_ast::ResolvedDimTermName::Index(Spanned::new(IndexName::new(text), name_span))
        }
        _ if ctx.imported_type_system_names.contains(text) => {
            dst_ast::ResolvedDimTermName::ImportedTypeSystem(Spanned::new(
                StructTypeName::new(text),
                name_span,
            ))
        }
        _ => text.parse::<TimeScale>().map_or_else(
            |_| {
                dst_ast::ResolvedDimTermName::Dimension(Spanned::new(DimName::new(text), name_span))
            },
            |scale| {
                dst_ast::ResolvedDimTermName::TimeScale(Spanned::new(
                    TimeScaleName::new(scale),
                    name_span,
                ))
            },
        ),
    };
    dst_ast::DimTerm {
        name: Spanned::new(value, name_span),
        power: term.power,
        span: term.span,
    }
}

fn lift_index_expr(idx: src_ast::IndexExpr, ctx: &ResolveContext) -> dst_ast::IndexExpr {
    match idx {
        src_ast::IndexExpr::Name(name) => {
            let text = name.value.name.as_str();
            let value = match ctx.generic_constraint(text) {
                Some(src_ast::GenericConstraint::Nat) => {
                    dst_ast::ResolvedIndexExprName::GenericNatParam(Spanned::new(
                        GenericParamName::new(text),
                        name.span,
                    ))
                }
                Some(src_ast::GenericConstraint::Index) => {
                    dst_ast::ResolvedIndexExprName::GenericIndexParam(Spanned::new(
                        GenericParamName::new(text),
                        name.span,
                    ))
                }
                _ => dst_ast::ResolvedIndexExprName::Index(Spanned::new(
                    IndexName::new(text),
                    name.span,
                )),
            };
            dst_ast::IndexExpr::Name(Spanned::new(value, name.span))
        }
        src_ast::IndexExpr::NatLiteral(n, span) => dst_ast::IndexExpr::NatLiteral(n, span),
        src_ast::IndexExpr::NatExpr(nat_expr) => dst_ast::IndexExpr::NatExpr(nat_expr),
    }
}

fn lift_generic_arg(g: src_ast::GenericArg, ctx: &mut ResolveContext) -> dst_ast::GenericArg {
    match g {
        src_ast::GenericArg::Type(t) => dst_ast::GenericArg::Type(lift_type_expr(t, ctx)),
        src_ast::GenericArg::Nat(n) => dst_ast::GenericArg::Nat(n),
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

fn lift_expr(e: src_ast::Expr, ctx: &mut ResolveContext) -> dst_ast::Expr {
    let span = e.span;
    dst_ast::Expr::new(lift_expr_kind(e.kind, ctx, span), span)
}

#[expect(clippy::too_many_lines, reason = "exhaustive ExprKind match")]
fn lift_expr_kind(
    k: src_ast::ExprKind,
    ctx: &mut ResolveContext,
    span: crate::syntax::span::Span,
) -> dst_ast::ExprKind {
    use src_ast::ExprKind as S;

    match k {
        S::Number(n) => dst_ast::ExprKind::Number(n),
        S::Integer(n) => dst_ast::ExprKind::Integer(n),
        S::Bool(b) => dst_ast::ExprKind::Bool(b),
        S::StringLiteral(s) => dst_ast::ExprKind::StringLiteral(s),
        S::TypeSystemRef(r) => dst_ast::ExprKind::TypeSystemRef(r),
        S::GraphRef(r) => dst_ast::ExprKind::GraphRef(r),
        S::ConstRef(r) => dst_ast::ExprKind::ConstRef(r),
        S::LocalRef(i) => dst_ast::ExprKind::LocalRef(i),
        S::UnitLiteral { value, unit } => dst_ast::ExprKind::UnitLiteral { value, unit },
        S::VariantLiteral { index, variant } => {
            dst_ast::ExprKind::VariantLiteral { index, variant }
        }
        S::BinOp { op, lhs, rhs } => dst_ast::ExprKind::BinOp {
            op,
            lhs: Box::new(lift_expr(*lhs, ctx)),
            rhs: Box::new(lift_expr(*rhs, ctx)),
        },
        S::UnaryOp { op, operand } => dst_ast::ExprKind::UnaryOp {
            op,
            operand: Box::new(lift_expr(*operand, ctx)),
        },
        S::FnCall {
            name,
            type_args,
            args,
        } => dst_ast::ExprKind::FnCall {
            name,
            type_args: type_args
                .into_iter()
                .map(|t| lift_generic_arg(t, ctx))
                .collect(),
            args: args.into_iter().map(|a| lift_expr(a, ctx)).collect(),
        },
        S::If {
            condition,
            then_branch,
            else_branch,
        } => dst_ast::ExprKind::If {
            condition: Box::new(lift_expr(*condition, ctx)),
            then_branch: Box::new(lift_expr(*then_branch, ctx)),
            else_branch: Box::new(lift_expr(*else_branch, ctx)),
        },
        S::Convert { expr, target } => dst_ast::ExprKind::Convert {
            expr: Box::new(lift_expr(*expr, ctx)),
            target,
        },
        S::DisplayTimezone { expr, timezone } => dst_ast::ExprKind::DisplayTimezone {
            expr: Box::new(lift_expr(*expr, ctx)),
            timezone,
        },
        S::FieldAccess { expr, field } => dst_ast::ExprKind::FieldAccess {
            expr: Box::new(lift_expr(*expr, ctx)),
            field,
        },
        S::StructConstruction {
            type_name,
            type_args,
            fields,
        } => dst_ast::ExprKind::StructConstruction {
            type_name,
            type_args: type_args
                .into_iter()
                .map(|t| lift_type_expr(t, ctx))
                .collect(),
            fields: fields
                .into_iter()
                .map(|f| dst_ast::FieldInit {
                    name: f.name,
                    value: f.value.map(|v| lift_expr(v, ctx)),
                })
                .collect(),
        },
        S::MapLiteral { entries } => dst_ast::ExprKind::MapLiteral {
            entries: entries
                .into_iter()
                .map(|e| dst_ast::MapEntry {
                    keys: e.keys,
                    value: lift_expr(e.value, ctx),
                })
                .collect(),
        },
        S::IndexAccess { expr, args } => dst_ast::ExprKind::IndexAccess {
            expr: Box::new(lift_expr(*expr, ctx)),
            args: args
                .into_iter()
                .map(|a| match a {
                    src_ast::IndexArg::Variant { index, variant } => {
                        dst_ast::IndexArg::Variant { index, variant }
                    }
                    src_ast::IndexArg::Var(i) => dst_ast::IndexArg::Var(i),
                    src_ast::IndexArg::Expr(e) => {
                        dst_ast::IndexArg::Expr(Box::new(lift_expr(*e, ctx)))
                    }
                })
                .collect(),
        },
        S::ForComp { bindings, body } => {
            let mut scope = HashSet::new();
            for binding in &bindings {
                scope.insert(binding.var.value.clone());
            }
            ctx.push_scope(scope);
            let body = Box::new(lift_expr(*body, ctx));
            ctx.pop_scope();
            dst_ast::ExprKind::ForComp { bindings, body }
        }
        S::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            let source = Box::new(lift_expr(*source, ctx));
            let init = Box::new(lift_expr(*init, ctx));
            let scope = HashSet::from([acc_name.value.clone(), val_name.value.clone()]);
            ctx.push_scope(scope);
            let body = Box::new(lift_expr(*body, ctx));
            ctx.pop_scope();
            dst_ast::ExprKind::Scan {
                source,
                init,
                acc_name,
                val_name,
                body,
            }
        }
        S::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            let init = Box::new(lift_expr(*init, ctx));
            let scope = HashSet::from([prev_name.value.clone(), curr_name.value.clone()]);
            ctx.push_scope(scope);
            let body = Box::new(lift_expr(*body, ctx));
            ctx.pop_scope();
            dst_ast::ExprKind::Unfold {
                init,
                prev_name,
                curr_name,
                body,
            }
        }
        S::Match { scrutinee, arms } => {
            let scrutinee = Box::new(lift_expr(*scrutinee, ctx));
            let arms = arms
                .into_iter()
                .map(|arm| {
                    let mut scope = HashSet::new();
                    for binding in &arm.pattern.bindings {
                        if let crate::syntax::ast::PatternBinding::Bind { var, .. } = binding {
                            scope.insert(LocalName::new(&var.name));
                        }
                    }
                    ctx.push_scope(scope);
                    let body = lift_expr(arm.body, ctx);
                    ctx.pop_scope();
                    dst_ast::MatchArm {
                        pattern: arm.pattern,
                        body,
                        span: arm.span,
                    }
                })
                .collect();
            dst_ast::ExprKind::Match { scrutinee, arms }
        }
        S::TupleMatch { scrutinees, arms } => dst_ast::ExprKind::TupleMatch {
            scrutinees: scrutinees.map(|s| lift_expr(s, ctx)),
            arms: arms.map(|arm| dst_ast::TupleMatchArm {
                patterns: arm.patterns.map(|ps| ps.map(|p| lift_expr(p, ctx))),
                body: lift_expr(arm.body, ctx),
                span: arm.span,
            }),
        },
        S::InlineDagRef { path, args, output } => dst_ast::ExprKind::InlineDagRef {
            path,
            args: args
                .into_iter()
                .map(|b| dst_ast::ParamBinding {
                    name: b.name,
                    value: lift_expr(b.value, ctx),
                    span: b.span,
                })
                .collect(),
            output,
        },
        S::UnresolvedRef(r) => resolve_unresolved_ref(r, ctx, span),
        // `ExprSugar(_)` payload is `Infallible` in `Desugared` — statically
        // unreachable.
        S::Sugar(s) => never(s),
    }
}

// ---------------------------------------------------------------------------
// Actual unresolved-ref resolution
// ---------------------------------------------------------------------------

fn resolve_unresolved_ref(
    r: UnresolvedRef,
    ctx: &ResolveContext,
    _span: crate::syntax::span::Span,
) -> dst_ast::ExprKind {
    match r {
        UnresolvedRef::NameRef(ident) => resolve_name_ref(ident, ctx),
        UnresolvedRef::QualifiedNameRef { qualifier, member } => {
            resolve_qualified_name_ref(qualifier, member, ctx)
        }
    }
}

/// Resolve a bare `NameRef` to a concrete [`dst_ast::ExprKind`].
///
/// Priority:
/// 1. Local scope (for/scan/unfold/match bindings) → `LocalRef`
/// 2. Builtin constants (PI, E, etc.) → `ConstRef`
/// 3. Time scale names (UTC, TAI, etc.) → `ConstRef`
/// 4. Constructors → `StructConstruction` (bare, no fields)
/// 5. Type-system names → `TypeSystemRef`
/// 6. Fallback → `LocalRef` (will be caught later by semantic validation)
fn resolve_name_ref(ident: crate::syntax::ast::Ident, ctx: &ResolveContext) -> dst_ast::ExprKind {
    let name = &ident.name;

    if ctx.is_local(name) {
        return dst_ast::ExprKind::LocalRef(ident);
    }

    if ctx.builtin_consts.contains_key(name.as_str()) {
        return dst_ast::ExprKind::ConstRef(Spanned::new(
            ScopedName::local(name.as_str()),
            ident.span,
        ));
    }

    if name.parse::<TimeScale>().is_ok() {
        return dst_ast::ExprKind::ConstRef(Spanned::new(
            ScopedName::local(name.as_str()),
            ident.span,
        ));
    }

    if ctx.constructor_names.contains(name.as_str()) {
        return dst_ast::ExprKind::StructConstruction {
            type_name: Spanned::new(ConstructorName::new(name), ident.span),
            type_args: Vec::new(),
            fields: Vec::new(),
        };
    }

    if ctx.type_names.contains(name.as_str()) {
        return dst_ast::ExprKind::TypeSystemRef(Spanned::new(
            TypeSystemRefKind::Type(StructTypeName::new(name)),
            ident.span,
        ));
    }

    if ctx.dim_names.contains(name.as_str()) {
        return dst_ast::ExprKind::TypeSystemRef(Spanned::new(
            TypeSystemRefKind::Dimension(DimName::new(name)),
            ident.span,
        ));
    }

    if ctx.index_variants.contains_key(name.as_str()) {
        return dst_ast::ExprKind::TypeSystemRef(Spanned::new(
            TypeSystemRefKind::Index(IndexName::new(name)),
            ident.span,
        ));
    }

    if ctx.imported_type_system_names.contains(name.as_str()) {
        return dst_ast::ExprKind::TypeSystemRef(Spanned::new(
            TypeSystemRefKind::Imported(StructTypeName::new(name)),
            ident.span,
        ));
    }

    for variants in ctx.index_variants.values() {
        if variants.contains(name.as_str()) {
            return dst_ast::ExprKind::TypeSystemRef(Spanned::new(
                TypeSystemRefKind::BareVariant(IndexVariantName::new(name)),
                ident.span,
            ));
        }
    }

    dst_ast::ExprKind::LocalRef(ident)
}

/// Resolve a `QualifiedNameRef` (`a.b`) to a concrete [`dst_ast::ExprKind`].
///
/// Priority:
/// 1. If `qualifier` is a known index name → `VariantLiteral`
/// 2. Otherwise → `ConstRef` carrying a qualified `ScopedName`
///    (module-qualified constant, validated later)
fn resolve_qualified_name_ref(
    qualifier: crate::syntax::ast::Ident,
    member: crate::syntax::ast::Ident,
    ctx: &ResolveContext,
) -> dst_ast::ExprKind {
    if ctx.index_variants.contains_key(qualifier.name.as_str()) {
        return dst_ast::ExprKind::VariantLiteral {
            index: Spanned::new(IndexName::new(&qualifier.name), qualifier.span),
            variant: Spanned::new(IndexVariantName::new(&member.name), member.span),
        };
    }

    let merged_span = qualifier.span.merge(member.span);
    dst_ast::ExprKind::ConstRef(Spanned::new(
        ScopedName::qualified(qualifier.name, member.name),
        merged_span,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desugar::resolved_ast::{DeclKind, ExprKind};
    use crate::syntax::parser::Parser;

    fn resolve_source(source: &str) -> dst_ast::File {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let mut desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        crate::syntax::ast::desugar_tuple_matches(&mut desugared);
        resolve_name_refs(desugared)
    }

    #[test]
    fn default_selective_import_can_resolve_bare_constructor() {
        let file = resolve_source(
            "import academy.lib.{ WeightlessStudent };\n\
             node another_learner: Student = WeightlessStudent;",
        );

        let node = match &file.declarations[1].kind {
            DeclKind::Node(node) => Some(node),
            _ => None,
        }
        .unwrap();
        let (type_name, type_args, fields) = match &node.value.kind {
            ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            } => Some((type_name, type_args, fields)),
            _ => None,
        }
        .unwrap();
        assert_eq!(type_name.value.as_str(), "WeightlessStudent");
        assert!(type_args.is_empty());
        assert!(fields.is_empty());
    }

    #[test]
    fn explicit_type_import_stays_type_system_ref() {
        let file = resolve_source(
            "import academy.lib.{ type Student };\n\
             node learner: Dimensionless = Student;",
        );

        let node = match &file.declarations[1].kind {
            DeclKind::Node(node) => Some(node),
            _ => None,
        }
        .unwrap();
        let name = match &node.value.kind {
            ExprKind::TypeSystemRef(name) => Some(name),
            _ => None,
        }
        .unwrap();
        assert_eq!(name.value.as_str(), "Student");
    }
}
