//! `From<X<Raw>> for X<Desugared>` impls.
//!
//! Every phase-parameterized AST type gets a [`From`] impl converting its
//! `Raw` form to its `Desugared` form. Most are mechanical structural
//! pass-throughs; the only interesting case is [`DeclKind`], which expands
//! the `Sugar(RawDeclSugar::Multi(_))` variant via
//! [`crate::syntax::desugar::expand_multi_decl`] instead of pass-through.
//!
//! These impls let consumers say `vec_of_raw.into_iter().map(Into::into)` or
//! `option_of_raw.map(Into::into)` to lift any AST tree from `Raw` to
//! `Desugared`. The desugar pass uses them to produce `File<Desugared>`.
//!
//! # Why so many impls?
//!
//! Rust has no quantification over generic type *constructors*, so we
//! cannot write a single blanket `impl<T<P>> From<T<Raw>> for T<Desugared>`.
//! Each phase-parameterized type needs its own `From` impl.
//!
//! # Phase-invariant types
//!
//! Types without a `<P>` parameter (e.g., `Attribute`, `Ident`, `ModulePath`,
//! `DimExpr`, `UnitExpr`, `IndexExpr`, `NatExpr`, `MapEntryKey`,
//! `MatchPattern`, etc.) are used as-is in both phases — no conversion
//! needed.
//!
//! # `MultiDecl` family
//!
//! `MultiDecl`, `MultiDeclSlot`, `MultiDeclSlice`, `MultiDataRow` are
//! parameterized over `<P>` for symmetry but only ever instantiated with
//! `<Raw>` (they live exclusively inside `RawDeclSugar::Multi`). They have
//! no `From<…<Raw>> for …<Desugared>` impl because the desugar pass
//! eliminates them entirely via `expand_multi_decl`.

use crate::syntax::ast::{
    AssertBody, AssertDecl, ConstNodeDecl, DagDecl, DeclKind, Declaration, DomainBound, Encoding,
    Expr, ExprKind, FieldDecl, FieldInit, FigureDecl, File, GenericArg, GenericParam, IncludeDecl,
    IndexArg, IndexDecl, IndexDeclKind, LayerDecl, MapEntry, MarkSpec, MatchArm, NodeDecl,
    ParamBinding, ParamDecl, PlotDecl, PlotField, TupleMatchArm, TypeDecl, TypeExpr, TypeExprKind,
    UnionMember, UnionTypeDecl, UnitDecl, UnitDef,
};
use crate::syntax::phase::{Desugared, Raw, RawDeclSugar, RawExprSugar};

// ---------------------------------------------------------------------------
// File / Declaration / DeclKind
// ---------------------------------------------------------------------------

impl From<File<Raw>> for File<Desugared> {
    fn from(f: File<Raw>) -> Self {
        Self {
            declarations: f.declarations.into_iter().flat_map(convert_decl).collect(),
        }
    }
}

/// Convert one `Declaration<Raw>` into N `Declaration<Desugared>`.
///
/// Returns a `Vec` because multi-decl sugar expands one declaration into
/// many. All other variants produce exactly one output declaration.
fn convert_decl(d: Declaration<Raw>) -> Vec<Declaration<Desugared>> {
    let Declaration {
        attributes,
        visibility,
        kind,
        span,
    } = d;
    match kind {
        DeclKind::Sugar(RawDeclSugar::Multi(multi)) => {
            // `expand_multi_decl` produces `Declaration<Raw>` values (one per
            // slot, all Param/Node/ConstNode — never `Sugar`). Lift each to
            // `Declaration<Desugared>` so the rest of the pass sees a uniform
            // post-desugar type.
            crate::syntax::desugar::expand_multi_decl(&multi)
                .into_iter()
                .map(lift_non_sugar_decl)
                .collect()
        }
        other => vec![Declaration {
            attributes,
            visibility,
            kind: convert_decl_kind_non_sugar(other),
            span,
        }],
    }
}

