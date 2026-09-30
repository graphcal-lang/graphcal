//! Built-in language domain model.
//!
//! This module owns the closed source vocabulary for built-in constants and
//! functions, and the static call shape of every built-in function. String
//! spellings enter the compiler only through [`BuiltinConst::parse`] and
//! [`BuiltinFn::parse`]; downstream phases dispatch on the typed families of
//! [`BuiltinFn`] instead of matching raw names.

use crate::semantic::time_scale::TimeScale;

/// Define a closed set of built-in names: the enum, the boundary parse, the
/// canonical `as_str` rendering, and an `ALL` listing — all generated from a
/// single table so the spellings can never drift apart.
macro_rules! define_builtin_names {
    (
        parse = $parse_vis:vis fn $parse:ident;
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name { $($(#[$variant_meta])* $variant),+ }

        impl $name {
            /// Every variant, in declaration order.
            $vis const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Every variant, in declaration order.
            $vis fn all() -> impl Iterator<Item = Self> {
                Self::ALL.iter().copied()
            }

            /// Parse a source spelling into the typed variant.
            #[must_use]
            $parse_vis fn $parse(name: &str) -> Option<Self> {
                match name {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Canonical source spelling.
            #[must_use]
            $vis const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Define a family of built-in functions as a closed sum of sub-families.
///
/// Parsing, spelling, and enumeration delegate to the sub-families, so each
/// spelling is written exactly once, in its leaf table.
macro_rules! define_builtin_family {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident($inner:ty)),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name { $($(#[$variant_meta])* $variant($inner)),+ }

        impl $name {
            /// Every member, grouped by sub-family in declaration order.
            $vis fn all() -> impl Iterator<Item = Self> {
                std::iter::empty()$(.chain(<$inner>::all().map(Self::$variant)))+
            }

            fn from_spelling(name: &str) -> Option<Self> {
                None$(.or_else(|| <$inner>::from_spelling(name).map(Self::$variant)))+
            }

            /// Canonical source spelling.
            #[must_use]
            $vis const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant(inner) => inner.as_str()),+
                }
            }
        }

        $(
            impl From<$inner> for $name {
                fn from(inner: $inner) -> Self {
                    Self::$variant(inner)
                }
            }
        )+

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

define_builtin_names! {
    parse = pub fn parse;
    /// Built-in constants with closed semantic meaning.
    pub enum BuiltinConst {
        Pi => "PI",
        E => "E",
        Tau => "TAU",
        Sqrt2 => "SQRT2",
        Ln2 => "LN2",
        Ln10 => "LN10",
    }
}

impl BuiltinConst {
    /// Exact numeric value of this built-in constant.
    #[must_use]
    pub const fn value(self) -> f64 {
        match self {
            Self::Pi => std::f64::consts::PI,
            Self::E => std::f64::consts::E,
            Self::Tau => std::f64::consts::TAU,
            Self::Sqrt2 => std::f64::consts::SQRT_2,
            Self::Ln2 => std::f64::consts::LN_2,
            Self::Ln10 => std::f64::consts::LN_10,
        }
    }
}

define_builtin_family! {
    /// Every built-in function, grouped by the family that owns its typing and
    /// evaluation rule.
    pub enum BuiltinFn {
        /// Real quantity functions backed by a scalar kernel and a dimension
        /// signature.
        Scalar(ScalarFn),
        /// Complex construction, inspection, and real/complex overloads.
        Complex(ComplexFn),
        /// Reductions over rank-one indexed collections.
        Aggregation(AggregationFn),
        /// Shape-aware operations over rank-one and rank-two indexed quantities.
        LinearAlgebra(LinearAlgebraFn),
        /// Datetime construction, inspection, and time-scale conversion.
        Datetime(DatetimeFn),
        /// Type-category conversions between `Int`, keys, and quantities.
        Conversion(ConversionFn),
    }
}

impl BuiltinFn {
    /// `epoch<S>`, the only built-in applied with a static generic argument.
    pub const EPOCH: Self = Self::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Epoch));

    /// Parse a source function name — the only place built-in function
    /// spellings cross into the typed core.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::from_spelling(name)
    }

    /// Static call shape of this built-in.
    #[must_use]
    pub const fn entry(self) -> BuiltinEntry {
        match self {
            Self::Scalar(function) => BuiltinEntry::Kernel(function),
            Self::Complex(function) => BuiltinEntry::Signature(function.signature()),
            Self::Aggregation(function) => BuiltinEntry::Signature(function.signature()),
            Self::LinearAlgebra(function) => BuiltinEntry::Signature(function.signature()),
            Self::Datetime(function) => BuiltinEntry::Bespoke(function.arity()),
            Self::Conversion(_) => BuiltinEntry::Bespoke(BuiltinArity::Exact(1)),
        }
    }

    /// Classify how this built-in is applied at a call site.
    #[must_use]
    pub const fn application(self) -> BuiltinApplication {
        match self {
            Self::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Epoch)) => {
                BuiltinApplication::Epoch
            }
            function => BuiltinApplication::ScaleFree(ScaleFreeBuiltin(function)),
        }
    }
}

/// Static call shape of one built-in function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinEntry {
    /// A scalar kernel; its dimension signature lives in the scalar catalog
    /// (`semantic::scalar_function`), which is checked against [`ScalarFn::arity`].
    Kernel(ScalarFn),
    /// A custom type rule documented by a static source-like signature.
    Signature(&'static DisplaySignature),
    /// A custom type rule with no user-facing signature (datetime and type
    /// conversions).
    Bespoke(BuiltinArity),
}

impl BuiltinEntry {
    /// Number of runtime arguments the call accepts.
    #[must_use]
    pub const fn arity(self) -> BuiltinArity {
        match self {
            Self::Kernel(function) => BuiltinArity::Exact(function.arity()),
            Self::Signature(signature) => BuiltinArity::Exact(signature.arity()),
            Self::Bespoke(arity) => arity,
        }
    }

    /// Fixed arity of a built-in with a documented signature, for display.
    #[must_use]
    pub const fn documented_arity(self) -> Option<usize> {
        match self {
            Self::Kernel(function) => Some(function.arity()),
            Self::Signature(signature) => Some(signature.arity()),
            Self::Bespoke(_) => None,
        }
    }
}

/// Number of runtime arguments a built-in accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinArity {
    /// Exactly this many arguments.
    Exact(usize),
    /// `required` arguments, optionally followed by one more.
    OptionalTrailing { required: usize },
}

impl BuiltinArity {
    /// Whether a call with `got` arguments has an accepted shape.
    #[must_use]
    pub const fn accepts(self, got: usize) -> bool {
        match self {
            Self::Exact(expected) => got == expected,
            Self::OptionalTrailing { required } => got == required || got == required + 1,
        }
    }
}

impl std::fmt::Display for BuiltinArity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(expected) => write!(f, "{expected}"),
            Self::OptionalTrailing { required } => write!(f, "{required} or {}", required + 1),
        }
    }
}

