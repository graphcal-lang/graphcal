//! Type-inference routing for built-in calls.
//!
//! [`crate::builtin`] owns the source vocabulary. This module owns the narrower
//! question asked by HIR type inference: can a built-in call be checked through
//! the ordinary scalar dimension-signature registry, or does it need a custom
//! type rule here?

use crate::builtin::BuiltinFnName;
use crate::registry::time_scale::TimeScale;

/// How HIR type inference should check a built-in function call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BuiltinTypeRule {
    /// Use [`crate::registry::builtins::BuiltinFunction::dim_sig`].
    RegistrySignature,
    /// One-argument reductions over indexed values.
    CollectionAggregation(AggregationFn),
    /// Type-category conversions between `Int` and dimensionless scalar values.
    TypeConversion(TypeConversionFn),
    /// Datetime time-scale conversion to the carried target scale.
    TimeScaleConversion(TimeScale),
    /// Datetime constructors whose result scale depends on constructor-specific rules.
    DatetimeConstructor(DatetimeConstructorFn),
    /// Datetime component extraction functions such as `year` and `second`.
    DatetimeExtract,
    /// Numeric-to-datetime constructors such as `from_jd`.
    DatetimeFromNumeric,
    /// Datetime-to-numeric extractors such as `to_jd`.
    DatetimeToNumeric,
}

/// Aggregation functions that can reduce an indexed collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AggregationFn {
    Sum,
    Min,
    Max,
    Mean,
    Count,
}

/// Type conversion functions handled by HIR type inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypeConversionFn {
    ToFloat,
    ToInt,
}

impl TypeConversionFn {
    /// Canonical source spelling, for diagnostics.
    #[must_use]
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ToFloat => BuiltinFnName::ToFloat.as_str(),
            Self::ToInt => BuiltinFnName::ToInt.as_str(),
        }
    }
}

/// Datetime constructor functions handled by HIR type inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DatetimeConstructorFn {
    Datetime,
    Epoch,
}

/// Classify a built-in for the type-inference call path.
#[must_use]
pub(super) const fn type_rule_for_builtin(name: BuiltinFnName) -> BuiltinTypeRule {
    match name {
        BuiltinFnName::Sum => BuiltinTypeRule::CollectionAggregation(AggregationFn::Sum),
        BuiltinFnName::Min => BuiltinTypeRule::CollectionAggregation(AggregationFn::Min),
        BuiltinFnName::Max => BuiltinTypeRule::CollectionAggregation(AggregationFn::Max),
        BuiltinFnName::Mean => BuiltinTypeRule::CollectionAggregation(AggregationFn::Mean),
        BuiltinFnName::Count => BuiltinTypeRule::CollectionAggregation(AggregationFn::Count),
        BuiltinFnName::ToFloat => BuiltinTypeRule::TypeConversion(TypeConversionFn::ToFloat),
        BuiltinFnName::ToInt => BuiltinTypeRule::TypeConversion(TypeConversionFn::ToInt),
        BuiltinFnName::ToUtc => BuiltinTypeRule::TimeScaleConversion(TimeScale::UTC),
        BuiltinFnName::ToTai => BuiltinTypeRule::TimeScaleConversion(TimeScale::TAI),
        BuiltinFnName::ToTt => BuiltinTypeRule::TimeScaleConversion(TimeScale::TT),
        BuiltinFnName::ToTdb => BuiltinTypeRule::TimeScaleConversion(TimeScale::TDB),
        BuiltinFnName::ToEt => BuiltinTypeRule::TimeScaleConversion(TimeScale::ET),
        BuiltinFnName::ToGpst => BuiltinTypeRule::TimeScaleConversion(TimeScale::GPST),
        BuiltinFnName::ToGst => BuiltinTypeRule::TimeScaleConversion(TimeScale::GST),
        BuiltinFnName::ToBdt => BuiltinTypeRule::TimeScaleConversion(TimeScale::BDT),
        BuiltinFnName::ToQzsst => BuiltinTypeRule::TimeScaleConversion(TimeScale::QZSST),
        BuiltinFnName::Datetime => {
            BuiltinTypeRule::DatetimeConstructor(DatetimeConstructorFn::Datetime)
        }
        BuiltinFnName::Epoch => BuiltinTypeRule::DatetimeConstructor(DatetimeConstructorFn::Epoch),
        BuiltinFnName::Year
        | BuiltinFnName::Month
        | BuiltinFnName::Day
        | BuiltinFnName::Hour
        | BuiltinFnName::Minute
        | BuiltinFnName::Second
        | BuiltinFnName::Weekday
        | BuiltinFnName::DayOfYear => BuiltinTypeRule::DatetimeExtract,
        BuiltinFnName::FromJd | BuiltinFnName::FromMjd | BuiltinFnName::FromUnix => {
            BuiltinTypeRule::DatetimeFromNumeric
        }
        BuiltinFnName::ToJd | BuiltinFnName::ToMjd | BuiltinFnName::ToUnix => {
            BuiltinTypeRule::DatetimeToNumeric
        }
        _ => BuiltinTypeRule::RegistrySignature,
    }
}

#[cfg(test)]
mod tests {
    use super::{BuiltinTypeRule, type_rule_for_builtin};
    use crate::builtin::BuiltinFnName;
    use crate::registry::builtins::builtin_functions;

    #[test]
    fn every_builtin_name_has_a_type_inference_route() {
        let ordinary_registry_functions = builtin_functions();
        for name in BuiltinFnName::ALL {
            match type_rule_for_builtin(*name) {
                BuiltinTypeRule::RegistrySignature => assert!(
                    ordinary_registry_functions.contains_key(name.as_str()),
                    "BuiltinFnName::{name:?} (`{}`) is neither in builtin_functions() nor handled by a custom HIR type rule",
                    name.as_str()
                ),
                BuiltinTypeRule::CollectionAggregation(_)
                | BuiltinTypeRule::TypeConversion(_)
                | BuiltinTypeRule::TimeScaleConversion(_)
                | BuiltinTypeRule::DatetimeConstructor(_)
                | BuiltinTypeRule::DatetimeExtract
                | BuiltinTypeRule::DatetimeFromNumeric
                | BuiltinTypeRule::DatetimeToNumeric => {}
            }
        }
    }
}
