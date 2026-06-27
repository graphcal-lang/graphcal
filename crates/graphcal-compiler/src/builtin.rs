//! Built-in language domain model.
//!
//! This module owns the closed set of built-in constants/functions and their
//! typed semantic classifications. String spellings enter the compiler through
//! [`BuiltinConst::parse`] and [`BuiltinFnName::parse`]; downstream phases carry
//! these typed variants instead of matching raw names.

use crate::registry::time_scale::TimeScale;

/// Define a closed set of built-in names: the enum, the `parse` boundary
/// crossing, the canonical `as_str` rendering, and an `ALL` listing for
/// cross-table consistency tests — all generated from a single table so the
/// spellings can never drift apart.
macro_rules! define_builtin_names {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name { $($variant),+ }

        impl $name {
            /// Every variant, for cross-table consistency tests.
            $vis const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Parse a source name into the typed variant — the only place
            /// these strings cross into the typed core.
            #[must_use]
            $vis fn parse(name: &str) -> Option<Self> {
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
    };
}

define_builtin_names! {
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
    /// Numeric value of the constant. Must agree with
    /// [`crate::registry::builtins::builtin_constants`] (enforced by test).
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

define_builtin_names! {
    /// Built-in function names with closed semantic meaning.
    pub enum BuiltinFnName {
        Sqrt => "sqrt",
        Cbrt => "cbrt",
        Exp => "exp",
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
        Abs => "abs",
        Floor => "floor",
        Ceil => "ceil",
        Round => "round",
        Trunc => "trunc",
        Sign => "sign",
        Min => "min",
        Max => "max",
        Hypot => "hypot",
        Clamp => "clamp",
        Sum => "sum",
        Mean => "mean",
        Count => "count",
        ToFloat => "to_float",
        ToInt => "to_int",
        ToUtc => "to_utc",
        ToTai => "to_tai",
        ToTt => "to_tt",
        ToTdb => "to_tdb",
        ToEt => "to_et",
        ToGpst => "to_gpst",
        ToGst => "to_gst",
        ToBdt => "to_bdt",
        ToQzsst => "to_qzsst",
        Datetime => "datetime",
        Epoch => "epoch",
        Year => "year",
        Month => "month",
        Day => "day",
        Hour => "hour",
        Minute => "minute",
        Second => "second",
        Weekday => "weekday",
        DayOfYear => "day_of_year",
        FromJd => "from_jd",
        FromMjd => "from_mjd",
        FromUnix => "from_unix",
        ToJd => "to_jd",
        ToMjd => "to_mjd",
        ToUnix => "to_unix",
    }
}

impl BuiltinFnName {
    /// Return the typed special-function classification when this built-in is
    /// one of the non-ordinary call categories.
    #[must_use]
    pub const fn special_kind(self) -> Option<SpecialFnKind> {
        match self {
            Self::Sum => Some(SpecialFnKind::Aggregation(AggregationFn::Sum)),
            Self::Min => Some(SpecialFnKind::Aggregation(AggregationFn::Min)),
            Self::Max => Some(SpecialFnKind::Aggregation(AggregationFn::Max)),
            Self::Mean => Some(SpecialFnKind::Aggregation(AggregationFn::Mean)),
            Self::Count => Some(SpecialFnKind::Aggregation(AggregationFn::Count)),
            Self::ToFloat => Some(SpecialFnKind::TypeConversion(TypeConversionFn::ToFloat)),
            Self::ToInt => Some(SpecialFnKind::TypeConversion(TypeConversionFn::ToInt)),
            Self::ToUtc => Some(SpecialFnKind::TimeScaleConversion(TimeScale::UTC)),
            Self::ToTai => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TAI)),
            Self::ToTt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TT)),
            Self::ToTdb => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TDB)),
            Self::ToEt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::ET)),
            Self::ToGpst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::GPST)),
            Self::ToGst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::GST)),
            Self::ToBdt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::BDT)),
            Self::ToQzsst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::QZSST)),
            Self::Datetime => Some(SpecialFnKind::Constructor(ConstructorFn::Datetime)),
            Self::Epoch => Some(SpecialFnKind::Constructor(ConstructorFn::Epoch)),
            Self::Year => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Year)),
            Self::Month => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Month)),
            Self::Day => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Day)),
            Self::Hour => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Hour)),
            Self::Minute => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Minute)),
            Self::Second => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Second)),
            Self::Weekday => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Weekday)),
            Self::DayOfYear => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::DayOfYear)),
            Self::FromJd => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromJd)),
            Self::FromMjd => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromMjd)),
            Self::FromUnix => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromUnix)),
            Self::ToJd => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToJd)),
            Self::ToMjd => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToMjd)),
            Self::ToUnix => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToUnix)),
            _ => None,
        }
    }
}

