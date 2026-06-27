//! Format-insensitive structural equality for the [`Raw`] syntax tree.
//!
//! The formatter rewrites source text and must then guarantee that it changed
//! only *formatting* — never the program. [`FormatEquivalent`] is the single
//! place that defines what "same program" means at the syntax level: it
//! compares two [`File<Raw>`](crate::syntax::ast::File) trees while ignoring
//! source spans.
//!
//! # Why a dedicated trait instead of `PartialEq`
//!
//! Spans are load-bearing for diagnostics, so the AST's normal equality (where
//! present) considers them — two `Spanned` values that differ only in span are
//! deliberately *not* `PartialEq`-equal. The formatter needs the opposite
//! relation: trees that differ *only* in formatting are equivalent. Giving that
//! relation its own trait keeps the "ignore spans" decision in one auditable
//! location instead of scattering span normalization or ad-hoc `==` overrides
//! through the codebase.
//!
//! # Extension point for future formatter transformations
//!
//! The formatter may eventually reorder nodes — e.g. sorting declarations, or
//! canonicalizing the order of map entries. When that happens, the *only* code
//! that changes is the relevant impl in this module: [`File`]'s impl would
//! compare declaration multisets instead of zipped sequences, and so on. The
//! formatter's self-check, and every other caller, stays untouched. This module
//! intentionally owns the definition of formatter equivalence so that
//! flexibility lives in one place.
//!
//! # Completeness is compiler-enforced
//!
//! Every impl destructures its type *exhaustively* — no `..` rest patterns and
//! no `_ =>` variant wildcards — binding span-only fields to `_`. Adding a
//! field or a variant to the AST therefore fails to compile here until it is
//! accounted for. The equivalence check can never silently go stale and miss a
//! meaning-changing edit.

use crate::syntax::ast::{
    AssertBody, AssertDecl, Attribute, AttributeArg, BaseDimDecl, ConstNodeDecl, DagDecl, DeclKind,
    Declaration, DimDecl, DimExpr, DimExprItem, DimTerm, DomainBound, Encoding, Expr, ExprKind,
    FieldDecl, FieldInit, FigureDecl, File, ForBinding, ForBindingIndex, GenericArg, GenericParam,
    Ident, IdentPath, ImportDecl, ImportItem, ImportKind, IncludeDecl, IndexArg, IndexDecl,
    IndexDeclKind, IndexExpr, LayerDecl, MapEntry, MapEntryKey, MarkSpec, MatchArm, MatchPattern,
    ModulePath, MultiDataRow, MultiDecl, MultiDeclSharedAxes, MultiDeclSlice, MultiDeclSlot,
    MultiHeaderCell, MultiSlotAxis, MultiSlotColumnSpan, NatExpr, NodeDecl, ParamBinding,
    ParamDecl, PatternBinding, PlotDecl, PlotField, RawDeclSugar, RawExprSugar, TableIndexSpec,
    TypeDecl, TypeDeclBody, TypeExpr, TypeExprKind, UnionMember, UnitDecl, UnitDef, UnitExpr,
    UnitExprItem, UnresolvedRef,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::plot_name::PlotPropertyName;
use crate::syntax::span::Spanned;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

/// Structural equality of two [`Raw`](crate::syntax::phase::Raw) syntax trees
/// modulo formatting — currently, modulo source spans.
///
/// Returns `true` when `self` and `other` denote the same program. This is the
/// invariant the formatter must uphold: formatting changes spans (and, in the
/// future, possibly node order) but never meaning.
pub trait FormatEquivalent {
    /// Returns `true` if `self` and `other` are equivalent up to formatting.
    fn format_equivalent(&self, other: &Self) -> bool;
}

// ---------------------------------------------------------------------------
// Leaves: types that carry no spans, compared by their normal equality.
// ---------------------------------------------------------------------------

/// Implements [`FormatEquivalent`] via `PartialEq` for span-free leaf types.
///
/// These types contain no source positions, so format-equivalence collapses to
/// ordinary equality. Restricting this shortcut to a hand-listed set keeps
/// span-bearing types from accidentally opting into a span-sensitive compare.
macro_rules! format_equivalent_via_eq {
    ($($t:ty),+ $(,)?) => {
        $(impl FormatEquivalent for $t {
            fn format_equivalent(&self, other: &Self) -> bool {
                self == other
            }
        })+
    };
}

format_equivalent_via_eq!(
    bool,
    i32,
    i64,
    crate::syntax::dimension::Rational,
    u64,
    usize,
    String,
    // Identifier newtypes — written identity only, never a span.
    DeclName,
    DimName,
    UnitName,
    IndexName,
    IndexVariantName,
    FieldName,
    ConstructorName,
    StructTypeName,
    GenericParamName,
    PlotPropertyName,
    ScopedName,
    crate::syntax::local_name::LocalName,
    NamePath,
    ModuleAliasName,
    crate::syntax::dimension::UnitRef,
    // Closed enums with no payload spans.
    crate::syntax::ast::BinOp,
    crate::syntax::ast::UnaryOp,
    crate::syntax::ast::MulDivOp,
    crate::syntax::ast::Visibility,
    crate::syntax::ast::BindableVisibility,
    crate::syntax::ast::UnitConstness,
    crate::syntax::ast::MarkType,
    crate::syntax::ast::EncodingChannel,
    crate::syntax::ast::MultiSlotKind,
    crate::syntax::ast::GenericConstraint,
    crate::syntax::ast::DomainBoundKind,
    crate::syntax::ast::ImportItemNamespace,
    crate::syntax::ast::MapEntryIndex,
);

/// Numeric literals compare bit-for-bit.
///
/// Bit comparison (rather than `==`) makes the relation reflexive even for the
/// degenerate `NaN` case and sidesteps the `clippy::float_cmp` lint — two
/// literals are format-equivalent exactly when they are the same literal.
impl FormatEquivalent for f64 {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

// ---------------------------------------------------------------------------
// Generic containers.
// ---------------------------------------------------------------------------

impl<T: FormatEquivalent> FormatEquivalent for Box<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        (**self).format_equivalent(&**other)
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Option<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.format_equivalent(b),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl<T: FormatEquivalent> FormatEquivalent for [T] {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(a, b)| a.format_equivalent(b))
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Vec<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.as_slice().format_equivalent(other.as_slice())
    }
}