/// Source-like signature of a built-in whose typing rule is custom, used at
/// LSP and display boundaries. Its parameter list is the arity source for the
/// function's family.
#[derive(Debug, PartialEq, Eq)]
pub struct DisplaySignature {
    /// Generic parameters, in declaration order.
    pub generics: &'static [SignatureGeneric],
    /// Runtime parameters, in call order.
    pub params: &'static [SignatureParam],
    /// Result type.
    pub result: SignatureType,
    /// Optional `where` clause.
    pub constraint: Option<SignatureConstraint>,
}

/// One generic parameter of a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureGeneric {
    /// Parameter name.
    pub name: &'static str,
    /// Parameter sort.
    pub kind: GenericParamKind,
}

/// Sort of a generic parameter in a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericParamKind {
    Dim,
    Index,
    Type,
}

impl GenericParamKind {
    /// Source spelling of the sort.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dim => "Dim",
            Self::Index => "Index",
            Self::Type => "Type",
        }
    }
}

/// One runtime parameter of a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureParam {
    /// Parameter name.
    pub name: &'static str,
    /// Parameter type.
    pub ty: SignatureType,
}

/// A prelude dimension named by a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureNamedDim {
    Angle,
    Dimensionless,
}

impl SignatureNamedDim {
    /// Source spelling of the dimension.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Angle => "Angle",
            Self::Dimensionless => "Dimensionless",
        }
    }
}

/// A dimension of a [`DisplaySignature`], spelled over its generic
/// parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureDim {
    /// A `Dim` generic parameter, e.g. `D`.
    Param(SignatureGeneric),
    /// A prelude dimension, e.g. `Angle`.
    Named(SignatureNamedDim),
    /// The product of two `Dim` parameters, `D1 * D2`.
    Product(SignatureGeneric, SignatureGeneric),
    /// The quotient of two `Dim` parameters, `D2 / D1`.
    Quotient(SignatureGeneric, SignatureGeneric),
    /// The reciprocal of a `Dim` parameter, `D^-1`.
    Reciprocal(SignatureGeneric),
    /// A `Dim` parameter raised to an `Index` parameter's cardinality, `D^|I|`.
    CardinalityPower(SignatureGeneric, SignatureGeneric),
}

impl SignatureDim {
    /// Whether the dimension is a binary product or quotient, which needs
    /// parentheses before an index suffix.
    const fn is_binary(self) -> bool {
        matches!(self, Self::Product(..) | Self::Quotient(..))
    }

    /// Generic parameters this dimension mentions, with the sort each
    /// position requires.
    #[cfg(test)]
    fn generics(self) -> impl Iterator<Item = (SignatureGeneric, GenericParamKind)> {
        let (first, second) = match self {
            Self::Param(dim) | Self::Reciprocal(dim) => (Some((dim, GenericParamKind::Dim)), None),
            Self::Named(_) => (None, None),
            Self::Product(lhs, rhs) | Self::Quotient(lhs, rhs) => (
                Some((lhs, GenericParamKind::Dim)),
                Some((rhs, GenericParamKind::Dim)),
            ),
            Self::CardinalityPower(dim, index) => (
                Some((dim, GenericParamKind::Dim)),
                Some((index, GenericParamKind::Index)),
            ),
        };
        first.into_iter().chain(second)
    }
}

impl std::fmt::Display for SignatureDim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Param(dim) => f.write_str(dim.name),
            Self::Named(named) => f.write_str(named.as_str()),
            Self::Product(lhs, rhs) => write!(f, "{} * {}", lhs.name, rhs.name),
            Self::Quotient(lhs, rhs) => write!(f, "{} / {}", lhs.name, rhs.name),
            Self::Reciprocal(dim) => write!(f, "{}^-1", dim.name),
            Self::CardinalityPower(dim, index) => write!(f, "{}^|{}|", dim.name, index.name),
        }
    }
}

/// A scalar value type of a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureValue {
    /// A quantity of the given dimension.
    Quantity(SignatureDim),
    /// A complex quantity, `Complex<D>`.
    Complex(SignatureDim),
    /// `Int`.
    Int,
    /// A key of an `Index` parameter, `Key<I>`.
    Key(SignatureGeneric),
    /// A `Type` generic parameter, e.g. `T`.
    TypeParam(SignatureGeneric),
}

impl SignatureValue {
    /// Generic parameters this value type mentions, with the sort each
    /// position requires.
    #[cfg(test)]
    fn generics(self) -> impl Iterator<Item = (SignatureGeneric, GenericParamKind)> {
        let (dimension, generic) = match self {
            Self::Quantity(dimension) | Self::Complex(dimension) => (Some(dimension), None),
            Self::Int => (None, None),
            Self::Key(index) => (None, Some((index, GenericParamKind::Index))),
            Self::TypeParam(ty) => (None, Some((ty, GenericParamKind::Type))),
        };
        dimension
            .into_iter()
            .flat_map(SignatureDim::generics)
            .chain(generic)
    }
}

