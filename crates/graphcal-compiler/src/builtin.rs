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

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
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
        Least => "least",
        Greatest => "greatest",
        Hypot => "hypot",
        Clamp => "clamp",
        Dot => "dot",
        Matmul => "matmul",
        Transpose => "transpose",
        Trace => "trace",
        Norm => "norm",
        Cross => "cross",
        Outer => "outer",
        Solve => "solve",
        Inverse => "inverse",
        Det => "det",
        Sum => "sum",
        Product => "product",
        Minimum => "minimum",
        Maximum => "maximum",
        Mean => "mean",
        Rss => "rss",
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

/// Built-in reductions over rank-one indexed collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregationFn {
    Sum,
    Product,
    Minimum,
    Maximum,
    Mean,
    RootSumSquare,
    Count,
}

impl AggregationFn {
    /// Every aggregation, for routing and signature-display tests.
    pub const ALL: &'static [Self] = &[
        Self::Sum,
        Self::Product,
        Self::Minimum,
        Self::Maximum,
        Self::Mean,
        Self::RootSumSquare,
        Self::Count,
    ];

    /// Canonical built-in function identity.
    #[must_use]
    pub const fn builtin_name(self) -> BuiltinFnName {
        match self {
            Self::Sum => BuiltinFnName::Sum,
            Self::Product => BuiltinFnName::Product,
            Self::Minimum => BuiltinFnName::Minimum,
            Self::Maximum => BuiltinFnName::Maximum,
            Self::Mean => BuiltinFnName::Mean,
            Self::RootSumSquare => BuiltinFnName::Rss,
            Self::Count => BuiltinFnName::Count,
        }
    }

    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> usize {
        1
    }

    /// Source-like parameter labels used at LSP/display boundaries.
    #[must_use]
    pub const fn parameter_labels(self) -> &'static [&'static str] {
        match self {
            Self::Count => &["values: T[I]"],
            Self::Sum
            | Self::Product
            | Self::Minimum
            | Self::Maximum
            | Self::Mean
            | Self::RootSumSquare => &["values: D[I]"],
        }
    }

    /// Source-like signature label used at LSP/display boundaries.
    #[must_use]
    pub const fn signature(self) -> &'static str {
        match self {
            Self::Sum => "fn sum<D: Dim, I: Index>(values: D[I]) -> D",
            Self::Product => "fn product<D: Dim, I: Index>(values: D[I]) -> D^|I|",
            Self::Minimum => "fn minimum<D: Dim, I: Index>(values: D[I]) -> D",
            Self::Maximum => "fn maximum<D: Dim, I: Index>(values: D[I]) -> D",
            Self::Mean => "fn mean<D: Dim, I: Index>(values: D[I]) -> D",
            Self::RootSumSquare => "fn rss<D: Dim, I: Index>(values: D[I]) -> D",
            Self::Count => "fn count<T: Type, I: Index>(values: T[I]) -> Int",
        }
    }
}

/// Built-in linear-algebra operations over rank-one and rank-two indexed
/// quantities.
///
/// This typed category is shared by dimension checking and evaluation. Source
/// spellings cross into it only through [`BuiltinFnName::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinearAlgebraFn {
    Dot,
    Matmul,
    Transpose,
    Trace,
    Norm,
    Cross,
    Outer,
    Solve,
    Inverse,
    Determinant,
}

impl LinearAlgebraFn {
    /// Every linear-algebra operation, for display-table and routing tests.
    pub const ALL: &'static [Self] = &[
        Self::Dot,
        Self::Matmul,
        Self::Transpose,
        Self::Trace,
        Self::Norm,
        Self::Cross,
        Self::Outer,
        Self::Solve,
        Self::Inverse,
        Self::Determinant,
    ];

    /// Canonical built-in function identity.
    #[must_use]
    pub const fn builtin_name(self) -> BuiltinFnName {
        match self {
            Self::Dot => BuiltinFnName::Dot,
            Self::Matmul => BuiltinFnName::Matmul,
            Self::Transpose => BuiltinFnName::Transpose,
            Self::Trace => BuiltinFnName::Trace,
            Self::Norm => BuiltinFnName::Norm,
            Self::Cross => BuiltinFnName::Cross,
            Self::Outer => BuiltinFnName::Outer,
            Self::Solve => BuiltinFnName::Solve,
            Self::Inverse => BuiltinFnName::Inverse,
            Self::Determinant => BuiltinFnName::Det,
        }
    }

    /// Number of runtime arguments.
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::Transpose | Self::Trace | Self::Norm | Self::Inverse | Self::Determinant => 1,
            Self::Dot | Self::Matmul | Self::Cross | Self::Outer | Self::Solve => 2,
        }
    }

    /// Source-like parameter labels used at LSP/display boundaries.
    #[must_use]
    pub const fn parameter_labels(self) -> &'static [&'static str] {
        match self {
            Self::Dot | Self::Cross => &["a: D1[I]", "b: D2[I]"],
            Self::Matmul => &["a: D1[I, J]", "b: D2[J, K]"],
            Self::Transpose => &["a: D[I, J]"],
            Self::Trace | Self::Inverse | Self::Determinant => &["a: D[I, I]"],
            Self::Norm => &["v: D[I]"],
            Self::Outer => &["a: D1[I]", "b: D2[J]"],
            Self::Solve => &["a: D1[I, I]", "b: D2[I]"],
        }
    }

    /// Source-like signature label used at LSP/display boundaries.
    #[must_use]
    pub const fn signature(self) -> &'static str {
        match self {
            Self::Dot => "fn dot<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> D1 * D2",
            Self::Matmul => {
                "fn matmul<D1: Dim, D2: Dim, I: Index, J: Index, K: Index>(a: D1[I, J], b: D2[J, K]) -> (D1 * D2)[I, K]"
            }
            Self::Transpose => "fn transpose<D: Dim, I: Index, J: Index>(a: D[I, J]) -> D[J, I]",
            Self::Trace => "fn trace<D: Dim, I: Index>(a: D[I, I]) -> D",
            Self::Norm => "fn norm<D: Dim, I: Index>(v: D[I]) -> D",
            Self::Cross => {
                "fn cross<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> (D1 * D2)[I] where |I| = 3"
            }
            Self::Outer => {
                "fn outer<D1: Dim, D2: Dim, I: Index, J: Index>(a: D1[I], b: D2[J]) -> (D1 * D2)[I, J]"
            }
            Self::Solve => {
                "fn solve<D1: Dim, D2: Dim, I: Index>(a: D1[I, I], b: D2[I]) -> (D2 / D1)[I]"
            }
            Self::Inverse => "fn inverse<D: Dim, I: Index>(a: D[I, I]) -> D^-1[I, I]",
            Self::Determinant => "fn det<D: Dim, I: Index>(a: D[I, I]) -> D^|I|",
        }
    }
}