impl<T: FormatEquivalent> FormatEquivalent for NonEmpty<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.as_slice().format_equivalent(other.as_slice())
    }
}

/// A spanned value is equivalent to another when their payloads are — the span
/// is the formatting difference this whole trait exists to ignore.
impl<T: FormatEquivalent> FormatEquivalent for Spanned<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { value, span: _ } = self;
        let Self {
            value: other_value,
            span: _,
        } = other;
        value.format_equivalent(other_value)
    }
}

// ---------------------------------------------------------------------------
// `common.rs` nodes.
// ---------------------------------------------------------------------------

impl FormatEquivalent for Ident {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { name, span: _ } = self;
        let Self {
            name: other_name,
            span: _,
        } = other;
        name == other_name
    }
}

impl FormatEquivalent for Attribute {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            args,
            span: _,
        } = self;
        let Self {
            name: other_name,
            args: other_args,
            span: _,
        } = other;
        name.format_equivalent(other_name) && args.format_equivalent(other_args)
    }
}

impl FormatEquivalent for AttributeArg {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Path { segments, span: _ } => {
                let Self::Path {
                    segments: other_segments,
                    span: _,
                } = other
                else {
                    return false;
                };
                segments.format_equivalent(other_segments)
            }
            Self::RangeStep { step, span: _ } => {
                let Self::RangeStep {
                    step: other_step,
                    span: _,
                } = other
                else {
                    return false;
                };
                step == other_step
            }
            Self::Group { elements, span: _ } => {
                let Self::Group {
                    elements: other_elements,
                    span: _,
                } = other
                else {
                    return false;
                };
                elements.format_equivalent(other_elements)
            }
        }
    }
}

impl FormatEquivalent for ImportKind {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Selective(items) => {
                let Self::Selective(other_items) = other else {
                    return false;
                };
                items.format_equivalent(other_items)
            }
            Self::Module { alias } => {
                let Self::Module { alias: other_alias } = other else {
                    return false;
                };
                alias.format_equivalent(other_alias)
            }
        }
    }
}

impl FormatEquivalent for ModulePath {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { segments, span: _ } = self;
        let Self {
            segments: other_segments,
            span: _,
        } = other;
        segments.format_equivalent(other_segments)
    }
}

impl FormatEquivalent for ImportItem {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            attributes,
            is_pub,
            namespace,
            name,
            alias,
        } = self;
        let Self {
            attributes: other_attributes,
            is_pub: other_is_pub,
            namespace: other_namespace,
            name: other_name,
            alias: other_alias,
        } = other;
        attributes.format_equivalent(other_attributes)
            && is_pub.format_equivalent(other_is_pub)
            && namespace.format_equivalent(other_namespace)
            && name.format_equivalent(other_name)
            && alias.format_equivalent(other_alias)
    }
}

// ---------------------------------------------------------------------------
// `decl.rs` nodes.
// ---------------------------------------------------------------------------

impl FormatEquivalent for File {
    fn format_equivalent(&self, other: &Self) -> bool {
        // Extension point: to allow the formatter to reorder declarations,
        // compare `declarations` as a multiset here instead of positionally.
        let Self { declarations } = self;
        let Self {
            declarations: other_declarations,
        } = other;
        declarations.format_equivalent(other_declarations)
    }
}

impl FormatEquivalent for Declaration {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            attributes,
            kind,
            span: _,
        } = self;
        let Self {
            attributes: other_attributes,
            kind: other_kind,
            span: _,
        } = other;
        attributes.format_equivalent(other_attributes) && kind.format_equivalent(other_kind)
    }
}