/// Lift a `Declaration<Raw>` known to be non-sugar to `Declaration<Desugared>`.
///
/// Used on the output of `expand_multi_decl`, which only produces
/// `Param` / `Node` / `ConstNode` slots — never `Sugar`. Panics if a `Sugar`
/// variant is encountered (would indicate `expand_multi_decl` invariant
/// violation).
fn lift_non_sugar_decl(d: Declaration<Raw>) -> Declaration<Desugared> {
    Declaration {
        attributes: d.attributes,
        visibility: d.visibility,
        kind: convert_decl_kind_non_sugar(d.kind),
        span: d.span,
    }
}

/// Convert a non-sugar `DeclKind<Raw>` variant to `DeclKind<Desugared>`.
///
/// Panics if called with `DeclKind::Sugar(_)` — `convert_decl` handles that
/// case directly so it never reaches here.
#[expect(
    clippy::panic,
    reason = "invariant: convert_decl handles Sugar separately and never calls this with Sugar"
)]
fn convert_decl_kind_non_sugar(k: DeclKind<Raw>) -> DeclKind<Desugared> {
    match k {
        DeclKind::Param(p) => DeclKind::Param(p.into()),
        DeclKind::Node(n) => DeclKind::Node(n.into()),
        DeclKind::ConstNode(c) => DeclKind::ConstNode(c.into()),
        DeclKind::BaseDimension(d) => DeclKind::BaseDimension(d),
        DeclKind::Dimension(d) => DeclKind::Dimension(d),
        DeclKind::Unit(u) => DeclKind::Unit(u.into()),
        DeclKind::Type(t) => DeclKind::Type(t.into()),
        DeclKind::UnionType(u) => DeclKind::UnionType(u.into()),
        DeclKind::Index(i) => DeclKind::Index(i.into()),
        DeclKind::Import(i) => DeclKind::Import(i),
        DeclKind::Include(i) => DeclKind::Include(i.into()),
        DeclKind::Dag(d) => DeclKind::Dag(d.into()),
        DeclKind::Assert(a) => DeclKind::Assert(a.into()),
        DeclKind::Plot(p) => DeclKind::Plot(p.into()),
        DeclKind::Figure(f) => DeclKind::Figure(f.into()),
        DeclKind::Layer(l) => DeclKind::Layer(l.into()),
        DeclKind::Sugar(_) => {
            panic!("convert_decl_kind_non_sugar called with Sugar — caller must dispatch first")
        }
    }
}

// ---------------------------------------------------------------------------
// Decl-specific structs
// ---------------------------------------------------------------------------

impl From<ParamDecl<Raw>> for ParamDecl<Desugared> {
    fn from(p: ParamDecl<Raw>) -> Self {
        Self {
            name: p.name,
            type_ann: p.type_ann.into(),
            value: p.value.map(Into::into),
        }
    }
}

impl From<NodeDecl<Raw>> for NodeDecl<Desugared> {
    fn from(n: NodeDecl<Raw>) -> Self {
        Self {
            name: n.name,
            type_ann: n.type_ann.into(),
            value: n.value.into(),
        }
    }
}

impl From<ConstNodeDecl<Raw>> for ConstNodeDecl<Desugared> {
    fn from(c: ConstNodeDecl<Raw>) -> Self {
        Self {
            name: c.name,
            type_ann: c.type_ann.into(),
            value: c.value.into(),
        }
    }
}

impl From<UnitDecl<Raw>> for UnitDecl<Desugared> {
    fn from(u: UnitDecl<Raw>) -> Self {
        Self {
            name: u.name,
            dim_type: u.dim_type,
            definition: u.definition.map(Into::into),
        }
    }
}

impl From<UnitDef<Raw>> for UnitDef<Desugared> {
    fn from(u: UnitDef<Raw>) -> Self {
        Self {
            scale_expr: u.scale_expr.into(),
            unit_expr: u.unit_expr,
            span: u.span,
        }
    }
}

