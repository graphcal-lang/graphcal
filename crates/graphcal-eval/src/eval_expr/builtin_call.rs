//! Evaluation routing for built-in calls.
//!
//! `graphcal_compiler::builtin` owns the source vocabulary. This module owns the
//! narrower question asked by runtime HIR evaluation: can a built-in call be
//! evaluated through the ordinary scalar built-in function registry, or does it
//! need a custom runtime path here?

use graphcal_compiler::builtin::BuiltinFnName;
use graphcal_compiler::registry::time_scale::TimeScale;

/// How HIR evaluation should dispatch a built-in function call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvalBuiltinRule {
    /// Use [`graphcal_compiler::registry::builtins::BuiltinFunction::eval`].
    RegistryFunction,
    /// One-argument reductions over indexed values.
    CollectionAggregation(AggregationFn),
    /// Type-category conversions between `Int` and dimensionless scalar values.
    TypeConversion(TypeConversionFn),
    /// Datetime time-scale conversion to the carried target scale.
    TimeScaleConversion(TimeScale),
    /// Datetime constructors whose result depends on constructor-specific rules.
    DatetimeConstructor(DatetimeConstructorFn),
    /// Datetime component extraction functions.
    DatetimeExtract(DatetimeExtractFn),
    /// Numeric-to-datetime constructors.
    DatetimeFromNumeric(DatetimeFromFn),
    /// Datetime-to-numeric extractors.
    DatetimeToNumeric(DatetimeToFn),
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

/// Type conversion functions handled by HIR evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypeConversionFn {
    ToFloat,
    ToInt,
}

/// Datetime constructor functions handled by HIR evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DatetimeConstructorFn {
    Datetime,
    Epoch,
}

/// Datetime component extraction functions handled by HIR evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DatetimeExtractFn {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Weekday,
    DayOfYear,
}

/// Numeric-to-datetime constructors handled by HIR evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DatetimeFromFn {
    Jd,
    Mjd,
    Unix,
}

/// Datetime-to-numeric extractors handled by HIR evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DatetimeToFn {
    Jd,
    Mjd,
    Unix,
}

/// Classify a built-in for the runtime HIR evaluation call path.
#[must_use]
pub(super) const fn eval_rule_for_builtin(name: BuiltinFnName) -> EvalBuiltinRule {
    match name {
        BuiltinFnName::Sum => EvalBuiltinRule::CollectionAggregation(AggregationFn::Sum),
        BuiltinFnName::Min => EvalBuiltinRule::CollectionAggregation(AggregationFn::Min),
        BuiltinFnName::Max => EvalBuiltinRule::CollectionAggregation(AggregationFn::Max),
        BuiltinFnName::Mean => EvalBuiltinRule::CollectionAggregation(AggregationFn::Mean),
        BuiltinFnName::Count => EvalBuiltinRule::CollectionAggregation(AggregationFn::Count),
        BuiltinFnName::ToFloat => EvalBuiltinRule::TypeConversion(TypeConversionFn::ToFloat),
        BuiltinFnName::ToInt => EvalBuiltinRule::TypeConversion(TypeConversionFn::ToInt),
        BuiltinFnName::ToUtc => EvalBuiltinRule::TimeScaleConversion(TimeScale::UTC),
        BuiltinFnName::ToTai => EvalBuiltinRule::TimeScaleConversion(TimeScale::TAI),
        BuiltinFnName::ToTt => EvalBuiltinRule::TimeScaleConversion(TimeScale::TT),
        BuiltinFnName::ToTdb => EvalBuiltinRule::TimeScaleConversion(TimeScale::TDB),
        BuiltinFnName::ToEt => EvalBuiltinRule::TimeScaleConversion(TimeScale::ET),
        BuiltinFnName::ToGpst => EvalBuiltinRule::TimeScaleConversion(TimeScale::GPST),
        BuiltinFnName::ToGst => EvalBuiltinRule::TimeScaleConversion(TimeScale::GST),
        BuiltinFnName::ToBdt => EvalBuiltinRule::TimeScaleConversion(TimeScale::BDT),
        BuiltinFnName::ToQzsst => EvalBuiltinRule::TimeScaleConversion(TimeScale::QZSST),
        BuiltinFnName::Datetime => {
            EvalBuiltinRule::DatetimeConstructor(DatetimeConstructorFn::Datetime)
        }
        BuiltinFnName::Epoch => EvalBuiltinRule::DatetimeConstructor(DatetimeConstructorFn::Epoch),
        BuiltinFnName::Year => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Year),
        BuiltinFnName::Month => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Month),
        BuiltinFnName::Day => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Day),
        BuiltinFnName::Hour => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Hour),
        BuiltinFnName::Minute => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Minute),
        BuiltinFnName::Second => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Second),
        BuiltinFnName::Weekday => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::Weekday),
        BuiltinFnName::DayOfYear => EvalBuiltinRule::DatetimeExtract(DatetimeExtractFn::DayOfYear),
        BuiltinFnName::FromJd => EvalBuiltinRule::DatetimeFromNumeric(DatetimeFromFn::Jd),
        BuiltinFnName::FromMjd => EvalBuiltinRule::DatetimeFromNumeric(DatetimeFromFn::Mjd),
        BuiltinFnName::FromUnix => EvalBuiltinRule::DatetimeFromNumeric(DatetimeFromFn::Unix),
        BuiltinFnName::ToJd => EvalBuiltinRule::DatetimeToNumeric(DatetimeToFn::Jd),
        BuiltinFnName::ToMjd => EvalBuiltinRule::DatetimeToNumeric(DatetimeToFn::Mjd),
        BuiltinFnName::ToUnix => EvalBuiltinRule::DatetimeToNumeric(DatetimeToFn::Unix),
        _ => EvalBuiltinRule::RegistryFunction,
    }
}

#[cfg(test)]
mod tests {
    use super::{EvalBuiltinRule, eval_rule_for_builtin};
    use graphcal_compiler::builtin::BuiltinFnName;
    use graphcal_compiler::registry::builtins::builtin_functions;

    #[test]
    fn every_builtin_name_has_an_eval_route() {
        let ordinary_registry_functions = builtin_functions();
        for name in BuiltinFnName::ALL {
            match eval_rule_for_builtin(*name) {
                EvalBuiltinRule::RegistryFunction => assert!(
                    ordinary_registry_functions.contains_key(name.as_str()),
                    "BuiltinFnName::{name:?} (`{}`) is neither in builtin_functions() nor handled by a custom eval rule",
                    name.as_str()
                ),
                EvalBuiltinRule::CollectionAggregation(_)
                | EvalBuiltinRule::TypeConversion(_)
                | EvalBuiltinRule::TimeScaleConversion(_)
                | EvalBuiltinRule::DatetimeConstructor(_)
                | EvalBuiltinRule::DatetimeExtract(_)
                | EvalBuiltinRule::DatetimeFromNumeric(_)
                | EvalBuiltinRule::DatetimeToNumeric(_) => {}
            }
        }
    }
}