impl FormatEquivalent for DeclKind {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Param(a) => {
                let Self::Param(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Node(a) => {
                let Self::Node(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::ConstNode(a) => {
                let Self::ConstNode(b) = other else {
                    return false;
                };
                a.format_equivalent(b)
            }
            Self::BaseDimension(a) => {
                let Self::BaseDimension(b) = other else {
                    return false;
                };
                a.format_equivalent(b)
            }
            Self::Dimension(a) => {
                let Self::Dimension(b) = other else {
                    return false;
                };
                a.format_equivalent(b)
            }
            Self::Unit(a) => {
                let Self::Unit(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Type(a) => {
                let Self::Type(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Index(a) => {
                let Self::Index(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Import(a) => {
                let Self::Import(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Include(a) => {
                let Self::Include(b) = other else {
                    return false;
                };
                a.format_equivalent(b)
            }
            Self::Dag(a) => {
                let Self::Dag(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Assert(a) => {
                let Self::Assert(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Plot(a) => {
                let Self::Plot(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Figure(a) => {
                let Self::Figure(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Layer(a) => {
                let Self::Layer(b) = other else { return false };
                a.format_equivalent(b)
            }
            Self::Sugar(a) => {
                let Self::Sugar(b) = other else { return false };
                a.format_equivalent(b)
            }
        }
    }
}

impl FormatEquivalent for RawDeclSugar {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self::Multi(a) = self;
        let Self::Multi(b) = other;
        a.format_equivalent(b)
    }
}

impl FormatEquivalent for ParamDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            type_ann,
            value,
        } = self;
        let Self {
            name: other_name,
            type_ann: other_type_ann,
            value: other_value,
        } = other;
        name.format_equivalent(other_name)
            && type_ann.format_equivalent(other_type_ann)
            && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for NodeDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            type_ann,
            value,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            type_ann: other_type_ann,
            value: other_value,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && type_ann.format_equivalent(other_type_ann)
            && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for ConstNodeDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            type_ann,
            value,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            type_ann: other_type_ann,
            value: other_value,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && type_ann.format_equivalent(other_type_ann)
            && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for BaseDimDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { visibility, name } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
        } = other;
        visibility.format_equivalent(other_visibility) && name.format_equivalent(other_name)
    }
}

impl FormatEquivalent for DimDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            definition,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            definition: other_definition,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && definition.format_equivalent(other_definition)
    }
}

impl FormatEquivalent for UnitDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            constness,
            name,
            dim_type,
            definition,
        } = self;
        let Self {
            visibility: other_visibility,
            constness: other_constness,
            name: other_name,
            dim_type: other_dim_type,
            definition: other_definition,
        } = other;
        visibility.format_equivalent(other_visibility)
            && constness.format_equivalent(other_constness)
            && name.format_equivalent(other_name)
            && dim_type.format_equivalent(other_dim_type)
            && definition.format_equivalent(other_definition)
    }
}

impl FormatEquivalent for UnitDef {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            scale_expr,
            unit_expr,
            span: _,
        } = self;
        let Self {
            scale_expr: other_scale_expr,
            unit_expr: other_unit_expr,
            span: _,
        } = other;
        scale_expr.format_equivalent(other_scale_expr)
            && unit_expr.format_equivalent(other_unit_expr)
    }
}

impl FormatEquivalent for TypeDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            generic_params,
            body,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            generic_params: other_generic_params,
            body: other_body,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && generic_params.format_equivalent(other_generic_params)
            && body.format_equivalent(other_body)
    }
}

impl FormatEquivalent for TypeDeclBody {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Required => matches!(other, Self::Required),
            Self::Constructors(members) => {
                let Self::Constructors(other_members) = other else {
                    return false;
                };
                members.format_equivalent(other_members)
            }
        }
    }
}

impl FormatEquivalent for UnionMember {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            payload,
            span: _,
        } = self;
        let Self {
            name: other_name,
            payload: other_payload,
            span: _,
        } = other;
        name.format_equivalent(other_name) && payload.format_equivalent(other_payload)
    }
}

impl FormatEquivalent for FieldDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { name, type_ann } = self;
        let Self {
            name: other_name,
            type_ann: other_type_ann,
        } = other;
        name.format_equivalent(other_name) && type_ann.format_equivalent(other_type_ann)
    }
}

impl FormatEquivalent for IndexDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            kind,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            kind: other_kind,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && kind.format_equivalent(other_kind)
    }
}

impl FormatEquivalent for IndexDeclKind {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Named { variants } => {
                let Self::Named {
                    variants: other_variants,
                } = other
                else {
                    return false;
                };
                variants.format_equivalent(other_variants)
            }
            Self::Range { start, end, step } => {
                let Self::Range {
                    start: other_start,
                    end: other_end,
                    step: other_step,
                } = other
                else {
                    return false;
                };
                start.format_equivalent(other_start)
                    && end.format_equivalent(other_end)
                    && step.format_equivalent(other_step)
            }
            Self::RequiredNamed => matches!(other, Self::RequiredNamed),
            Self::RequiredRange { dimension } => {
                let Self::RequiredRange {
                    dimension: other_dimension,
                } = other
                else {
                    return false;
                };
                dimension.format_equivalent(other_dimension)
            }
        }
    }
}

impl FormatEquivalent for GenericParam {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            constraint,
            default,
        } = self;
        let Self {
            name: other_name,
            constraint: other_constraint,
            default: other_default,
        } = other;
        name.format_equivalent(other_name)
            && constraint.format_equivalent(other_constraint)
            && default.format_equivalent(other_default)
    }
}

