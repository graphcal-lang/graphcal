//! Diagnostics of declared value domains (min and max constraints).
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::generic_param::GenericParamId;
use crate::hir::expr::{LocalUnit, ResolvedUnitExpr};
use crate::semantic::checked_type::TypeSpelling;
use crate::semantic::dimension_table::DimensionSpelling;
use crate::syntax::ast::DomainBoundKind;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};
use crate::tir::typed::declared_type_spelling::DeclaredTypeSpelling;

/// One step from a constant into a nested value: a struct field or an
/// indexed entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValuePathStep {
    Field(FieldName),
    Entry(IndexEntryKey),
}

/// A constrained field of a nominal type: `Type.field` when the constructor
/// is record-shaped and named like its type (`constructor` is `None`),
/// `Type.Constructor.field` otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalFieldPath {
    pub type_name: StructTypeName,
    pub constructor: Option<ConstructorName>,
    pub field: FieldName,
}

impl std::fmt::Display for NominalFieldPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.constructor {
            None => write!(f, "{}.{}", self.type_name, self.field),
            Some(constructor) => write!(f, "{}.{constructor}.{}", self.type_name, self.field),
        }
    }
}

/// The constrained thing a domain diagnostic names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainSubject {
    /// A constrained declaration.
    Declaration(DeclName),
    /// A constrained field of a nominal type.
    NominalField(Box<NominalFieldPath>),
    /// A constrained field named by its constructor: `Constructor.field`.
    ConstructorField(Box<(ConstructorName, FieldName)>),
    /// A value inside a constant.
    Value(Box<ValuePath>),
}

/// A constant's value or a value nested inside it: `root.step.step`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuePath {
    root: DeclName,
    steps: Vec<ValuePathStep>,
}

impl ValuePath {
    /// The whole value of the constant `root`.
    #[must_use]
    pub const fn new(root: DeclName) -> Self {
        Self {
            root,
            steps: Vec::new(),
        }
    }

    /// The value one step further into this one.
    #[must_use]
    pub fn child(&self, step: ValuePathStep) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self {
            root: self.root.clone(),
            steps,
        }
    }
}

impl std::fmt::Display for ValuePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.root.fmt(f)?;
        for step in &self.steps {
            match step {
                ValuePathStep::Field(field) => write!(f, ".{field}")?,
                ValuePathStep::Entry(key) => write!(f, ".{key}")?,
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for DomainSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(name) => name.fmt(f),
            Self::NominalField(path) => path.fmt(f),
            Self::ConstructorField(path) => write!(f, "{}.{}", path.0, path.1),
            Self::Value(path) => path.fmt(f),
        }
    }
}

/// How a domain diagnostic spells a constrained type or a bound's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainTypeSpelling {
    Dimension(DimensionSpelling),
    Checked(TypeSpelling),
    Declared(DeclaredTypeSpelling),
}

impl std::fmt::Display for DomainTypeSpelling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dimension(dimension) => dimension.fmt(f),
            Self::Checked(ty) => ty.fmt(f),
            Self::Declared(ty) => ty.fmt(f),
        }
    }
}

/// A type that cannot carry a `min`/`max` domain constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnconstrainableType {
    Bool,
    Complex,
    Key,
    Struct(StructTypeName),
    GenericTypeParam(GenericParamId),
    /// A concrete application found unconstrainable after specialization.
    Checked(TypeSpelling),
}

impl std::fmt::Display for UnconstrainableType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool => f.write_str("Bool"),
            Self::Complex => f.write_str("Complex"),
            Self::Key => f.write_str("Key"),
            Self::Struct(name) => write!(f, "struct `{name}`"),
            Self::GenericTypeParam(param) => write!(f, "generic Type parameter `{param}`"),
            Self::Checked(ty) => ty.fmt(f),
        }
    }
}

