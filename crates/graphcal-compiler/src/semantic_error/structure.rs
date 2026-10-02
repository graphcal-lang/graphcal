//! Diagnostics of nominal struct types, their fields, and constructors.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::generic_param::{GenericArgArity, GenericParamId};
use crate::hir::expr::LocalId;
use crate::nat::NatPolyForm;
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::checked_type::{
    CheckedType, IndexDisplayName, StructTypeRef, Symbolic, TypeSpelling,
};
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::local_name::LocalName;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

/// A generic argument that is still symbolic where a concrete one is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolicGenericArgument {
    Index(IndexDisplayName),
    Nat(NatPolyForm),
    Type(CheckedType<Symbolic>),
}

impl std::fmt::Display for SymbolicGenericArgument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(index) => index.fmt(f),
            Self::Nat(form) => f.write_str(&form.format()),
            Self::Type(ty) => write!(f, "{ty:?}"),
        }
    }
}

/// A generic parameter or expression left where a concrete type is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnboundGeneric {
    /// A quantity type's dimension: one parameter, or an expression over them.
    QuantityDimension(Option<GenericParamId>),
    /// A `Complex<D>` type's dimension.
    ComplexDimension(Option<GenericParamId>),
    /// A dimension generic argument.
    DimensionArgument(Option<GenericParamId>),
    TypeParameter(GenericParamId),
    IndexParameterAsType(GenericParamId),
    IndexParameter(GenericParamId),
    NatArgument(NatPolyForm),
    NatAxis(NatPolyForm),
    /// A generic parameter of a value type that inference cannot bind.
    NotConcretelyBound {
        sort: GenericSort,
        name: GenericParamName,
    },
}

/// The sort of a generic parameter, as diagnostics name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericSort {
    Type,
    Index,
    Dimension,
}

impl std::fmt::Display for GenericSort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Type => "type",
            Self::Index => "index",
            Self::Dimension => "dimension",
        })
    }
}

impl std::fmt::Display for UnboundGeneric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuantityDimension(Some(name)) => {
                write!(
                    f,
                    "cannot use generic dimension parameter `{name}` as a concrete type"
                )
            }
            Self::QuantityDimension(None) => {
                f.write_str("cannot use generic dimension expression as a concrete type")
            }
            Self::ComplexDimension(Some(name)) => {
                write!(f, "complex dimension parameter `{name}` is not bound")
            }
            Self::ComplexDimension(None) => {
                f.write_str("complex dimension expression is not concrete")
            }
            Self::DimensionArgument(Some(name)) => {
                write!(f, "generic dimension parameter `{name}` is not bound")
            }
            Self::DimensionArgument(None) => {
                f.write_str("generic dimension expression is not concrete")
            }
            Self::TypeParameter(name) => {
                write!(
                    f,
                    "cannot use generic type parameter `{name}` as a concrete type"
                )
            }
            Self::IndexParameterAsType(name) => {
                write!(
                    f,
                    "cannot use generic index parameter `{name}` as a concrete type"
                )
            }
            Self::IndexParameter(name) => {
                write!(f, "generic index parameter `{name}` is not bound")
            }
            Self::NatArgument(form) => {
                write!(
                    f,
                    "generic Nat argument `{}` is not concrete",
                    form.format()
                )
            }
            Self::NatAxis(form) => write!(
                f,
                "cannot use generic nat expression `{}` as a concrete type",
                form.format()
            ),
            Self::NotConcretelyBound { sort, name } => {
                write!(
                    f,
                    "generic {sort} parameter `{name}` is not concretely bound"
                )
            }
        }
    }
}

/// A member a nominal-type diagnostic names: a payload field, or a
/// constructor that does not belong to the scrutinized type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NominalMember {
    Field(FieldName),
    Constructor(ConstructorName),
}

impl std::fmt::Display for NominalMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Field(name) => name.fmt(f),
            Self::Constructor(name) => name.fmt(f),
        }
    }
}

/// The struct type an unknown-type diagnostic names: a canonical resolved
/// name (rendered owner-qualified) or a checked type reference (rendered by
/// its source name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownStructTypeName {
    Resolved(ResolvedStructTypeName),
    Checked(StructTypeRef),
}

impl std::fmt::Display for UnknownStructTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolved(name) => name.fmt(f),
            Self::Checked(name) => name.fmt(f),
        }
    }
}

/// The value a field access was attempted on when it has no fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldlessOperand {
    /// A value whose type is not a struct at all.
    NonStruct(TypeSpelling),
    /// A required (opaque) type, which declares no fields.
    RequiredType(StructTypeRef),
    /// A tagged union, whose fields are reached through `match`.
    Union(StructTypeRef),
}