impl FormatEquivalent for ImportDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            path,
            kind,
        } = self;
        let Self {
            visibility: other_visibility,
            path: other_path,
            kind: other_kind,
        } = other;
        visibility.format_equivalent(other_visibility)
            && path.format_equivalent(other_path)
            && kind.format_equivalent(other_kind)
    }
}

impl FormatEquivalent for IncludeDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            path,
            param_bindings,
            kind,
        } = self;
        let Self {
            visibility: other_visibility,
            path: other_path,
            param_bindings: other_param_bindings,
            kind: other_kind,
        } = other;
        visibility.format_equivalent(other_visibility)
            && path.format_equivalent(other_path)
            && param_bindings.format_equivalent(other_param_bindings)
            && kind.format_equivalent(other_kind)
    }
}

impl FormatEquivalent for DagDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            body,
            span: _,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            body: other_body,
            span: _,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && body.format_equivalent(other_body)
    }
}

impl FormatEquivalent for AssertDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            body,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            body: other_body,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && body.format_equivalent(other_body)
    }
}

impl FormatEquivalent for AssertBody {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Expr(expr) => {
                let Self::Expr(other_expr) = other else {
                    return false;
                };
                expr.format_equivalent(other_expr)
            }
            Self::Tolerance {
                actual,
                expected,
                tolerance,
                is_relative,
            } => {
                let Self::Tolerance {
                    actual: other_actual,
                    expected: other_expected,
                    tolerance: other_tolerance,
                    is_relative: other_is_relative,
                } = other
                else {
                    return false;
                };
                actual.format_equivalent(other_actual)
                    && expected.format_equivalent(other_expected)
                    && tolerance.format_equivalent(other_tolerance)
                    && is_relative.format_equivalent(other_is_relative)
            }
        }
    }
}

impl FormatEquivalent for PlotDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            mark,
            encodings,
            properties,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            mark: other_mark,
            encodings: other_encodings,
            properties: other_properties,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && mark.format_equivalent(other_mark)
            && encodings.format_equivalent(other_encodings)
            && properties.format_equivalent(other_properties)
    }
}

impl FormatEquivalent for MarkSpec {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            mark_type,
            mark_type_span: _,
            properties,
            span: _,
        } = self;
        let Self {
            mark_type: other_mark_type,
            mark_type_span: _,
            properties: other_properties,
            span: _,
        } = other;
        mark_type.format_equivalent(other_mark_type)
            && properties.format_equivalent(other_properties)
    }
}

impl FormatEquivalent for Encoding {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            channel,
            channel_span: _,
            value,
            span: _,
        } = self;
        let Self {
            channel: other_channel,
            channel_span: _,
            value: other_value,
            span: _,
        } = other;
        channel.format_equivalent(other_channel) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for PlotField {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            value,
            span: _,
        } = self;
        let Self {
            name: other_name,
            value: other_value,
            span: _,
        } = other;
        name.format_equivalent(other_name) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for FigureDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            plot_names,
            fields,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            plot_names: other_plot_names,
            fields: other_fields,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && plot_names.format_equivalent(other_plot_names)
            && fields.format_equivalent(other_fields)
    }
}

impl FormatEquivalent for LayerDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            name,
            plot_names,
            fields,
        } = self;
        let Self {
            visibility: other_visibility,
            name: other_name,
            plot_names: other_plot_names,
            fields: other_fields,
        } = other;
        visibility.format_equivalent(other_visibility)
            && name.format_equivalent(other_name)
            && plot_names.format_equivalent(other_plot_names)
            && fields.format_equivalent(other_fields)
    }
}

impl FormatEquivalent for MultiDecl {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            slots,
            shared_axes,
            slot_axes,
            slices,
            span: _,
            table_expr_span: _,
        } = self;
        let Self {
            slots: other_slots,
            shared_axes: other_shared_axes,
            slot_axes: other_slot_axes,
            slices: other_slices,
            span: _,
            table_expr_span: _,
        } = other;
        slots.format_equivalent(other_slots)
            && shared_axes.format_equivalent(other_shared_axes)
            && slot_axes.format_equivalent(other_slot_axes)
            && slices.format_equivalent(other_slices)
    }
}

impl FormatEquivalent for MultiDeclSlot {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            visibility,
            kind,
            kind_span: _,
            name,
            type_ann,
            header_span: _,
        } = self;
        let Self {
            visibility: other_visibility,
            kind: other_kind,
            kind_span: _,
            name: other_name,
            type_ann: other_type_ann,
            header_span: _,
        } = other;
        visibility.format_equivalent(other_visibility)
            && kind.format_equivalent(other_kind)
            && name.format_equivalent(other_name)
            && type_ann.format_equivalent(other_type_ann)
    }
}

impl FormatEquivalent for MultiSlotAxis {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Underscore => matches!(other, Self::Underscore),
            Self::Axis(axis) => {
                let Self::Axis(other_axis) = other else {
                    return false;
                };
                axis.format_equivalent(other_axis)
            }
        }
    }
}