/// How a domain diagnostic spells an evaluated `min`/`max` bound: as the
/// bound was written when it is a literal, its evaluated value otherwise.
#[derive(Debug, Clone, PartialEq)]
pub enum DomainBoundSpelling {
    /// A number: a written number, or the SI value of a computed quantity
    /// bound.
    Number(f64),
    /// An integer bound.
    Integer(i64),
    /// A written quantity literal: its number and unit.
    Quantity {
        value: f64,
        unit: ResolvedUnitExpr<LocalUnit>,
    },
    /// A written negation of a bound.
    Negated(Box<Self>),
    /// A datetime bound: its epoch.
    Datetime(hifitime::Epoch),
}

impl std::fmt::Display for DomainBoundSpelling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(value) => f.write_str(&crate::display::number::format_number(*value)),
            Self::Integer(value) => value.fmt(f),
            Self::Quantity { value, unit } => {
                let unit = crate::display::unit_label::format_unit_terms_with_config(
                    unit.terms
                        .iter()
                        .map(|item| (item.op, item.name.value.to_string(), item.power)),
                    true,
                );
                write!(
                    f,
                    "{} {unit}",
                    crate::display::number::format_number(*value)
                )
            }
            Self::Negated(bound) => write!(f, "-{bound}"),
            Self::Datetime(epoch) => epoch.fmt(f),
        }
    }
}

/// Which declared bound a value is outside of.
#[derive(Debug, Clone, PartialEq)]
pub enum DomainBoundViolation {
    /// The value is below the `min` bound.
    BelowMinimum(DomainBoundSpelling),
    /// The value is above the `max` bound.
    AboveMaximum(DomainBoundSpelling),
}

impl std::fmt::Display for DomainBoundViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BelowMinimum(bound) => write!(f, "below minimum ({bound})"),
            Self::AboveMaximum(bound) => write!(f, "above maximum ({bound})"),
        }
    }
}

/// A value outside its declared domain: the bound it violates, at the entry
/// of an indexed value it is found in (outermost axis first).
#[derive(Debug, Clone, PartialEq)]
pub struct DomainViolationDetail {
    entries: Vec<IndexEntryKey>,
    bound: DomainBoundViolation,
}

impl DomainViolationDetail {
    /// A value that violates `bound` itself.
    #[must_use]
    pub const fn new(bound: DomainBoundViolation) -> Self {
        Self {
            entries: Vec::new(),
            bound,
        }
    }

    /// This violation, found at `entry` of an indexed value.
    #[must_use]
    pub fn at_entry(mut self, entry: IndexEntryKey) -> Self {
        self.entries.insert(0, entry);
        self
    }

    /// The bound the value violates.
    #[must_use]
    pub const fn bound(&self) -> &DomainBoundViolation {
        &self.bound
    }

    /// The entries, outermost first, the violating value is found at.
    #[must_use]
    pub fn entries(&self) -> &[IndexEntryKey] {
        &self.entries
    }
}

impl std::fmt::Display for DomainViolationDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for entry in &self.entries {
            write!(f, "at {entry}: ")?;
        }
        self.bound.fmt(f)
    }
}

impl std::error::Error for DomainViolationDetail {}

