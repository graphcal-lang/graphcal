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

fn assert_compile_error(source: &str, code: &str) {
    let error = compile_and_eval(source).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(
        miette::Diagnostic::code(&error).is_some_and(|c| c.to_string() == code),
        "expected {code}, got {rendered}"
    );
}

// TYC-4: `-` and `± Time` on `Datetime<UTC>` are rejected (decision D1);
// every other scale keeps its arithmetic and UTC keeps its comparisons.
#[test]
fn utc_arithmetic_is_rejected_in_every_operator_position() {
    for (ty, body) in [
        ("Time", "@b - @a"),
        ("Datetime", "@a + 1.0 s"),
        ("Datetime", "@a - 1.0 s"),
        ("Datetime", "1.0 s + @a"),
    ] {
        let source = format!(
            r#"
node a: Datetime = datetime("2016-12-31T23:59:59Z");
node b: Datetime = datetime("2017-01-01T00:00:00Z");
node bad: {ty} = {body};
"#
        );
        assert_compile_error(&source, "graphcal::D044");
    }
}

#[test]
fn tai_arithmetic_and_utc_comparisons_stay_defined() {
    let result = compile_and_eval(
        r#"
node a: Datetime = datetime("2016-12-31T23:59:59Z");
node b: Datetime = datetime("2017-01-01T00:00:00Z");
node elapsed_is_two_seconds: Bool = to_tai(@b) - to_tai(@a) == 2.0 s;
node shifted: Bool = to_utc(to_tai(@a) + 2.0 s) == @b;
node ordered: Bool = @a < @b;
node tt_shift: Bool = epoch<TT>("2000-01-01T12:00:00") + 1.0 s == epoch<TT>("2000-01-01T12:00:01");
"#,
    )
    .unwrap();
    for name in ["elapsed_is_two_seconds", "shifted", "ordered", "tt_shift"] {
        assert_true(&result, name);
    }
}

// TYC-2: the Julian-date functions are removed with a dedicated diagnostic, and
// `to_unix` accepts only `Datetime<UTC>` (decision D2).
#[test]
fn removed_julian_date_functions_fail_loudly() {
    for call in [
        "to_jd(datetime(\"2000-01-01T12:00:00Z\"))",
        "to_mjd(datetime(\"2000-01-01T12:00:00Z\"))",
    ] {
        assert_compile_error(
            &format!("node x: Dimensionless = {call};"),
            "graphcal::N038",
        );
    }
    for call in ["from_jd(2451545.0)", "from_mjd(51544.5)"] {
        assert_compile_error(&format!("node x: Datetime = {call};"), "graphcal::N038");
    }
}

#[test]
fn to_unix_rejects_every_non_utc_scale() {
    for scale in ["TAI", "TT", "TDB", "ET", "GPST", "GST", "BDT", "QZSST"] {
        assert_compile_error(
            &format!(r#"node x: Dimensionless = to_unix(epoch<{scale}>("2024-01-01T00:00:00"));"#),
            "graphcal::D001",
        );
    }
}