impl std::fmt::Display for SignatureValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Quantity(dimension) => dimension.fmt(f),
            Self::Complex(dimension) => write!(f, "Complex<{dimension}>"),
            Self::Int => f.write_str("Int"),
            Self::Key(index) => write!(f, "Key<{}>", index.name),
            Self::TypeParam(ty) => f.write_str(ty.name),
        }
    }
}

/// A parameter or result type of a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureType {
    /// A scalar value type.
    Value(SignatureValue),
    /// A value type indexed by `Index` parameters, outermost first, e.g. `D[I, J]`.
    Indexed(SignatureValue, &'static [SignatureGeneric]),
    /// Either of two value types, e.g. `D | Complex<D>`.
    Either(SignatureValue, SignatureValue),
}

impl SignatureType {
    /// Generic parameters this type mentions, with the sort each position
    /// requires.
    #[cfg(test)]
    fn generics(self) -> impl Iterator<Item = (SignatureGeneric, GenericParamKind)> {
        let (value, axes, alternative): (_, &'static [SignatureGeneric], _) = match self {
            Self::Value(value) => (value, &[], None),
            Self::Indexed(value, axes) => (value, axes, None),
            Self::Either(value, alternative) => (value, &[], Some(alternative)),
        };
        value
            .generics()
            .chain(alternative.into_iter().flat_map(SignatureValue::generics))
            .chain(axes.iter().map(|axis| (*axis, GenericParamKind::Index)))
    }
}

impl std::fmt::Display for SignatureType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(value) => value.fmt(f),
            Self::Indexed(value, axes) => {
                match value {
                    SignatureValue::Quantity(dimension) if dimension.is_binary() => {
                        write!(f, "({dimension})")?;
                    }
                    _ => value.fmt(f)?,
                }
                let axes = axes.iter().map(|axis| axis.name).collect::<Vec<_>>();
                write!(f, "[{}]", axes.join(", "))
            }
            Self::Either(value, alternative) => write!(f, "{value} | {alternative}"),
        }
    }
}

/// A `where` clause of a [`DisplaySignature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureConstraint {
    /// An `Index` parameter has a fixed cardinality, `|I| = 3`.
    Cardinality {
        index: SignatureGeneric,
        size: usize,
    },
}

impl std::fmt::Display for SignatureConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cardinality { index, size } => write!(f, "|{}| = {size}", index.name),
        }
    }
}

impl DisplaySignature {
    /// Number of runtime parameters.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.params.len()
    }

    const fn where_clause(self, constraint: SignatureConstraint) -> Self {
        Self {
            constraint: Some(constraint),
            ..self
        }
    }

    /// Source-like `name: type` label of each runtime parameter.
    pub fn parameter_labels(&self) -> impl Iterator<Item = String> + '_ {
        self.params
            .iter()
            .map(|param| format!("{}: {}", param.name, param.ty))
    }

    /// Source-like signature label for the function spelled `name`.
    #[must_use]
    pub fn label(&self, name: &str) -> String {
        let generics = if self.generics.is_empty() {
            String::new()
        } else {
            let generics = self
                .generics
                .iter()
                .map(|generic| format!("{}: {}", generic.name, generic.kind.as_str()))
                .collect::<Vec<_>>();
            format!("<{}>", generics.join(", "))
        };
        let params = self.parameter_labels().collect::<Vec<_>>().join(", ");
        let constraint = self
            .constraint
            .map_or_else(String::new, |constraint| format!(" where {constraint}"));
        format!(
            "fn {name}{generics}({params}) -> {}{constraint}",
            self.result
        )
    }

    /// Generic parameters mentioned by the parameter, result, and constraint
    /// types, with the sort each position requires.
    #[cfg(test)]
    fn referenced_generics(&self) -> impl Iterator<Item = (SignatureGeneric, GenericParamKind)> {
        let constraint = self.constraint.map(|constraint| match constraint {
            SignatureConstraint::Cardinality { index, .. } => (index, GenericParamKind::Index),
        });
        self.params
            .iter()
            .flat_map(|param| param.ty.generics())
            .chain(self.result.generics())
            .chain(constraint)
    }
}

const fn dim(name: &'static str) -> SignatureGeneric {
    SignatureGeneric {
        name,
        kind: GenericParamKind::Dim,
    }
}

const fn index(name: &'static str) -> SignatureGeneric {
    SignatureGeneric {
        name,
        kind: GenericParamKind::Index,
    }
}

/// A `'static` slice of [`SignatureParam`]s written as `name: type` pairs.
macro_rules! params {
    ($($name:ident: $ty:expr),* $(,)?) => {
        &const { [$(SignatureParam { name: stringify!($name), ty: $ty }),*] }
    };
}

const fn sig(
    generics: &'static [SignatureGeneric],
    params: &'static [SignatureParam],
    result: SignatureType,
) -> DisplaySignature {
    DisplaySignature {
        generics,
        params,
        result,
        constraint: None,
    }
}

const G_D: SignatureGeneric = dim("D");
const G_D1: SignatureGeneric = dim("D1");
const G_D2: SignatureGeneric = dim("D2");
const G_I: SignatureGeneric = index("I");
const G_J: SignatureGeneric = index("J");
const G_K: SignatureGeneric = index("K");
const G_T: SignatureGeneric = SignatureGeneric {
    name: "T",
    kind: GenericParamKind::Type,
};

/// A quantity of a `Dim` parameter, e.g. `D`.
const fn quantity(dim: SignatureGeneric) -> SignatureValue {
    SignatureValue::Quantity(SignatureDim::Param(dim))
}

/// A complex quantity of a `Dim` parameter, `Complex<D>`.
const fn complex(dim: SignatureGeneric) -> SignatureValue {
    SignatureValue::Complex(SignatureDim::Param(dim))
}