impl std::fmt::Display for FieldlessOperand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonStruct(found) => found.fmt(f),
            Self::RequiredType(name) => write!(f, "required type `{name}` has no fields"),
            Self::Union(name) => write!(f, "union type `{name}` (use `match` to access fields)"),
        }
    }
}

/// A local the checker could not find: a source-named local or a lowered
/// local slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownLocal {
    Named(LocalName),
    Unbound(crate::syntax::names::NameAtom),
    Slot(LocalId),
}

impl std::fmt::Display for UnknownLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(name) => name.fmt(f),
            Self::Unbound(name) => name.fmt(f),
            Self::Slot(local) => write!(f, "#{}", local.index()),
        }
    }
}

/// Diagnostics of nominal struct types, their fields, and constructors.
#[derive(Debug, Clone, Error)]
pub enum StructError {
    #[error("unknown struct type `{name}`")]
    UnknownStructType { name: UnknownStructTypeName },
    #[error("unknown field `{member}` on struct `{type_name}`")]
    UnknownField {
        type_name: StructTypeName,
        member: NominalMember,
    },
    #[error("missing field(s) {missing:?} in construction of `{type_name}`")]
    MissingFields {
        type_name: StructTypeName,
        missing: Vec<FieldName>,
    },
    #[error("missing field(s) {missing:?} in match pattern for `{constructor}`")]
    MissingPatternFields {
        constructor: ConstructorName,
        missing: Vec<FieldName>,
    },
    #[error("constructor `{constructor}` cannot use empty parentheses")]
    EmptyParenthesizedConstructor { constructor: ConstructorName },
    #[error("extra field(s) {extra:?} in construction of `{type_name}`")]
    ExtraFields {
        type_name: StructTypeName,
        extra: Vec<FieldName>,
    },
    #[error("field `{field_name}` of `{type_name}`: expected dimension {expected}, found {found}")]
    FieldDimensionMismatch {
        type_name: StructTypeName,
        field_name: FieldName,
        expected: TypeSpelling,
        found: TypeSpelling,
    },
    #[error("cannot access field of non-struct value `{name}`")]
    NotAStruct { name: FieldlessOperand },
    #[error("unknown local variable `{name}`")]
    UnknownLocalRef { name: UnknownLocal },
    #[error("cannot match on `Key<{index}>`; only named-axis keys support label matching")]
    CannotMatchFiniteKey { index: IndexDisplayName },
    #[error("cannot match on coordinate index `{index}`; only named indexes can be matched")]
    CannotMatchCoordinateIndex { index: IndexDisplayName },
    #[error("label match arms must use index-label patterns")]
    LabelArmNotLabelPattern,
    #[error("duplicate match arm for variant `{variant}`")]
    DuplicateLabelArm { variant: IndexVariantName },
    #[error("non-exhaustive match: variant `{index}#{variant}` not covered")]
    NonExhaustiveLabelMatch {
        index: IndexDisplayName,
        variant: IndexVariantName,
    },
    #[error("union match arms must use constructor patterns")]
    UnionArmNotConstructorPattern,
    #[error("duplicate match arm for `{constructor}`")]
    DuplicateConstructorArm { constructor: ConstructorName },
    #[error("duplicate pattern binding for field `{field}` in `{constructor}`")]
    DuplicatePatternBinding {
        field: FieldName,
        constructor: ConstructorName,
    },
    #[error("non-exhaustive match: member `{member}` not covered")]
    NonExhaustiveUnionMatch { member: ConstructorName },
    #[error("cannot match on type `{found}`; expected a tagged union or label value")]
    UnmatchableType { found: TypeSpelling },
    #[error("match expression has no arms")]
    EmptyMatch,
    #[error("duplicate field `{field}` in constructor `{constructor}`")]
    DuplicateConstructionField {
        field: FieldName,
        constructor: ConstructorName,
    },
    #[error("internal: unknown field `{field}` in constructor `{constructor}`")]
    UnresolvedConstructionField {
        field: FieldName,
        constructor: ConstructorName,
    },
    #[error("constructor `{constructor}` requires field arguments")]
    ConstructorRequiresFields { constructor: ConstructorName },
    #[error("type `{type_name}` expects {expected} generic argument(s), got {got}")]
    GenericArgCount {
        type_name: StructTypeName,
        expected: GenericArgArity,
        got: usize,
    },
    #[error("internal: generic parameter `{param}` has no default")]
    MissingGenericDefault { param: GenericParamName },
    #[error("type `{type_name}` expects at most {maximum} generic arguments, got {got}")]
    TooManyGenericArgs {
        type_name: StructTypeName,
        maximum: usize,
        got: usize,
    },
    #[error("concrete type `{type_name}` requires exactly {expected} generic arguments, got {got}")]
    ConcreteGenericArgCount {
        type_name: StructTypeName,
        expected: usize,
        got: usize,
    },
    #[error("generic argument `{argument}` for `{parameter}` is not concrete")]
    NonConcreteGenericArgument {
        parameter: GenericParamName,
        argument: Box<SymbolicGenericArgument>,
    },
    #[error(
        "recursive generic type `{type_name}` changes its arguments; concrete field obligations cannot be discharged finitely"
    )]
    RecursiveGenericTypeArguments { type_name: StructTypeName },
    #[error(
        "default for generic parameter `{param}` may reference only earlier generic parameters; `{referenced}` is not earlier"
    )]
    GenericDefaultForwardReference {
        param: GenericParamName,
        referenced: GenericParamName,
    },
    #[error(
        "generic parameter `{param}` without a default cannot follow defaulted parameter `{first_defaulted}`"
    )]
    RequiredGenericAfterDefault {
        param: GenericParamName,
        first_defaulted: GenericParamName,
    },
    #[error("{generic}")]
    UnboundGenericInConcreteType { generic: Box<UnboundGeneric> },
}