impl FormatEquivalent for MultiSlotColumnSpan {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Single(col) => {
                let Self::Single(other_col) = other else {
                    return false;
                };
                col.format_equivalent(other_col)
            }
            Self::Range {
                start,
                end,
                extra_axis,
            } => {
                let Self::Range {
                    start: other_start,
                    end: other_end,
                    extra_axis: other_extra_axis,
                } = other
                else {
                    return false;
                };
                start.format_equivalent(other_start)
                    && end.format_equivalent(other_end)
                    && extra_axis.format_equivalent(other_extra_axis)
            }
        }
    }
}

impl FormatEquivalent for MultiDeclSlice {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            prefix_keys,
            header_cells,
            header_span: _,
            column_layout,
            rows,
        } = self;
        let Self {
            prefix_keys: other_prefix_keys,
            header_cells: other_header_cells,
            header_span: _,
            column_layout: other_column_layout,
            rows: other_rows,
        } = other;
        prefix_keys.format_equivalent(other_prefix_keys)
            && header_cells.format_equivalent(other_header_cells)
            && column_layout.format_equivalent(other_column_layout)
            && rows.format_equivalent(other_rows)
    }
}

impl FormatEquivalent for MultiHeaderCell {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Underscore { span: _ } => matches!(other, Self::Underscore { .. }),
            Self::Variant {
                axis,
                variant,
                span: _,
            } => {
                let Self::Variant {
                    axis: other_axis,
                    variant: other_variant,
                    span: _,
                } = other
                else {
                    return false;
                };
                axis.format_equivalent(other_axis) && variant.format_equivalent(other_variant)
            }
        }
    }
}

impl FormatEquivalent for MultiDataRow {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            label,
            values,
            span: _,
        } = self;
        let Self {
            label: other_label,
            values: other_values,
            span: _,
        } = other;
        label.format_equivalent(other_label) && values.format_equivalent(other_values)
    }
}

// ---------------------------------------------------------------------------
// `value.rs` nodes.
// ---------------------------------------------------------------------------

impl FormatEquivalent for TypeExpr {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            kind,
            constraints,
            span: _,
        } = self;
        let Self {
            kind: other_kind,
            constraints: other_constraints,
            span: _,
        } = other;
        kind.format_equivalent(other_kind) && constraints.format_equivalent(other_constraints)
    }
}

impl FormatEquivalent for TypeExprKind {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Dimensionless => matches!(other, Self::Dimensionless),
            Self::Bool => matches!(other, Self::Bool),
            Self::Int => matches!(other, Self::Int),
            Self::Datetime => matches!(other, Self::Datetime),
            Self::DatetimeApplication { type_args } => {
                let Self::DatetimeApplication {
                    type_args: other_type_args,
                } = other
                else {
                    return false;
                };
                type_args.format_equivalent(other_type_args)
            }
            Self::DimExpr(dim) => {
                let Self::DimExpr(other_dim) = other else {
                    return false;
                };
                dim.format_equivalent(other_dim)
            }
            Self::Indexed { base, indexes } => {
                let Self::Indexed {
                    base: other_base,
                    indexes: other_indexes,
                } = other
                else {
                    return false;
                };
                base.format_equivalent(other_base) && indexes.format_equivalent(other_indexes)
            }
            Self::TypeApplication { name, type_args } => {
                let Self::TypeApplication {
                    name: other_name,
                    type_args: other_type_args,
                } = other
                else {
                    return false;
                };
                name.format_equivalent(other_name) && type_args.format_equivalent(other_type_args)
            }
        }
    }
}

impl FormatEquivalent for DomainBound {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            kind,
            kind_span: _,
            value,
            span: _,
        } = self;
        let Self {
            kind: other_kind,
            kind_span: _,
            value: other_value,
            span: _,
        } = other;
        kind.format_equivalent(other_kind) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for IndexExpr {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Name(name) => {
                let Self::Name(other_name) = other else {
                    return false;
                };
                name.format_equivalent(other_name)
            }
            Self::NatExpr(nat) => {
                let Self::NatExpr(other_nat) = other else {
                    return false;
                };
                nat.format_equivalent(other_nat)
            }
        }
    }
}

impl FormatEquivalent for DimExpr {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { terms, span: _ } = self;
        let Self {
            terms: other_terms,
            span: _,
        } = other;
        terms.format_equivalent(other_terms)
    }
}

impl FormatEquivalent for DimExprItem {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { op, term } = self;
        let Self {
            op: other_op,
            term: other_term,
        } = other;
        op.format_equivalent(other_op) && term.format_equivalent(other_term)
    }
}

impl FormatEquivalent for DimTerm {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            power,
            span: _,
        } = self;
        let Self {
            name: other_name,
            power: other_power,
            span: _,
        } = other;
        name.format_equivalent(other_name) && power.format_equivalent(other_power)
    }
}

impl FormatEquivalent for UnitExpr {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { terms, span: _ } = self;
        let Self {
            terms: other_terms,
            span: _,
        } = other;
        terms.format_equivalent(other_terms)
    }
}

impl FormatEquivalent for UnitExprItem {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { op, name, power } = self;
        let Self {
            op: other_op,
            name: other_name,
            power: other_power,
        } = other;
        op.format_equivalent(other_op)
            && name.format_equivalent(other_name)
            && power.format_equivalent(other_power)
    }
}