/// A scalar signature type.
const fn value(value: SignatureValue) -> SignatureType {
    SignatureType::Value(value)
}

/// A quantity of a `Dim` parameter indexed by `Index` parameters, e.g. `D[I, J]`.
const fn indexed(dim: SignatureGeneric, axes: &'static [SignatureGeneric]) -> SignatureType {
    SignatureType::Indexed(quantity(dim), axes)
}

const ANGLE: SignatureValue =
    SignatureValue::Quantity(SignatureDim::Named(SignatureNamedDim::Angle));
const DIMENSIONLESS: SignatureDim = SignatureDim::Named(SignatureNamedDim::Dimensionless);

/// A built-in function other than `epoch<S>`.
///
/// Only [`BuiltinFn::application`] constructs this, so a scale-free built-in
/// reference can never stand for `epoch` without its time scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScaleFreeBuiltin(BuiltinFn);

impl ScaleFreeBuiltin {
    /// Canonical built-in function identity.
    #[must_use]
    pub const fn function(self) -> BuiltinFn {
        self.0
    }
}

impl std::fmt::Display for ScaleFreeBuiltin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// How a built-in function is applied at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinApplication {
    /// A built-in callable without static generic arguments.
    ScaleFree(ScaleFreeBuiltin),
    /// `epoch<S>`, which requires a static time-scale argument.
    Epoch,
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Real quantity functions evaluated by a scalar `f64` kernel and checked
    /// against a dimension signature.
    pub enum ScalarFn {
        Sqrt => "sqrt",
        Cbrt => "cbrt",
        Expm1 => "expm1",
        Ln => "ln",
        Log10 => "log10",
        Log2 => "log2",
        Log => "log",
        Log1p => "log1p",
        Sin => "sin",
        Cos => "cos",
        Tan => "tan",
        Asin => "asin",
        Acos => "acos",
        Atan => "atan",
        Atan2 => "atan2",
        Sinh => "sinh",
        Cosh => "cosh",
        Tanh => "tanh",
        Asinh => "asinh",
        Acosh => "acosh",
        Atanh => "atanh",
        Floor => "floor",
        Ceil => "ceil",
        Round => "round",
        Trunc => "trunc",
        Sign => "sign",
        Least => "least",
        Greatest => "greatest",
        Hypot => "hypot",
        Clamp => "clamp",
    }
}

impl ScalarFn {
    /// Number of runtime arguments; the scalar catalog's kernels and
    /// signatures are checked against it.
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::Sqrt
            | Self::Cbrt
            | Self::Expm1
            | Self::Ln
            | Self::Log10
            | Self::Log2
            | Self::Log1p
            | Self::Sin
            | Self::Cos
            | Self::Tan
            | Self::Asin
            | Self::Acos
            | Self::Atan
            | Self::Sinh
            | Self::Cosh
            | Self::Tanh
            | Self::Asinh
            | Self::Acosh
            | Self::Atanh
            | Self::Floor
            | Self::Ceil
            | Self::Round
            | Self::Trunc
            | Self::Sign => 1,
            Self::Log | Self::Atan2 | Self::Least | Self::Greatest | Self::Hypot => 2,
            Self::Clamp => 3,
        }
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Built-ins that construct, inspect, or overload operations for complex
    /// quantities.
    pub enum ComplexFn {
        Rectangular => "complex",
        Polar => "polar",
        ToComplex => "to_complex",
        Real => "re",
        Imaginary => "im",
        Phase => "phase",
        Conjugate => "conj",
        Absolute => "abs",
        Exponential => "exp",
    }
}

impl ComplexFn {
    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> usize {
        self.signature().arity()
    }

    /// Source-like signature used at LSP/display boundaries.
    #[must_use]
    pub const fn signature(self) -> &'static DisplaySignature {
        match self {
            Self::Rectangular => {
                &const {
                    sig(
                        &[G_D],
                        params![re: value(quantity(G_D)), im: value(quantity(G_D))],
                        value(complex(G_D)),
                    )
                }
            }
            Self::Polar => {
                &const {
                    sig(
                        &[G_D],
                        params![magnitude: value(quantity(G_D)), phase: value(ANGLE)],
                        value(complex(G_D)),
                    )
                }
            }
            Self::ToComplex => {
                &const {
                    sig(
                        &[G_D],
                        params![x: value(quantity(G_D))],
                        value(complex(G_D)),
                    )
                }
            }
            Self::Real | Self::Imaginary => {
                &const {
                    sig(
                        &[G_D],
                        params![z: value(complex(G_D))],
                        value(quantity(G_D)),
                    )
                }
            }
            Self::Phase => &const { sig(&[G_D], params![z: value(complex(G_D))], value(ANGLE)) },
            Self::Conjugate => {
                &const { sig(&[G_D], params![z: value(complex(G_D))], value(complex(G_D))) }
            }
            Self::Absolute => {
                &const {
                    sig(
                        &[G_D],
                        params![x: SignatureType::Either(quantity(G_D), complex(G_D))],
                        value(quantity(G_D)),
                    )
                }
            }
            Self::Exponential => {
                const DIMENSIONLESS_OR_COMPLEX: SignatureType = SignatureType::Either(
                    SignatureValue::Quantity(DIMENSIONLESS),
                    SignatureValue::Complex(DIMENSIONLESS),
                );
                &const {
                    sig(
                        &[],
                        params![x: DIMENSIONLESS_OR_COMPLEX],
                        DIMENSIONLESS_OR_COMPLEX,
                    )
                }
            }
        }
    }
}