impl BuiltinFnName {
    /// Classify this built-in as an indexed aggregation.
    #[must_use]
    pub const fn aggregation(self) -> Option<AggregationFn> {
        match self {
            Self::Sum => Some(AggregationFn::Sum),
            Self::Product => Some(AggregationFn::Product),
            Self::Minimum => Some(AggregationFn::Minimum),
            Self::Maximum => Some(AggregationFn::Maximum),
            Self::Mean => Some(AggregationFn::Mean),
            Self::Rss => Some(AggregationFn::RootSumSquare),
            Self::Count => Some(AggregationFn::Count),
            _ => None,
        }
    }

    /// Classify this built-in as a linear-algebra operation.
    #[must_use]
    pub const fn linear_algebra(self) -> Option<LinearAlgebraFn> {
        match self {
            Self::Dot => Some(LinearAlgebraFn::Dot),
            Self::Matmul => Some(LinearAlgebraFn::Matmul),
            Self::Transpose => Some(LinearAlgebraFn::Transpose),
            Self::Trace => Some(LinearAlgebraFn::Trace),
            Self::Norm => Some(LinearAlgebraFn::Norm),
            Self::Cross => Some(LinearAlgebraFn::Cross),
            Self::Outer => Some(LinearAlgebraFn::Outer),
            Self::Solve => Some(LinearAlgebraFn::Solve),
            Self::Inverse => Some(LinearAlgebraFn::Inverse),
            Self::Det => Some(LinearAlgebraFn::Determinant),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AggregationFn, BuiltinFnName, LinearAlgebraFn};

    #[test]
    fn aggregation_names_have_one_typed_route() {
        for function in AggregationFn::ALL {
            let name = function.builtin_name();
            assert_eq!(BuiltinFnName::parse(name.as_str()), Some(name));
            assert_eq!(name.aggregation(), Some(*function));
        }
        let classified = BuiltinFnName::ALL
            .iter()
            .filter(|name| name.aggregation().is_some())
            .count();
        assert_eq!(classified, AggregationFn::ALL.len());
    }

    #[test]
    fn linear_algebra_names_have_one_typed_route() {
        for function in LinearAlgebraFn::ALL {
            let name = function.builtin_name();
            assert_eq!(BuiltinFnName::parse(name.as_str()), Some(name));
            assert_eq!(name.linear_algebra(), Some(*function));
        }
        let classified = BuiltinFnName::ALL
            .iter()
            .filter(|name| name.linear_algebra().is_some())
            .count();
        assert_eq!(classified, LinearAlgebraFn::ALL.len());
    }

    #[test]
    fn extremum_function_spellings_are_canonical() {
        assert_eq!(BuiltinFnName::parse("least"), Some(BuiltinFnName::Least));
        assert_eq!(
            BuiltinFnName::parse("greatest"),
            Some(BuiltinFnName::Greatest)
        );
        assert_eq!(
            BuiltinFnName::parse("minimum"),
            Some(BuiltinFnName::Minimum)
        );
        assert_eq!(
            BuiltinFnName::parse("maximum"),
            Some(BuiltinFnName::Maximum)
        );
        assert_eq!(BuiltinFnName::parse("min"), None);
        assert_eq!(BuiltinFnName::parse("max"), None);
    }

    #[test]
    fn datetime_hour_extractor_keeps_its_spelling() {
        assert_eq!(BuiltinFnName::parse("hour"), Some(BuiltinFnName::Hour));
    }
}
