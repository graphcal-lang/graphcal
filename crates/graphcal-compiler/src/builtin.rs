//! Built-in language domain model.
//!
//! This module owns the closed source vocabulary for built-in constants and
//! functions. String spellings enter the compiler through [`BuiltinConst::parse`]
//! and [`BuiltinFnName::parse`]; downstream phases carry these typed variants
//! instead of matching raw names.

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
    pub(crate) const fn value(self) -> f64 {
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