impl DiagnosticKind for StructError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownStructType { .. } => "graphcal::S002",
            Self::UnknownField { .. } => "graphcal::S003",
            Self::MissingFields { .. } => "graphcal::S004",
            Self::MissingPatternFields { .. } => "graphcal::S009",
            Self::EmptyParenthesizedConstructor { .. } => "graphcal::S010",
            Self::ExtraFields { .. } => "graphcal::S005",
            Self::FieldDimensionMismatch { .. } => "graphcal::S006",
            Self::NotAStruct { .. } => "graphcal::S007",
            Self::UnknownLocalRef { .. } => "graphcal::S008",
            Self::CannotMatchFiniteKey { .. } => "graphcal::S011",
            Self::CannotMatchCoordinateIndex { .. } => "graphcal::S012",
            Self::LabelArmNotLabelPattern => "graphcal::S013",
            Self::DuplicateLabelArm { .. } => "graphcal::S014",
            Self::NonExhaustiveLabelMatch { .. } => "graphcal::S015",
            Self::UnionArmNotConstructorPattern => "graphcal::S016",
            Self::DuplicateConstructorArm { .. } => "graphcal::S017",
            Self::DuplicatePatternBinding { .. } => "graphcal::S018",
            Self::NonExhaustiveUnionMatch { .. } => "graphcal::S019",
            Self::UnmatchableType { .. } => "graphcal::S020",
            Self::EmptyMatch => "graphcal::S021",
            Self::DuplicateConstructionField { .. } => "graphcal::S022",
            Self::UnresolvedConstructionField { .. } => "graphcal::S023",
            Self::ConstructorRequiresFields { .. } => "graphcal::S024",
            Self::GenericArgCount { .. } => "graphcal::S025",
            Self::MissingGenericDefault { .. } => "graphcal::S026",
            Self::TooManyGenericArgs { .. } => "graphcal::S027",
            Self::ConcreteGenericArgCount { .. } => "graphcal::S028",
            Self::NonConcreteGenericArgument { .. } => "graphcal::S029",
            Self::RecursiveGenericTypeArguments { .. } => "graphcal::S030",
            Self::GenericDefaultForwardReference { .. } => "graphcal::S031",
            Self::RequiredGenericAfterDefault { .. } => "graphcal::S032",
            Self::UnboundGenericInConcreteType { .. } => "graphcal::S033",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::UnknownStructType { .. } | Self::UnknownLocalRef { .. } => {
                Some("not found".to_owned())
            }
            Self::UnknownField { .. } => Some("no such field".to_owned()),
            Self::MissingFields { .. } => Some("incomplete construction".to_owned()),
            Self::MissingPatternFields { .. } => Some("incomplete pattern".to_owned()),
            Self::EmptyParenthesizedConstructor { .. } => {
                Some("empty parentheses are invalid here".to_owned())
            }
            Self::ExtraFields { .. } => Some("unexpected fields".to_owned()),
            Self::FieldDimensionMismatch { found, .. } => Some(format!("has dimension {found}")),
            Self::NotAStruct { .. } => Some("not a struct".to_owned()),
            Self::CannotMatchFiniteKey { .. }
            | Self::CannotMatchCoordinateIndex { .. }
            | Self::LabelArmNotLabelPattern
            | Self::DuplicateLabelArm { .. }
            | Self::NonExhaustiveLabelMatch { .. }
            | Self::UnionArmNotConstructorPattern
            | Self::DuplicateConstructorArm { .. }
            | Self::DuplicatePatternBinding { .. }
            | Self::NonExhaustiveUnionMatch { .. }
            | Self::UnmatchableType { .. }
            | Self::EmptyMatch
            | Self::DuplicateConstructionField { .. }
            | Self::UnresolvedConstructionField { .. }
            | Self::ConstructorRequiresFields { .. }
            | Self::GenericArgCount { .. }
            | Self::MissingGenericDefault { .. }
            | Self::TooManyGenericArgs { .. }
            | Self::ConcreteGenericArgCount { .. }
            | Self::NonConcreteGenericArgument { .. }
            | Self::RecursiveGenericTypeArguments { .. }
            | Self::GenericDefaultForwardReference { .. }
            | Self::RequiredGenericAfterDefault { .. }
            | Self::UnboundGenericInConcreteType { .. } => Some("error here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnknownStructType { .. } => Some("struct types must be declared with `type` before use".to_owned()),
            Self::UnknownField { .. }
            | Self::FieldDimensionMismatch { .. }
            | Self::CannotMatchFiniteKey { .. }
            | Self::CannotMatchCoordinateIndex { .. }
            | Self::LabelArmNotLabelPattern
            | Self::DuplicateLabelArm { .. }
            | Self::NonExhaustiveLabelMatch { .. }
            | Self::UnionArmNotConstructorPattern
            | Self::DuplicateConstructorArm { .. }
            | Self::DuplicatePatternBinding { .. }
            | Self::NonExhaustiveUnionMatch { .. }
            | Self::UnmatchableType { .. }
            | Self::EmptyMatch
            | Self::DuplicateConstructionField { .. }
            | Self::UnresolvedConstructionField { .. }
            | Self::ConstructorRequiresFields { .. }
            | Self::GenericArgCount { .. }
            | Self::MissingGenericDefault { .. }
            | Self::TooManyGenericArgs { .. }
            | Self::ConcreteGenericArgCount { .. }
            | Self::NonConcreteGenericArgument { .. }
            | Self::RecursiveGenericTypeArguments { .. }
            | Self::GenericDefaultForwardReference { .. }
            | Self::RequiredGenericAfterDefault { .. }
            | Self::UnboundGenericInConcreteType { .. } => None,
            Self::MissingFields { .. } => Some("all fields are required when constructing a struct".to_owned()),
            Self::MissingPatternFields { .. } => Some("all constructor fields must be bound as `field: variable` or discarded with `field: _`".to_owned()),
            Self::EmptyParenthesizedConstructor { constructor, .. } => Some(format!("write a unit constructor as `{constructor}`; payload constructors require named field arguments")),
            Self::ExtraFields { .. } => Some("only fields declared in the struct type are allowed".to_owned()),
            Self::NotAStruct { .. } => Some("field access `.field` is only valid on struct values".to_owned()),
            Self::UnknownLocalRef { .. } => Some("local variables are introduced by `for`, `scan`, `unfold`, `match`, or function parameters".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::UnknownStructType { .. }
            | Self::UnknownField { .. }
            | Self::MissingFields { .. }
            | Self::MissingPatternFields { .. }
            | Self::EmptyParenthesizedConstructor { .. }
            | Self::ExtraFields { .. }
            | Self::FieldDimensionMismatch { .. }
            | Self::NotAStruct { .. }
            | Self::UnknownLocalRef { .. }
            | Self::CannotMatchFiniteKey { .. }
            | Self::CannotMatchCoordinateIndex { .. }
            | Self::LabelArmNotLabelPattern
            | Self::DuplicateLabelArm { .. }
            | Self::NonExhaustiveLabelMatch { .. }
            | Self::UnionArmNotConstructorPattern
            | Self::DuplicateConstructorArm { .. }
            | Self::DuplicatePatternBinding { .. }
            | Self::NonExhaustiveUnionMatch { .. }
            | Self::UnmatchableType { .. }
            | Self::EmptyMatch
            | Self::DuplicateConstructionField { .. }
            | Self::UnresolvedConstructionField { .. }
            | Self::ConstructorRequiresFields { .. }
            | Self::GenericArgCount { .. }
            | Self::MissingGenericDefault { .. }
            | Self::TooManyGenericArgs { .. }
            | Self::ConcreteGenericArgCount { .. }
            | Self::NonConcreteGenericArgument { .. }
            | Self::RecursiveGenericTypeArguments { .. }
            | Self::GenericDefaultForwardReference { .. }
            | Self::RequiredGenericAfterDefault { .. }
            | Self::UnboundGenericInConcreteType { .. } => Vec::new(),
        }
    }
}