impl From<TypeDecl<Raw>> for TypeDecl<Desugared> {
    fn from(t: TypeDecl<Raw>) -> Self {
        Self {
            name: t.name,
            generic_params: t.generic_params.into_iter().map(Into::into).collect(),
            fields: t.fields.map(|fs| fs.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<UnionTypeDecl<Raw>> for UnionTypeDecl<Desugared> {
    fn from(u: UnionTypeDecl<Raw>) -> Self {
        Self {
            name: u.name,
            generic_params: u.generic_params.into_iter().map(Into::into).collect(),
            members: u.members.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<UnionMember<Raw>> for UnionMember<Desugared> {
    fn from(u: UnionMember<Raw>) -> Self {
        Self {
            name: u.name,
            type_args: u.type_args.into_iter().map(Into::into).collect(),
            span: u.span,
        }
    }
}

impl From<FieldDecl<Raw>> for FieldDecl<Desugared> {
    fn from(f: FieldDecl<Raw>) -> Self {
        Self {
            name: f.name,
            type_ann: f.type_ann.into(),
        }
    }
}

impl From<GenericParam<Raw>> for GenericParam<Desugared> {
    fn from(g: GenericParam<Raw>) -> Self {
        Self {
            name: g.name,
            constraint: g.constraint,
            default: g.default.map(Into::into),
        }
    }
}

impl From<IndexDecl<Raw>> for IndexDecl<Desugared> {
    fn from(i: IndexDecl<Raw>) -> Self {
        Self {
            name: i.name,
            kind: i.kind.into(),
        }
    }
}

impl From<IndexDeclKind<Raw>> for IndexDeclKind<Desugared> {
    fn from(k: IndexDeclKind<Raw>) -> Self {
        match k {
            IndexDeclKind::Named { variants } => Self::Named { variants },
            IndexDeclKind::Range { start, end, step } => Self::Range {
                start: Box::new((*start).into()),
                end: Box::new((*end).into()),
                step: Box::new((*step).into()),
            },
            IndexDeclKind::RequiredNamed => Self::RequiredNamed,
            IndexDeclKind::RequiredRange { dimension } => Self::RequiredRange { dimension },
        }
    }
}

impl From<IncludeDecl<Raw>> for IncludeDecl<Desugared> {
    fn from(i: IncludeDecl<Raw>) -> Self {
        Self {
            path: i.path,
            param_bindings: i.param_bindings.into_iter().map(Into::into).collect(),
            kind: i.kind,
        }
    }
}

impl From<ParamBinding<Raw>> for ParamBinding<Desugared> {
    fn from(p: ParamBinding<Raw>) -> Self {
        Self {
            name: p.name,
            value: p.value.into(),
            span: p.span,
        }
    }
}

impl From<DagDecl<Raw>> for DagDecl<Desugared> {
    fn from(d: DagDecl<Raw>) -> Self {
        Self {
            name: d.name,
            body: d.body.into_iter().flat_map(convert_decl).collect(),
            span: d.span,
        }
    }
}

impl From<AssertDecl<Raw>> for AssertDecl<Desugared> {
    fn from(a: AssertDecl<Raw>) -> Self {
        Self {
            name: a.name,
            body: a.body.into(),
        }
    }
}

impl From<AssertBody<Raw>> for AssertBody<Desugared> {
    fn from(b: AssertBody<Raw>) -> Self {
        match b {
            AssertBody::Expr(e) => Self::Expr(e.into()),
            AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                is_relative,
            } => Self::Tolerance {
                actual: Box::new((*actual).into()),
                expected: Box::new((*expected).into()),
                tolerance: Box::new((*tolerance).into()),
                is_relative,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Plot family
// ---------------------------------------------------------------------------

impl From<PlotDecl<Raw>> for PlotDecl<Desugared> {
    fn from(p: PlotDecl<Raw>) -> Self {
        Self {
            name: p.name,
            mark: p.mark.into(),
            encodings: p.encodings.into_iter().map(Into::into).collect(),
            properties: p.properties.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<MarkSpec<Raw>> for MarkSpec<Desugared> {
    fn from(m: MarkSpec<Raw>) -> Self {
        Self {
            mark_type: m.mark_type,
            mark_type_span: m.mark_type_span,
            properties: m.properties.into_iter().map(Into::into).collect(),
            span: m.span,
        }
    }
}

impl From<Encoding<Raw>> for Encoding<Desugared> {
    fn from(e: Encoding<Raw>) -> Self {
        Self {
            channel: e.channel,
            channel_span: e.channel_span,
            value: e.value.into(),
            span: e.span,
        }
    }
}

impl From<PlotField<Raw>> for PlotField<Desugared> {
    fn from(p: PlotField<Raw>) -> Self {
        Self {
            name: p.name,
            value: p.value.into(),
            span: p.span,
        }
    }
}

impl From<FigureDecl<Raw>> for FigureDecl<Desugared> {
    fn from(f: FigureDecl<Raw>) -> Self {
        Self {
            name: f.name,
            plot_names: f.plot_names,
            fields: f.fields.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<LayerDecl<Raw>> for LayerDecl<Desugared> {
    fn from(l: LayerDecl<Raw>) -> Self {
        Self {
            name: l.name,
            plot_names: l.plot_names,
            fields: l.fields.into_iter().map(Into::into).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Type expressions
// ---------------------------------------------------------------------------

impl From<TypeExpr<Raw>> for TypeExpr<Desugared> {
    fn from(t: TypeExpr<Raw>) -> Self {
        Self {
            kind: t.kind.into(),
            constraints: t.constraints.into_iter().map(Into::into).collect(),
            span: t.span,
        }
    }
}

impl From<TypeExprKind<Raw>> for TypeExprKind<Desugared> {
    fn from(k: TypeExprKind<Raw>) -> Self {
        match k {
            TypeExprKind::Dimensionless => Self::Dimensionless,
            TypeExprKind::Bool => Self::Bool,
            TypeExprKind::Int => Self::Int,
            TypeExprKind::Datetime => Self::Datetime,
            TypeExprKind::DimExpr(d) => Self::DimExpr(d),
            TypeExprKind::Indexed { base, indexes } => Self::Indexed {
                base: Box::new((*base).into()),
                indexes,
            },
            TypeExprKind::TypeApplication { name, type_args } => Self::TypeApplication {
                name,
                type_args: type_args.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl From<DomainBound<Raw>> for DomainBound<Desugared> {
    fn from(d: DomainBound<Raw>) -> Self {
        Self {
            kind: d.kind,
            kind_span: d.kind_span,
            value: d.value.into(),
            span: d.span,
        }
    }
}

impl From<GenericArg<Raw>> for GenericArg<Desugared> {
    fn from(g: GenericArg<Raw>) -> Self {
        match g {
            GenericArg::Type(t) => Self::Type(t.into()),
            GenericArg::Nat(n) => Self::Nat(n),
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl From<Expr<Raw>> for Expr<Desugared> {
    fn from(e: Expr<Raw>) -> Self {
        Self::new(e.kind.into(), e.span)
    }
}

impl From<ExprKind<Raw>> for ExprKind<Desugared> {
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive variant pass-through over a wide enum is inherently long"
    )]
    fn from(k: ExprKind<Raw>) -> Self {
        match k {
            // Phase-invariant payload — direct rebind.
            ExprKind::Number(n) => Self::Number(n),
            ExprKind::Integer(n) => Self::Integer(n),
            ExprKind::Bool(b) => Self::Bool(b),
            ExprKind::StringLiteral(s) => Self::StringLiteral(s),
            ExprKind::GraphRef(r) => Self::GraphRef(r),
            ExprKind::ConstRef(r) => Self::ConstRef(r),
            ExprKind::LocalRef(i) => Self::LocalRef(i),
            ExprKind::UnitLiteral { value, unit } => Self::UnitLiteral { value, unit },
            ExprKind::VariantLiteral { index, variant } => Self::VariantLiteral { index, variant },
            // `RefSugar` payload is `UnresolvedRef` in both `Raw` and
            // `Desugared` phases, so this is a direct rebind.
            ExprKind::UnresolvedRef(r) => Self::UnresolvedRef(r),
            // Recursive — convert children.
            ExprKind::BinOp { op, lhs, rhs } => Self::BinOp {
                op,
                lhs: Box::new((*lhs).into()),
                rhs: Box::new((*rhs).into()),
            },
            ExprKind::UnaryOp { op, operand } => Self::UnaryOp {
                op,
                operand: Box::new((*operand).into()),
            },
            ExprKind::FnCall {
                name,
                type_args,
                args,
            } => Self::FnCall {
                name,
                type_args: type_args.into_iter().map(Into::into).collect(),
                args: args.into_iter().map(Into::into).collect(),
            },
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => Self::If {
                condition: Box::new((*condition).into()),
                then_branch: Box::new((*then_branch).into()),
                else_branch: Box::new((*else_branch).into()),
            },
            ExprKind::Convert { expr, target } => Self::Convert {
                expr: Box::new((*expr).into()),
                target,
            },
            ExprKind::DisplayTimezone { expr, timezone } => Self::DisplayTimezone {
                expr: Box::new((*expr).into()),
                timezone,
            },
            ExprKind::AsCast { expr, target_type } => Self::AsCast {
                expr: Box::new((*expr).into()),
                target_type: target_type.into(),
            },
            ExprKind::FieldAccess { expr, field } => Self::FieldAccess {
                expr: Box::new((*expr).into()),
                field,
            },
            ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            } => Self::StructConstruction {
                type_name,
                type_args: type_args.into_iter().map(Into::into).collect(),
                fields: fields.into_iter().map(Into::into).collect(),
            },
            ExprKind::MapLiteral { entries } => Self::MapLiteral {
                entries: entries.into_iter().map(Into::into).collect(),
            },
            ExprKind::ForComp { bindings, body } => Self::ForComp {
                bindings,
                body: Box::new((*body).into()),
            },
            ExprKind::IndexAccess { expr, args } => Self::IndexAccess {
                expr: Box::new((*expr).into()),
                args: args.into_iter().map(Into::into).collect(),
            },
            ExprKind::Scan {
                source,
                init,
                acc_name,
                val_name,
                body,
            } => Self::Scan {
                source: Box::new((*source).into()),
                init: Box::new((*init).into()),
                acc_name,
                val_name,
                body: Box::new((*body).into()),
            },
            ExprKind::Unfold {
                init,
                prev_name,
                curr_name,
                body,
            } => Self::Unfold {
                init: Box::new((*init).into()),
                prev_name,
                curr_name,
                body: Box::new((*body).into()),
            },
            ExprKind::Match { scrutinee, arms } => Self::Match {
                scrutinee: Box::new((*scrutinee).into()),
                arms: arms.into_iter().map(Into::into).collect(),
            },
            ExprKind::TupleMatch { scrutinees, arms } => Self::TupleMatch {
                scrutinees: scrutinees.into_iter().map(Into::into).collect(),
                arms: arms.into_iter().map(Into::into).collect(),
            },
            ExprKind::InlineDagRef { path, args, output } => Self::InlineDagRef {
                path,
                args: args.into_iter().map(Into::into).collect(),
                output,
            },
            ExprKind::Sugar(RawExprSugar::TableLiteral {
                indexes: _,
                entries,
            }) => {
                // Drop the `indexes` metadata — the entries already carry
                // full `Index.Variant` keys (the parser materializes synthetic
                // names for `NatRange` axes during `parse_table_*`). The
                // `table` keyword is purely surface syntax preserved by the
                // formatter via the raw AST; downstream stages see the
                // canonical map form.
                Self::MapLiteral {
                    entries: entries.into_iter().map(Into::into).collect(),
                }
            }
        }
    }
}

impl From<MapEntry<Raw>> for MapEntry<Desugared> {
    fn from(m: MapEntry<Raw>) -> Self {
        Self {
            keys: m.keys,
            value: m.value.into(),
        }
    }
}

impl From<IndexArg<Raw>> for IndexArg<Desugared> {
    fn from(a: IndexArg<Raw>) -> Self {
        match a {
            IndexArg::Variant { index, variant } => Self::Variant { index, variant },
            IndexArg::Var(i) => Self::Var(i),
            IndexArg::Expr(e) => Self::Expr(Box::new((*e).into())),
        }
    }
}

impl From<FieldInit<Raw>> for FieldInit<Desugared> {
    fn from(f: FieldInit<Raw>) -> Self {
        Self {
            name: f.name,
            value: f.value.map(Into::into),
        }
    }
}

impl From<MatchArm<Raw>> for MatchArm<Desugared> {
    fn from(a: MatchArm<Raw>) -> Self {
        Self {
            pattern: a.pattern,
            body: a.body.into(),
            span: a.span,
        }
    }
}

impl From<TupleMatchArm<Raw>> for TupleMatchArm<Desugared> {
    fn from(a: TupleMatchArm<Raw>) -> Self {
        Self {
            patterns: a
                .patterns
                .map(|ps| ps.into_iter().map(Into::into).collect()),
            body: a.body.into(),
            span: a.span,
        }
    }
}