define_builtin_family! {
    /// Built-in reductions over rank-one indexed collections.
    pub enum AggregationFn {
        /// Reductions that produce a value from the collection's elements.
        Value(ValueAggregation),
        /// Reductions that produce a key of the reduced axis.
        Key(KeyAggregation),
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Built-in reductions that combine indexed elements into one value.
    pub enum ValueAggregation {
        Sum => "sum",
        Product => "product",
        Minimum => "minimum",
        Maximum => "maximum",
        Mean => "mean",
        RootSumSquare => "rss",
        Count => "count",
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Built-in reductions that select the key of an extremum element.
    pub enum KeyAggregation {
        Argmin => "argmin",
        Argmax => "argmax",
    }
}

impl AggregationFn {
    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> usize {
        self.signature().arity()
    }

    /// Source-like signature used at LSP/display boundaries.
    #[must_use]
    pub const fn signature(self) -> &'static DisplaySignature {
        match self {
            Self::Value(
                ValueAggregation::Sum
                | ValueAggregation::Minimum
                | ValueAggregation::Maximum
                | ValueAggregation::Mean
                | ValueAggregation::RootSumSquare,
            ) => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![values: indexed(G_D, &[G_I])],
                        value(quantity(G_D)),
                    )
                }
            }
            Self::Value(ValueAggregation::Product) => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![values: indexed(G_D, &[G_I])],
                        value(SignatureValue::Quantity(SignatureDim::CardinalityPower(
                            G_D, G_I,
                        ))),
                    )
                }
            }
            Self::Value(ValueAggregation::Count) => {
                &const {
                    sig(
                        &[G_T, G_I],
                        params![values: SignatureType::Indexed(SignatureValue::TypeParam(G_T), &[G_I])],
                        value(SignatureValue::Int),
                    )
                }
            }
            Self::Key(KeyAggregation::Argmin | KeyAggregation::Argmax) => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![values: indexed(G_D, &[G_I])],
                        value(SignatureValue::Key(G_I)),
                    )
                }
            }
        }
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Built-in linear-algebra operations over rank-one and rank-two indexed
    /// quantities.
    pub enum LinearAlgebraFn {
        Dot => "dot",
        Matmul => "matmul",
        Transpose => "transpose",
        Trace => "trace",
        Norm => "norm",
        Cross => "cross",
        Outer => "outer",
        Solve => "solve",
        Inverse => "inverse",
        Determinant => "det",
    }
}

impl LinearAlgebraFn {
    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> usize {
        self.signature().arity()
    }

    /// Source-like signature used at LSP/display boundaries.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per function of a static signature table"
    )]
    pub const fn signature(self) -> &'static DisplaySignature {
        match self {
            Self::Dot => {
                &const {
                    sig(
                        &[G_D1, G_D2, G_I],
                        params![a: indexed(G_D1, &[G_I]), b: indexed(G_D2, &[G_I])],
                        value(SignatureValue::Quantity(SignatureDim::Product(G_D1, G_D2))),
                    )
                }
            }
            Self::Matmul => {
                &const {
                    sig(
                        &[G_D1, G_D2, G_I, G_J, G_K],
                        params![a: indexed(G_D1, &[G_I, G_J]), b: indexed(G_D2, &[G_J, G_K])],
                        SignatureType::Indexed(
                            SignatureValue::Quantity(SignatureDim::Product(G_D1, G_D2)),
                            &[G_I, G_K],
                        ),
                    )
                }
            }
            Self::Transpose => {
                &const {
                    sig(
                        &[G_D, G_I, G_J],
                        params![a: indexed(G_D, &[G_I, G_J])],
                        indexed(G_D, &[G_J, G_I]),
                    )
                }
            }
            Self::Trace => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![a: indexed(G_D, &[G_I, G_I])],
                        value(quantity(G_D)),
                    )
                }
            }
            Self::Norm => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![v: indexed(G_D, &[G_I])],
                        value(quantity(G_D)),
                    )
                }
            }
            Self::Cross => {
                &const {
                    sig(
                        &[G_D1, G_D2, G_I],
                        params![a: indexed(G_D1, &[G_I]), b: indexed(G_D2, &[G_I])],
                        SignatureType::Indexed(
                            SignatureValue::Quantity(SignatureDim::Product(G_D1, G_D2)),
                            &[G_I],
                        ),
                    )
                    .where_clause(SignatureConstraint::Cardinality {
                        index: G_I,
                        size: 3,
                    })
                }
            }
            Self::Outer => {
                &const {
                    sig(
                        &[G_D1, G_D2, G_I, G_J],
                        params![a: indexed(G_D1, &[G_I]), b: indexed(G_D2, &[G_J])],
                        SignatureType::Indexed(
                            SignatureValue::Quantity(SignatureDim::Product(G_D1, G_D2)),
                            &[G_I, G_J],
                        ),
                    )
                }
            }
            Self::Solve => {
                &const {
                    sig(
                        &[G_D1, G_D2, G_I],
                        params![a: indexed(G_D1, &[G_I, G_I]), b: indexed(G_D2, &[G_I])],
                        SignatureType::Indexed(
                            SignatureValue::Quantity(SignatureDim::Quotient(G_D2, G_D1)),
                            &[G_I],
                        ),
                    )
                }
            }
            Self::Inverse => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![a: indexed(G_D, &[G_I, G_I])],
                        SignatureType::Indexed(
                            SignatureValue::Quantity(SignatureDim::Reciprocal(G_D)),
                            &[G_I, G_I],
                        ),
                    )
                }
            }
            Self::Determinant => {
                &const {
                    sig(
                        &[G_D, G_I],
                        params![a: indexed(G_D, &[G_I, G_I])],
                        value(SignatureValue::Quantity(SignatureDim::CardinalityPower(
                            G_D, G_I,
                        ))),
                    )
                }
            }
        }
    }
}

define_builtin_family! {
    /// Datetime construction, inspection, and time-scale conversion.
    pub enum DatetimeFn {
        /// Construct a datetime from a contextual literal.
        Constructor(DatetimeConstructorFn),
        /// Extract a Gregorian calendar field as `Int`.
        Field(DatetimeField),
        /// Construct a UTC datetime from a numeric epoch count.
        FromNumeric(DatetimeFromNumericFn),
        /// Convert a datetime into a dimensionless numeric epoch count.
        ToNumeric(DatetimeToNumericFn),
        /// Re-express a datetime in another time scale.
        ScaleConversion(TimeScaleConversionFn),
    }
}