/// Aggregation functions: operate on indexed collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregationFn {
    Sum,
    Min,
    Max,
    Mean,
    Count,
}

impl AggregationFn {
    /// Canonical built-in function represented by this aggregation category.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::Sum => BuiltinFnName::Sum,
            Self::Min => BuiltinFnName::Min,
            Self::Max => BuiltinFnName::Max,
            Self::Mean => BuiltinFnName::Mean,
            Self::Count => BuiltinFnName::Count,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Type conversion functions: `to_float`, `to_int`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeConversionFn {
    ToFloat,
    ToInt,
}

impl TypeConversionFn {
    /// Canonical built-in function represented by this conversion category.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::ToFloat => BuiltinFnName::ToFloat,
            Self::ToInt => BuiltinFnName::ToInt,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Datetime constructor functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstructorFn {
    Datetime,
    Epoch,
}

impl ConstructorFn {
    /// Canonical built-in function represented by this constructor category.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::Datetime => BuiltinFnName::Datetime,
            Self::Epoch => BuiltinFnName::Epoch,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Datetime extraction functions: extract a component from a `Datetime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatetimeExtractFn {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Weekday,
    DayOfYear,
}

impl DatetimeExtractFn {
    /// Canonical built-in function represented by this extractor category.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::Year => BuiltinFnName::Year,
            Self::Month => BuiltinFnName::Month,
            Self::Day => BuiltinFnName::Day,
            Self::Hour => BuiltinFnName::Hour,
            Self::Minute => BuiltinFnName::Minute,
            Self::Second => BuiltinFnName::Second,
            Self::Weekday => BuiltinFnName::Weekday,
            Self::DayOfYear => BuiltinFnName::DayOfYear,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Datetime-from-numeric constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatetimeFromFn {
    FromJd,
    FromMjd,
    FromUnix,
}

impl DatetimeFromFn {
    /// Canonical built-in function represented by this numeric constructor.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::FromJd => BuiltinFnName::FromJd,
            Self::FromMjd => BuiltinFnName::FromMjd,
            Self::FromUnix => BuiltinFnName::FromUnix,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Datetime-to-numeric functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatetimeToFn {
    ToJd,
    ToMjd,
    ToUnix,
}

impl DatetimeToFn {
    /// Canonical built-in function represented by this numeric extractor.
    #[must_use]
    pub const fn as_builtin(self) -> BuiltinFnName {
        match self {
            Self::ToJd => BuiltinFnName::ToJd,
            Self::ToMjd => BuiltinFnName::ToMjd,
            Self::ToUnix => BuiltinFnName::ToUnix,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.as_builtin().as_str()
    }
}

/// Classification of special built-in functions.
///
/// Each variant carries a sub-enum identifying the specific function, so
/// downstream handlers can match on typed variants instead of raw strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialFnKind {
    /// Aggregation functions: `sum`, `min`, `max`, `mean`, `count`.
    Aggregation(AggregationFn),
    /// Type conversion functions: `to_float`, `to_int`.
    TypeConversion(TypeConversionFn),
    /// Time-scale conversion functions: `to_utc`, `to_tai`, etc.
    TimeScaleConversion(TimeScale),
    /// Constructor functions: `datetime`, `epoch`.
    Constructor(ConstructorFn),
    /// Datetime extraction functions: `year`, `month`, `day`, etc.
    DatetimeExtract(DatetimeExtractFn),
    /// Datetime-from-numeric functions: `from_jd`, `from_mjd`, `from_unix`.
    DatetimeFrom(DatetimeFromFn),
    /// Datetime-to-numeric functions: `to_jd`, `to_mjd`, `to_unix`.
    DatetimeTo(DatetimeToFn),
}

/// Classify a function name as a special built-in function.
///
/// Returns `None` if the name is not a recognized special function.
#[must_use]
pub fn classify_special_fn(name: &str) -> Option<SpecialFnKind> {
    BuiltinFnName::parse(name).and_then(BuiltinFnName::special_kind)
}

/// Returns `true` if `name` is a built-in aggregation function (`sum`, `min`, etc.).
#[must_use]
pub fn is_aggregation_fn(name: &str) -> bool {
    matches!(
        classify_special_fn(name),
        Some(SpecialFnKind::Aggregation(_))
    )
}