impl FormatEquivalent for UnresolvedRef {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self::Path(a) = self;
        let Self::Path(b) = other;
        a.format_equivalent(b)
    }
}

impl FormatEquivalent for IdentPath {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { segments } = self;
        let Self {
            segments: other_segments,
        } = other;
        segments.format_equivalent(other_segments)
    }
}

impl FormatEquivalent for ParamBinding {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            name,
            value,
            span: _,
        } = self;
        let Self {
            name: other_name,
            value: other_value,
            span: _,
        } = other;
        name.format_equivalent(other_name) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for TableIndexSpec {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Named(name) => {
                let Self::Named(other_name) = other else {
                    return false;
                };
                name.format_equivalent(other_name)
            }
            Self::NatRange(size, _span) => {
                let Self::NatRange(other_size, _other_span) = other else {
                    return false;
                };
                size.format_equivalent(other_size)
            }
        }
    }
}

impl FormatEquivalent for MultiDeclSharedAxes {
    fn format_equivalent(&self, other: &Self) -> bool {
        // Private fields; compare through the public accessors. The shape
        // (slice axes + a distinguished row axis) is fixed, so positional
        // comparison is total.
        self.slice_axes().format_equivalent(other.slice_axes())
            && self.row_axis().format_equivalent(other.row_axis())
    }
}

impl FormatEquivalent for MapEntryKey {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { index, variant } = self;
        let Self {
            index: other_index,
            variant: other_variant,
        } = other;
        index.format_equivalent(other_index) && variant.format_equivalent(other_variant)
    }
}

impl FormatEquivalent for MapEntry {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { keys, value } = self;
        let Self {
            keys: other_keys,
            value: other_value,
        } = other;
        keys.format_equivalent(other_keys) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for ForBinding {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { var, index } = self;
        let Self {
            var: other_var,
            index: other_index,
        } = other;
        var.format_equivalent(other_var) && index.format_equivalent(other_index)
    }
}

impl FormatEquivalent for ForBindingIndex {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Named(name) => {
                let Self::Named(other_name) = other else {
                    return false;
                };
                name.format_equivalent(other_name)
            }
            Self::Range { arg, span: _ } => {
                let Self::Range {
                    arg: other_arg,
                    span: _,
                } = other
                else {
                    return false;
                };
                arg.format_equivalent(other_arg)
            }
        }
    }
}

impl FormatEquivalent for NatExpr {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Literal(value, _span) => {
                let Self::Literal(other_value, _other_span) = other else {
                    return false;
                };
                value.format_equivalent(other_value)
            }
            Self::Var(ident) => {
                let Self::Var(other_ident) = other else {
                    return false;
                };
                ident.format_equivalent(other_ident)
            }
            Self::Add(lhs, rhs, _span) => {
                let Self::Add(other_lhs, other_rhs, _other_span) = other else {
                    return false;
                };
                lhs.format_equivalent(other_lhs) && rhs.format_equivalent(other_rhs)
            }
            Self::Mul(lhs, rhs, _span) => {
                let Self::Mul(other_lhs, other_rhs, _other_span) = other else {
                    return false;
                };
                lhs.format_equivalent(other_lhs) && rhs.format_equivalent(other_rhs)
            }
        }
    }
}

impl FormatEquivalent for GenericArg {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Type(type_expr) => {
                let Self::Type(other_type_expr) = other else {
                    return false;
                };
                type_expr.format_equivalent(other_type_expr)
            }
            Self::Nat(nat) => {
                let Self::Nat(other_nat) = other else {
                    return false;
                };
                nat.format_equivalent(other_nat)
            }
        }
    }
}

impl FormatEquivalent for IndexArg {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Variant { index, variant } => {
                let Self::Variant {
                    index: other_index,
                    variant: other_variant,
                } = other
                else {
                    return false;
                };
                index.format_equivalent(other_index) && variant.format_equivalent(other_variant)
            }
            Self::Var(ident) => {
                let Self::Var(other_ident) = other else {
                    return false;
                };
                ident.format_equivalent(other_ident)
            }
            Self::Expr(expr) => {
                let Self::Expr(other_expr) = other else {
                    return false;
                };
                expr.format_equivalent(other_expr)
            }
        }
    }
}

impl FormatEquivalent for FieldInit {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { name, value } = self;
        let Self {
            name: other_name,
            value: other_value,
        } = other;
        name.format_equivalent(other_name) && value.format_equivalent(other_value)
    }
}

impl FormatEquivalent for MatchArm {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self {
            pattern,
            body,
            span: _,
        } = self;
        let Self {
            pattern: other_pattern,
            body: other_body,
            span: _,
        } = other;
        pattern.format_equivalent(other_pattern) && body.format_equivalent(other_body)
    }
}