impl DatetimeFn {
    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> BuiltinArity {
        match self {
            // `datetime(literal)` or `datetime(literal, timezone)`.
            Self::Constructor(DatetimeConstructorFn::Datetime) => {
                BuiltinArity::OptionalTrailing { required: 1 }
            }
            Self::Constructor(DatetimeConstructorFn::Epoch)
            | Self::Field(_)
            | Self::FromNumeric(_)
            | Self::ToNumeric(_)
            | Self::ScaleConversion(_) => BuiltinArity::Exact(1),
        }
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Datetime constructors whose result scale depends on constructor-specific
    /// rules.
    pub enum DatetimeConstructorFn {
        Datetime => "datetime",
        /// `epoch<S>`, applied with a static time scale.
        Epoch => "epoch",
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Gregorian calendar fields extracted from a datetime.
    pub enum DatetimeField {
        Year => "year",
        Month => "month",
        Day => "day",
        Hour => "hour",
        Minute => "minute",
        Second => "second",
        Weekday => "weekday",
        DayOfYear => "day_of_year",
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Numeric-to-datetime constructors.
    pub enum DatetimeFromNumericFn {
        Jd => "from_jd",
        Mjd => "from_mjd",
        Unix => "from_unix",
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Datetime-to-numeric extractors.
    pub enum DatetimeToNumericFn {
        Jd => "to_jd",
        Mjd => "to_mjd",
        Unix => "to_unix",
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Datetime time-scale conversions.
    pub enum TimeScaleConversionFn {
        ToUtc => "to_utc",
        ToTai => "to_tai",
        ToTt => "to_tt",
        ToTdb => "to_tdb",
        ToEt => "to_et",
        ToGpst => "to_gpst",
        ToGst => "to_gst",
        ToBdt => "to_bdt",
        ToQzsst => "to_qzsst",
    }
}

impl TimeScaleConversionFn {
    /// Time scale of the converted datetime.
    #[must_use]
    pub const fn target(self) -> TimeScale {
        match self {
            Self::ToUtc => TimeScale::UTC,
            Self::ToTai => TimeScale::TAI,
            Self::ToTt => TimeScale::TT,
            Self::ToTdb => TimeScale::TDB,
            Self::ToEt => TimeScale::ET,
            Self::ToGpst => TimeScale::GPST,
            Self::ToGst => TimeScale::GST,
            Self::ToBdt => TimeScale::BDT,
            Self::ToQzsst => TimeScale::QZSST,
        }
    }
}

define_builtin_names! {
    parse = fn from_spelling;
    /// Type-category conversions between `Int`, keys, and dimensionless
    /// quantity values.
    pub enum ConversionFn {
        ToFloat => "to_float",
        ToInt => "to_int",
        Coord => "coord",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        AggregationFn, BuiltinApplication, BuiltinArity, BuiltinConst, BuiltinEntry, BuiltinFn,
        ComplexFn, DatetimeConstructorFn, DatetimeFn, LinearAlgebraFn, ScalarFn,
        TimeScaleConversionFn,
    };
    use crate::semantic::time_scale::TimeScale;

    #[test]
    fn every_builtin_function_round_trips_through_its_spelling() {
        let all = BuiltinFn::all().collect::<Vec<_>>();
        assert_eq!(all.len(), 86);
        for function in &all {
            assert_eq!(BuiltinFn::parse(function.as_str()), Some(*function));
            assert_eq!(function.to_string(), function.as_str());
        }
        let spellings = all.iter().map(|f| f.as_str()).collect::<HashSet<_>>();
        assert_eq!(spellings.len(), all.len(), "spellings must be unique");
    }

    #[test]
    fn family_sizes_are_pinned() {
        let count = |pred: fn(&BuiltinFn) -> bool| BuiltinFn::all().filter(pred).count();
        assert_eq!(count(|f| matches!(f, BuiltinFn::Scalar(_))), 30);
        assert_eq!(count(|f| matches!(f, BuiltinFn::Complex(_))), 9);
        assert_eq!(count(|f| matches!(f, BuiltinFn::Aggregation(_))), 9);
        assert_eq!(count(|f| matches!(f, BuiltinFn::LinearAlgebra(_))), 10);
        assert_eq!(count(|f| matches!(f, BuiltinFn::Datetime(_))), 25);
        assert_eq!(count(|f| matches!(f, BuiltinFn::Conversion(_))), 3);
    }

    #[test]
    fn unknown_and_near_miss_spellings_are_rejected() {
        for name in [
            "min",
            "max",
            "",
            "Sqrt",
            "sqrt ",
            "epoch<UTC>",
            "PI",
            "determinant",
        ] {
            assert_eq!(BuiltinFn::parse(name), None, "`{name}` must not parse");
        }
    }

    #[test]
    fn representative_spellings_select_their_family() {
        assert_eq!(
            BuiltinFn::parse("least"),
            Some(BuiltinFn::Scalar(ScalarFn::Least))
        );
        assert_eq!(
            BuiltinFn::parse("abs"),
            Some(BuiltinFn::Complex(ComplexFn::Absolute))
        );
        assert_eq!(
            BuiltinFn::parse("exp"),
            Some(BuiltinFn::Complex(ComplexFn::Exponential))
        );
        assert_eq!(
            BuiltinFn::parse("minimum"),
            Some(BuiltinFn::Aggregation(AggregationFn::Value(
                super::ValueAggregation::Minimum
            )))
        );
        assert_eq!(
            BuiltinFn::parse("argmax"),
            Some(BuiltinFn::Aggregation(AggregationFn::Key(
                super::KeyAggregation::Argmax
            )))
        );
        assert_eq!(
            BuiltinFn::parse("det"),
            Some(BuiltinFn::LinearAlgebra(LinearAlgebraFn::Determinant))
        );
        assert_eq!(
            BuiltinFn::parse("hour"),
            Some(BuiltinFn::Datetime(DatetimeFn::Field(
                super::DatetimeField::Hour
            )))
        );
        assert_eq!(BuiltinFn::parse("epoch"), Some(BuiltinFn::EPOCH));
        assert_eq!(
            BuiltinFn::parse("to_int"),
            Some(BuiltinFn::Conversion(super::ConversionFn::ToInt))
        );
    }

    #[test]
    fn only_epoch_requires_a_time_scale_application() {
        for function in BuiltinFn::all() {
            match function.application() {
                BuiltinApplication::Epoch => assert_eq!(function, BuiltinFn::EPOCH),
                BuiltinApplication::ScaleFree(builtin) => {
                    assert_ne!(function, BuiltinFn::EPOCH);
                    assert_eq!(builtin.function(), function);
                    assert_eq!(builtin.to_string(), function.as_str());
                }
            }
        }
        assert_eq!(
            BuiltinFn::EPOCH,
            BuiltinFn::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Epoch))
        );
    }

    #[test]
    fn entries_have_the_expected_arities() {
        let arity = |name: &str| BuiltinFn::parse(name).unwrap().entry().arity();
        for (name, expected) in [
            ("sqrt", 1),
            ("log", 2),
            ("atan2", 2),
            ("hypot", 2),
            ("clamp", 3),
            ("complex", 2),
            ("polar", 2),
            ("abs", 1),
            ("exp", 1),
            ("sum", 1),
            ("argmin", 1),
            ("dot", 2),
            ("matmul", 2),
            ("transpose", 1),
            ("solve", 2),
            ("det", 1),
            ("epoch", 1),
            ("year", 1),
            ("from_jd", 1),
            ("to_unix", 1),
            ("to_tai", 1),
            ("to_float", 1),
            ("coord", 1),
        ] {
            assert_eq!(arity(name), BuiltinArity::Exact(expected), "`{name}`");
        }
        assert_eq!(
            arity("datetime"),
            BuiltinArity::OptionalTrailing { required: 1 }
        );
    }

    #[test]
    fn entry_kinds_follow_the_family() {
        for function in BuiltinFn::all() {
            let entry = function.entry();
            match (function, entry) {
                (BuiltinFn::Scalar(scalar), BuiltinEntry::Kernel(kernel)) => {
                    assert_eq!(scalar, kernel);
                    assert_eq!(entry.documented_arity(), Some(scalar.arity()));
                }
                (BuiltinFn::Complex(f), BuiltinEntry::Signature(signature)) => {
                    assert_eq!(signature, f.signature());
                    assert_eq!(entry.documented_arity(), Some(f.arity()));
                }
                (BuiltinFn::Aggregation(f), BuiltinEntry::Signature(signature)) => {
                    assert_eq!(signature, f.signature());
                    assert_eq!(entry.documented_arity(), Some(f.arity()));
                }
                (BuiltinFn::LinearAlgebra(f), BuiltinEntry::Signature(signature)) => {
                    assert_eq!(signature, f.signature());
                    assert_eq!(entry.documented_arity(), Some(f.arity()));
                }
                (BuiltinFn::Datetime(f), BuiltinEntry::Bespoke(arity)) => {
                    assert_eq!(arity, f.arity());
                    assert_eq!(entry.documented_arity(), None);
                }
                (BuiltinFn::Conversion(_), BuiltinEntry::Bespoke(arity)) => {
                    assert_eq!(arity, BuiltinArity::Exact(1));
                    assert_eq!(entry.documented_arity(), None);
                }
                (function, entry) => panic!("`{function}` has entry {entry:?}"),
            }
        }
    }

    #[test]
    fn arity_acceptance_and_display() {
        assert!(BuiltinArity::Exact(2).accepts(2));
        assert!(!BuiltinArity::Exact(2).accepts(1));
        assert!(!BuiltinArity::Exact(2).accepts(3));
        let optional = BuiltinArity::OptionalTrailing { required: 1 };
        assert!(!optional.accepts(0));
        assert!(optional.accepts(1));
        assert!(optional.accepts(2));
        assert!(!optional.accepts(3));
        assert_eq!(BuiltinArity::Exact(3).to_string(), "3");
        assert_eq!(optional.to_string(), "1 or 2");
    }

    #[test]
    fn time_scale_conversions_target_their_scale() {
        for (function, scale) in [
            (TimeScaleConversionFn::ToUtc, TimeScale::UTC),
            (TimeScaleConversionFn::ToTai, TimeScale::TAI),
            (TimeScaleConversionFn::ToTt, TimeScale::TT),
            (TimeScaleConversionFn::ToTdb, TimeScale::TDB),
            (TimeScaleConversionFn::ToEt, TimeScale::ET),
            (TimeScaleConversionFn::ToGpst, TimeScale::GPST),
            (TimeScaleConversionFn::ToGst, TimeScale::GST),
            (TimeScaleConversionFn::ToBdt, TimeScale::BDT),
            (TimeScaleConversionFn::ToQzsst, TimeScale::QZSST),
        ] {
            assert_eq!(function.target(), scale);
        }
        assert_eq!(TimeScaleConversionFn::ALL.len(), 9);
    }

    /// User-visible signature help (label and parameter labels) of every
    /// built-in with a static signature; it must stay byte-identical.
    const EXPECTED_SIGNATURES: &[(&str, &str, &[&str])] = &[
        (
            "complex",
            "fn complex<D: Dim>(re: D, im: D) -> Complex<D>",
            &["re: D", "im: D"],
        ),
        (
            "polar",
            "fn polar<D: Dim>(magnitude: D, phase: Angle) -> Complex<D>",
            &["magnitude: D", "phase: Angle"],
        ),
        (
            "to_complex",
            "fn to_complex<D: Dim>(x: D) -> Complex<D>",
            &["x: D"],
        ),
        (
            "re",
            "fn re<D: Dim>(z: Complex<D>) -> D",
            &["z: Complex<D>"],
        ),
        (
            "im",
            "fn im<D: Dim>(z: Complex<D>) -> D",
            &["z: Complex<D>"],
        ),
        (
            "phase",
            "fn phase<D: Dim>(z: Complex<D>) -> Angle",
            &["z: Complex<D>"],
        ),
        (
            "conj",
            "fn conj<D: Dim>(z: Complex<D>) -> Complex<D>",
            &["z: Complex<D>"],
        ),
        (
            "abs",
            "fn abs<D: Dim>(x: D | Complex<D>) -> D",
            &["x: D | Complex<D>"],
        ),
        (
            "exp",
            "fn exp(x: Dimensionless | Complex<Dimensionless>) -> Dimensionless | Complex<Dimensionless>",
            &["x: Dimensionless | Complex<Dimensionless>"],
        ),
        (
            "sum",
            "fn sum<D: Dim, I: Index>(values: D[I]) -> D",
            &["values: D[I]"],
        ),
        (
            "product",
            "fn product<D: Dim, I: Index>(values: D[I]) -> D^|I|",
            &["values: D[I]"],
        ),
        (
            "minimum",
            "fn minimum<D: Dim, I: Index>(values: D[I]) -> D",
            &["values: D[I]"],
        ),
        (
            "maximum",
            "fn maximum<D: Dim, I: Index>(values: D[I]) -> D",
            &["values: D[I]"],
        ),
        (
            "mean",
            "fn mean<D: Dim, I: Index>(values: D[I]) -> D",
            &["values: D[I]"],
        ),
        (
            "rss",
            "fn rss<D: Dim, I: Index>(values: D[I]) -> D",
            &["values: D[I]"],
        ),
        (
            "count",
            "fn count<T: Type, I: Index>(values: T[I]) -> Int",
            &["values: T[I]"],
        ),
        (
            "argmin",
            "fn argmin<D: Dim, I: Index>(values: D[I]) -> Key<I>",
            &["values: D[I]"],
        ),
        (
            "argmax",
            "fn argmax<D: Dim, I: Index>(values: D[I]) -> Key<I>",
            &["values: D[I]"],
        ),
        (
            "dot",
            "fn dot<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> D1 * D2",
            &["a: D1[I]", "b: D2[I]"],
        ),
        (
            "matmul",
            "fn matmul<D1: Dim, D2: Dim, I: Index, J: Index, K: Index>(a: D1[I, J], b: D2[J, K]) -> (D1 * D2)[I, K]",
            &["a: D1[I, J]", "b: D2[J, K]"],
        ),
        (
            "transpose",
            "fn transpose<D: Dim, I: Index, J: Index>(a: D[I, J]) -> D[J, I]",
            &["a: D[I, J]"],
        ),
        (
            "trace",
            "fn trace<D: Dim, I: Index>(a: D[I, I]) -> D",
            &["a: D[I, I]"],
        ),
        (
            "norm",
            "fn norm<D: Dim, I: Index>(v: D[I]) -> D",
            &["v: D[I]"],
        ),
        (
            "cross",
            "fn cross<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> (D1 * D2)[I] where |I| = 3",
            &["a: D1[I]", "b: D2[I]"],
        ),
        (
            "outer",
            "fn outer<D1: Dim, D2: Dim, I: Index, J: Index>(a: D1[I], b: D2[J]) -> (D1 * D2)[I, J]",
            &["a: D1[I]", "b: D2[J]"],
        ),
        (
            "solve",
            "fn solve<D1: Dim, D2: Dim, I: Index>(a: D1[I, I], b: D2[I]) -> (D2 / D1)[I]",
            &["a: D1[I, I]", "b: D2[I]"],
        ),
        (
            "inverse",
            "fn inverse<D: Dim, I: Index>(a: D[I, I]) -> D^-1[I, I]",
            &["a: D[I, I]"],
        ),
        (
            "det",
            "fn det<D: Dim, I: Index>(a: D[I, I]) -> D^|I|",
            &["a: D[I, I]"],
        ),
    ];

    /// Every type variable a documented signature mentions is one of its own
    /// generic parameters, used at the sort that parameter declares.
    #[test]
    fn display_signatures_mention_only_their_own_generics() {
        for function in BuiltinFn::all() {
            let BuiltinEntry::Signature(signature) = function.entry() else {
                continue;
            };
            for (generic, kind) in signature.referenced_generics() {
                assert_eq!(generic.kind, kind, "`{function}` uses `{}`", generic.name);
                assert!(
                    signature.generics.contains(&generic),
                    "`{function}` mentions undeclared `{}`",
                    generic.name
                );
            }
        }
    }

    /// The rendered signatures are user-visible (LSP signature help); they
    /// must match the source-like text exactly.
    #[test]
    fn display_signatures_render_exactly() {
        let expected = EXPECTED_SIGNATURES;
        let mut documented = 0;
        for function in BuiltinFn::all() {
            let BuiltinEntry::Signature(signature) = function.entry() else {
                continue;
            };
            documented += 1;
            let (_, label, params) = expected
                .iter()
                .find(|(name, _, _)| *name == function.as_str())
                .unwrap_or_else(|| panic!("no expected signature for `{function}`"));
            assert_eq!(signature.label(function.as_str()), *label);
            assert_eq!(signature.parameter_labels().collect::<Vec<_>>(), *params);
            assert_eq!(signature.arity(), params.len());
        }
        assert_eq!(documented, expected.len());
    }

    #[test]
    fn typed_constant_catalog_is_exhaustive_and_round_trips() {
        assert_eq!(BuiltinConst::ALL.len(), 6);
        for constant in BuiltinConst::ALL {
            assert_eq!(BuiltinConst::parse(constant.as_str()), Some(*constant));
            assert!(constant.value().is_finite());
        }
        assert_eq!(BuiltinConst::parse("pi"), None);
    }
}