/// Diagnostics of declared value domains (min and max constraints).
#[derive(Debug, Clone, Error)]
pub enum DomainError {
    #[error("domain violation: `{name}` value {value} is {violation}")]
    DomainViolation {
        name: DomainSubject,
        value: String,
        violation: Box<DomainViolationDetail>,
    },
    #[error(
        "domain bound dimension mismatch on `{name}`: type has dimension {type_dim}, but {bound_name} bound has dimension {bound_dim}"
    )]
    DomainDimensionMismatch {
        name: DomainSubject,
        type_dim: DomainTypeSpelling,
        bound_name: DomainBoundKind,
        bound_dim: DomainTypeSpelling,
    },
    #[error("domain constraint on `{name}`: min ({min}) exceeds max ({max})")]
    DomainMinExceedsMax {
        name: DomainSubject,
        min: Box<DomainBoundSpelling>,
        max: Box<DomainBoundSpelling>,
    },
    #[error("domain constraints are not valid on `{type_kind}` types")]
    InvalidDomainTarget { type_kind: UnconstrainableType },
    #[error("domain bound type mismatch on Int `{name}`: {bound_name} bound has type {bound_type}")]
    IntDomainBoundTypeMismatch {
        name: DomainSubject,
        bound_name: DomainBoundKind,
        bound_type: TypeSpelling,
    },
    #[error("domain constraints are not supported on generic type arguments")]
    GenericTypeArgDomainConstraint,
    #[error(
        "datetime domain bound type mismatch on `{name}`: target is {target_type}, but {bound_name} bound is {bound_type}"
    )]
    DatetimeDomainBoundTypeMismatch {
        name: DomainSubject,
        target_type: TypeSpelling,
        bound_name: DomainBoundKind,
        bound_type: TypeSpelling,
    },
    /// An Int bound on a quantity domain that `f64` cannot represent exactly.
    #[error("domain bound integer {value} is too large for exact quantity comparison")]
    InexactIntDomainBound { value: i64 },
}

impl DiagnosticKind for DomainError {
    fn code(&self) -> &'static str {
        match self {
            Self::DomainViolation { .. } => "graphcal::C001",
            Self::DomainDimensionMismatch { .. } => "graphcal::C002",
            Self::DomainMinExceedsMax { .. } => "graphcal::C003",
            Self::InvalidDomainTarget { .. } => "graphcal::C004",
            Self::IntDomainBoundTypeMismatch { .. } => "graphcal::C005",
            Self::GenericTypeArgDomainConstraint => "graphcal::C006",
            Self::DatetimeDomainBoundTypeMismatch { .. } => "graphcal::C007",
            Self::InexactIntDomainBound { .. } => "graphcal::C008",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::DomainViolation { .. } => Some("value out of declared domain".to_owned()),
            Self::DomainDimensionMismatch { .. } => {
                Some("dimension mismatch in domain bound".to_owned())
            }
            Self::DomainMinExceedsMax { .. } => Some("min > max".to_owned()),
            Self::InvalidDomainTarget { .. } => Some("constraints not valid here".to_owned()),
            Self::IntDomainBoundTypeMismatch { .. } => {
                Some("Int bound must have type Int".to_owned())
            }
            Self::GenericTypeArgDomainConstraint => Some("constraint not allowed here".to_owned()),
            Self::DatetimeDomainBoundTypeMismatch { .. } => {
                Some("datetime bound has the wrong time scale or value type".to_owned())
            }
            Self::InexactIntDomainBound { .. } => Some("error here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::DomainViolation { .. } => Some("the value must satisfy the domain constraints declared on the type".to_owned()),
            Self::DomainDimensionMismatch { .. } => Some("domain bounds must have the same dimension as the constrained type".to_owned()),
            Self::DomainMinExceedsMax { .. } => Some("the min bound must be less than or equal to the max bound".to_owned()),
            Self::InvalidDomainTarget { .. } => Some("domain constraints (min/max) are only valid on quantity, Int, and Datetime types".to_owned()),
            Self::IntDomainBoundTypeMismatch { .. } => Some("Int domain bounds must be Int so their full range is preserved exactly".to_owned()),
            Self::GenericTypeArgDomainConstraint => Some("put the constraint on the field in the struct definition, not on the generic type argument".to_owned()),
            Self::DatetimeDomainBoundTypeMismatch { .. } => Some("datetime bounds must have exactly the constrained Datetime<S> type; use an explicit time-scale conversion".to_owned()),
            Self::InexactIntDomainBound { .. } => None,
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DomainViolation { .. }
            | Self::DomainDimensionMismatch { .. }
            | Self::DomainMinExceedsMax { .. }
            | Self::InvalidDomainTarget { .. }
            | Self::IntDomainBoundTypeMismatch { .. }
            | Self::GenericTypeArgDomainConstraint
            | Self::DatetimeDomainBoundTypeMismatch { .. }
            | Self::InexactIntDomainBound { .. } => Vec::new(),
        }
    }
}