impl FormatEquivalent for MatchPattern {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Path {
                path,
                bindings,
                span: _,
            } => {
                let Self::Path {
                    path: other_path,
                    bindings: other_bindings,
                    span: _,
                } = other
                else {
                    return false;
                };
                path.format_equivalent(other_path) && bindings.format_equivalent(other_bindings)
            }
            Self::Constructor {
                name,
                bindings,
                span: _,
            } => {
                let Self::Constructor {
                    name: other_name,
                    bindings: other_bindings,
                    span: _,
                } = other
                else {
                    return false;
                };
                name.format_equivalent(other_name) && bindings.format_equivalent(other_bindings)
            }
            Self::IndexLabel {
                index,
                variant,
                span: _,
            } => {
                let Self::IndexLabel {
                    index: other_index,
                    variant: other_variant,
                    span: _,
                } = other
                else {
                    return false;
                };
                index.format_equivalent(other_index) && variant.format_equivalent(other_variant)
            }
        }
    }
}

impl FormatEquivalent for PatternBinding {
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Bind { field, var } => {
                let Self::Bind {
                    field: other_field,
                    var: other_var,
                } = other
                else {
                    return false;
                };
                field.format_equivalent(other_field) && var.format_equivalent(other_var)
            }
            Self::Wildcard { field, span: _ } => {
                let Self::Wildcard {
                    field: other_field,
                    span: _,
                } = other
                else {
                    return false;
                };
                field.format_equivalent(other_field)
            }
        }
    }
}

impl FormatEquivalent for RawExprSugar {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self::TableLiteral { indexes, entries } = self;
        let Self::TableLiteral {
            indexes: other_indexes,
            entries: other_entries,
        } = other;
        indexes.format_equivalent(other_indexes) && entries.format_equivalent(other_entries)
    }
}

impl FormatEquivalent for Expr {
    fn format_equivalent(&self, other: &Self) -> bool {
        // Mirrors the manual `Clone`: route each tree level through the
        // stack-growth guard so deep left-nested operator chains do not
        // overflow. The span is ignored — that is the whole point.
        crate::stack::with_stack_growth(|| self.kind.format_equivalent(&other.kind))
    }
}

