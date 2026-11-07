//! Regressions for the datetime contract: leap seconds, time-scale
//! conversion, UTC arithmetic, and numeric epoch conversions (#2246).
#![cfg(test)]

use graphcal_eval::eval::{EvalResult, Value};
use graphcal_project::prepare::compile_and_eval;

fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .entries
        .iter()
        .find(|graphcal_eval::eval::EvalEntry { name: n, .. }| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .outcome
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

fn assert_true(result: &EvalResult, name: &str) {
    match find_entry(result, name) {
        Value::Bool(value) => assert!(value, "`{name}` must be true"),
        other => panic!("expected Bool for `{name}`, got {other:?}"),
    }
}

// MDL-8: TAI→UTC must keep the old TAI−UTC offset until UTC midnight, which is
// ΔAT seconds after the TAI midnight hifitime switched at.
#[test]
fn tai_to_utc_keeps_the_old_offset_before_a_leap_second() {
    let result = compile_and_eval(
        r#"
node before_2015: Bool = to_utc(epoch<TAI>("2015-07-01T00:00:00")) == datetime("2015-06-30T23:59:25Z");
node midnight_2015: Bool = to_utc(epoch<TAI>("2015-07-01T00:00:36")) == datetime("2015-07-01T00:00:00Z");
node before_2017: Bool = to_utc(epoch<TAI>("2017-01-01T00:00:00")) == datetime("2016-12-31T23:59:24Z");
node via_tt: Bool = to_utc(epoch<TT>("2017-01-01T00:00:32.184")) == datetime("2016-12-31T23:59:24Z");
"#,
    )
    .unwrap();
    for name in ["before_2015", "midnight_2015", "before_2017", "via_tt"] {
        assert_true(&result, name);
    }
}