impl FormatEquivalent for ExprKind {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per ExprKind variant; exhaustiveness is the point"
    )]
    fn format_equivalent(&self, other: &Self) -> bool {
        match self {
            Self::Number(value) => {
                let Self::Number(other_value) = other else {
                    return false;
                };
                value.format_equivalent(other_value)
            }
            Self::Integer(value) => {
                let Self::Integer(other_value) = other else {
                    return false;
                };
                value.format_equivalent(other_value)
            }
            Self::Bool(value) => {
                let Self::Bool(other_value) = other else {
                    return false;
                };
                value.format_equivalent(other_value)
            }
            Self::StringLiteral(value) => {
                let Self::StringLiteral(other_value) = other else {
                    return false;
                };
                value.format_equivalent(other_value)
            }
            Self::GraphRef(name) => {
                let Self::GraphRef(other_name) = other else {
                    return false;
                };
                name.format_equivalent(other_name)
            }
            Self::BinOp { op, lhs, rhs } => {
                let Self::BinOp {
                    op: other_op,
                    lhs: other_lhs,
                    rhs: other_rhs,
                } = other
                else {
                    return false;
                };
                op.format_equivalent(other_op)
                    && lhs.format_equivalent(other_lhs)
                    && rhs.format_equivalent(other_rhs)
            }
            Self::UnaryOp { op, operand } => {
                let Self::UnaryOp {
                    op: other_op,
                    operand: other_operand,
                } = other
                else {
                    return false;
                };
                op.format_equivalent(other_op) && operand.format_equivalent(other_operand)
            }
            Self::FnCall {
                callee,
                type_args,
                args,
            } => {
                let Self::FnCall {
                    callee: other_callee,
                    type_args: other_type_args,
                    args: other_args,
                } = other
                else {
                    return false;
                };
                callee.format_equivalent(other_callee)
                    && type_args.format_equivalent(other_type_args)
                    && args.format_equivalent(other_args)
            }
            Self::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let Self::If {
                    condition: other_condition,
                    then_branch: other_then_branch,
                    else_branch: other_else_branch,
                } = other
                else {
                    return false;
                };
                condition.format_equivalent(other_condition)
                    && then_branch.format_equivalent(other_then_branch)
                    && else_branch.format_equivalent(other_else_branch)
            }
            Self::UnitLiteral { value, unit } => {
                let Self::UnitLiteral {
                    value: other_value,
                    unit: other_unit,
                } = other
                else {
                    return false;
                };
                value.format_equivalent(other_value) && unit.format_equivalent(other_unit)
            }
            Self::Convert { expr, target } => {
                let Self::Convert {
                    expr: other_expr,
                    target: other_target,
                } = other
                else {
                    return false;
                };
                expr.format_equivalent(other_expr) && target.format_equivalent(other_target)
            }
            Self::DisplayTimezone { expr, timezone } => {
                let Self::DisplayTimezone {
                    expr: other_expr,
                    timezone: other_timezone,
                } = other
                else {
                    return false;
                };
                expr.format_equivalent(other_expr) && timezone.format_equivalent(other_timezone)
            }
            Self::FieldAccess { expr, field } => {
                let Self::FieldAccess {
                    expr: other_expr,
                    field: other_field,
                } = other
                else {
                    return false;
                };
                expr.format_equivalent(other_expr) && field.format_equivalent(other_field)
            }
            Self::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let Self::ConstructorCall {
                    callee: other_callee,
                    generic_args: other_generic_args,
                    fields: other_fields,
                } = other
                else {
                    return false;
                };
                callee.format_equivalent(other_callee)
                    && generic_args.format_equivalent(other_generic_args)
                    && fields.format_equivalent(other_fields)
            }
            Self::MapLiteral { entries } => {
                let Self::MapLiteral {
                    entries: other_entries,
                } = other
                else {
                    return false;
                };
                entries.format_equivalent(other_entries)
            }
            Self::ForComp { bindings, body } => {
                let Self::ForComp {
                    bindings: other_bindings,
                    body: other_body,
                } = other
                else {
                    return false;
                };
                bindings.format_equivalent(other_bindings) && body.format_equivalent(other_body)
            }
            Self::IndexAccess { expr, args } => {
                let Self::IndexAccess {
                    expr: other_expr,
                    args: other_args,
                } = other
                else {
                    return false;
                };
                expr.format_equivalent(other_expr) && args.format_equivalent(other_args)
            }
            Self::Scan {
                source,
                init,
                acc_name,
                val_name,
                body,
            } => {
                let Self::Scan {
                    source: other_source,
                    init: other_init,
                    acc_name: other_acc_name,
                    val_name: other_val_name,
                    body: other_body,
                } = other
                else {
                    return false;
                };
                source.format_equivalent(other_source)
                    && init.format_equivalent(other_init)
                    && acc_name.format_equivalent(other_acc_name)
                    && val_name.format_equivalent(other_val_name)
                    && body.format_equivalent(other_body)
            }
            Self::Unfold {
                init,
                prev_name,
                curr_name,
                body,
            } => {
                let Self::Unfold {
                    init: other_init,
                    prev_name: other_prev_name,
                    curr_name: other_curr_name,
                    body: other_body,
                } = other
                else {
                    return false;
                };
                init.format_equivalent(other_init)
                    && prev_name.format_equivalent(other_prev_name)
                    && curr_name.format_equivalent(other_curr_name)
                    && body.format_equivalent(other_body)
            }
            Self::Match { scrutinee, arms } => {
                let Self::Match {
                    scrutinee: other_scrutinee,
                    arms: other_arms,
                } = other
                else {
                    return false;
                };
                scrutinee.format_equivalent(other_scrutinee) && arms.format_equivalent(other_arms)
            }
            Self::InlineDagRef { path, args, output } => {
                let Self::InlineDagRef {
                    path: other_path,
                    args: other_args,
                    output: other_output,
                } = other
                else {
                    return false;
                };
                path.format_equivalent(other_path)
                    && args.format_equivalent(other_args)
                    && output.format_equivalent(other_output)
            }
            Self::UnresolvedRef(reference) => {
                let Self::UnresolvedRef(other_reference) = other else {
                    return false;
                };
                reference.format_equivalent(other_reference)
            }
            Self::Sugar(sugar) => {
                let Self::Sugar(other_sugar) = other else {
                    return false;
                };
                sugar.format_equivalent(other_sugar)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FormatEquivalent;
    use crate::syntax::ast::File;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> File {
        Parser::new(source)
            .parse_file()
            .unwrap_or_else(|err| panic!("test source should parse: {err:?}\n---\n{source}"))
    }

    /// Two parses of the same text are equivalent — reflexivity over real spans.
    #[test]
    fn identical_sources_are_equivalent() {
        let source = "node x: Dimensionless = 1.0 + 2.0 * 3.0;\n";
        assert!(parse(source).format_equivalent(&parse(source)));
    }

    /// Layout-only differences (whitespace, newlines) move every span but must
    /// not change equivalence — this is the property the formatter relies on.
    #[test]
    fn whitespace_differences_are_equivalent() {
        let dense = "node x:Dimensionless=1.0+2.0*3.0;node y:Dimensionless=@x;";
        let spaced = "
            node x: Dimensionless = 1.0 + 2.0 * 3.0;

            node y: Dimensionless = @x;
        ";
        assert!(parse(dense).format_equivalent(&parse(spaced)));
    }

    /// Comments live in source metadata, not the AST, so they never affect
    /// equivalence.
    #[test]
    fn comment_differences_are_equivalent() {
        let bare = "node x: Dimensionless = 1.0;\n";
        let commented = "// leading\nnode x: Dimensionless = 1.0; // trailing\n";
        assert!(parse(bare).format_equivalent(&parse(commented)));
    }

    #[test]
    fn number_literal_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\n";
        let b = "node x: Dimensionless = 2.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    #[test]
    fn operator_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0 + 2.0;\n";
        let b = "node x: Dimensionless = 1.0 - 2.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    #[test]
    fn name_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\n";
        let b = "node y: Dimensionless = 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    /// Operand order is structural: `a - b` differs from `b - a` even though
    /// the spans cover the same ranges.
    #[test]
    fn operand_order_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0 - 2.0;\n";
        let b = "node x: Dimensionless = 2.0 - 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    /// A dropped declaration must be caught — the canonical "formatting changed
    /// the program" failure.
    #[test]
    fn missing_declaration_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\nnode y: Dimensionless = 2.0;\n";
        let b = "node x: Dimensionless = 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }
}
