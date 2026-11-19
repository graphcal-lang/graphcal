//! Datetime construction and declared-scale Gregorian field extraction.

use std::cmp::Ordering;

use graphcal_compiler::datetime_literal::{OffsetDateTimeLiteral, ZonedDateTimeLiteral};
use num_rational::BigRational;
use num_traits::ToPrimitive;
use thiserror::Error;

const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// A numeric datetime conversion or arithmetic operation exceeded hifitime's
/// representable duration range.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub(super) enum DatetimeRangeError {
    #[error("{context} must be finite, got {value}")]
    NonFinite { context: &'static str, value: f64 },
    #[error("{context} is outside the representable datetime range")]
    OutOfRange { context: &'static str },
}

/// The duration of `nanoseconds`, when hifitime can represent it.
fn checked_duration_from_nanoseconds(
    nanoseconds: i128,
    context: &'static str,
) -> Result<hifitime::Duration, DatetimeRangeError> {
    let range =
        hifitime::Duration::MIN.total_nanoseconds()..=hifitime::Duration::MAX.total_nanoseconds();
    if range.contains(&nanoseconds) {
        Ok(hifitime::Duration::from_total_nanoseconds(nanoseconds))
    } else {
        Err(DatetimeRangeError::OutOfRange { context })
    }
}

/// The exact binary64 value `seconds`, rounded once to the nearest whole
/// nanosecond with ties to even.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "arbitrary-precision rationals cannot overflow; the i128 increment is checked"
)]
fn nanoseconds_from_seconds(
    seconds: f64,
    context: &'static str,
) -> Result<i128, DatetimeRangeError> {
    let exact = BigRational::from_float(seconds).ok_or(DatetimeRangeError::NonFinite {
        context,
        value: seconds,
    })? * BigRational::from_integer(NANOS_PER_SECOND.into());
    let floor = exact.floor();
    let below = floor
        .to_integer()
        .to_i128()
        .ok_or(DatetimeRangeError::OutOfRange { context })?;
    let half = BigRational::new(1.into(), 2.into());
    let round_up = match (exact - floor).cmp(&half) {
        Ordering::Less => false,
        Ordering::Greater => true,
        Ordering::Equal => below.rem_euclid(2) == 1,
    };
    if round_up {
        below
            .checked_add(1)
            .ok_or(DatetimeRangeError::OutOfRange { context })
    } else {
        Ok(below)
    }
}

/// `nanoseconds` as seconds, correctly rounded to binary64.
///
/// Up to 2^53 in magnitude both the count and 10^9 are exact binary64
/// values, so one IEEE division rounds correctly; beyond that the quotient is
/// rounded from the exact rational.
fn seconds_from_nanoseconds(
    nanoseconds: i128,
    context: &'static str,
) -> Result<f64, DatetimeRangeError> {
    const EXACT_BINARY64_INTEGER: u128 = 1 << 53;
    if nanoseconds.unsigned_abs() <= EXACT_BINARY64_INTEGER {
        #[expect(
            clippy::cast_precision_loss,
            reason = "an integer of magnitude at most 2^53 is exact in binary64"
        )]
        let exact = nanoseconds as f64;
        return Ok(exact / 1e9);
    }
    BigRational::new(nanoseconds.into(), NANOS_PER_SECOND.into())
        .to_f64()
        .filter(|seconds| seconds.is_finite())
        .ok_or(DatetimeRangeError::OutOfRange { context })
}

fn checked_duration_from_seconds(
    seconds: f64,
    context: &'static str,
) -> Result<hifitime::Duration, DatetimeRangeError> {
    checked_duration_from_nanoseconds(nanoseconds_from_seconds(seconds, context)?, context)
}

fn checked_epoch_with_delta(
    epoch: hifitime::Epoch,
    delta: hifitime::Duration,
    subtract: bool,
    context: &'static str,
) -> Result<hifitime::Epoch, DatetimeRangeError> {
    let epoch_ns = epoch.duration.total_nanoseconds();
    let delta_ns = delta.total_nanoseconds();
    let total_ns = if subtract {
        epoch_ns.checked_sub(delta_ns)
    } else {
        epoch_ns.checked_add(delta_ns)
    }
    .ok_or(DatetimeRangeError::OutOfRange { context })?;
    Ok(hifitime::Epoch::from_duration(
        checked_duration_from_nanoseconds(total_ns, context)?,
        epoch.time_scale,
    ))
}

pub(super) fn checked_epoch_add_seconds(
    epoch: hifitime::Epoch,
    seconds: f64,
) -> Result<hifitime::Epoch, DatetimeRangeError> {
    checked_epoch_with_delta(
        epoch,
        checked_duration_from_seconds(seconds, "datetime duration")?,
        false,
        "datetime addition",
    )
}

pub(super) fn checked_epoch_subtract_seconds(
    epoch: hifitime::Epoch,
    seconds: f64,
) -> Result<hifitime::Epoch, DatetimeRangeError> {
    checked_epoch_with_delta(
        epoch,
        checked_duration_from_seconds(seconds, "datetime duration")?,
        true,
        "datetime subtraction",
    )
}

pub(super) fn checked_epoch_difference_seconds(
    lhs: hifitime::Epoch,
    rhs: hifitime::Epoch,
) -> Result<f64, DatetimeRangeError> {
    let rhs = rhs.to_time_scale(lhs.time_scale);
    let difference_ns = lhs
        .duration
        .total_nanoseconds()
        .checked_sub(rhs.duration.total_nanoseconds())
        .ok_or(DatetimeRangeError::OutOfRange {
            context: "datetime difference",
        })?;
    checked_duration_from_nanoseconds(difference_ns, "datetime difference")?;
    seconds_from_nanoseconds(difference_ns, "datetime difference")
}

/// The Unix-time reading of the UTC datetime `epoch`, correctly rounded.
pub(super) fn unix_seconds(epoch: hifitime::Epoch) -> Result<f64, DatetimeRangeError> {
    seconds_from_nanoseconds(
        epoch.to_unix_duration().total_nanoseconds(),
        "Unix timestamp",
    )
}

/// The UTC datetime whose Unix-time reading is `seconds`.
pub(super) fn checked_epoch_from_unix_seconds(
    seconds: f64,
) -> Result<hifitime::Epoch, DatetimeRangeError> {
    checked_epoch_with_delta(
        hifitime::Epoch::from_unix_seconds(0.0),
        checked_duration_from_seconds(seconds, "Unix timestamp")?,
        false,
        "Unix timestamp conversion",
    )
}

/// Invalid Gregorian components returned by the datetime backend.
#[derive(Debug, Error)]
pub(super) enum GregorianFieldsError {
    #[error("hifitime returned invalid Gregorian month {0}")]
    Month(u8),
    #[error("hifitime returned invalid Gregorian day {day} for {year}-{month:02}")]
    Day { year: i32, month: u8, day: u8 },
    #[error("hifitime returned invalid Gregorian hour {0}")]
    Hour(u8),
    #[error("hifitime returned invalid Gregorian minute {0}")]
    Minute(u8),
    #[error("hifitime returned invalid Gregorian second {0}")]
    Second(u8),
    #[error("hifitime returned invalid Gregorian nanosecond {0}")]
    Nanosecond(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GregorianMonth {
    January,
    February,
    March,
    April,
    May,
    June,
    July,
    August,
    September,
    October,
    November,
    December,
}

impl GregorianMonth {
    const fn number(self) -> u8 {
        match self {
            Self::January => 1,
            Self::February => 2,
            Self::March => 3,
            Self::April => 4,
            Self::May => 5,
            Self::June => 6,
            Self::July => 7,
            Self::August => 8,
            Self::September => 9,
            Self::October => 10,
            Self::November => 11,
            Self::December => 12,
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Gregorian month offsets plus at most one leap day fit in u16"
    )]
    fn days_before(self, leap_year: bool) -> u16 {
        match self {
            Self::January => 0,
            Self::February => 31,
            Self::March => 59 + u16::from(leap_year),
            Self::April => 90 + u16::from(leap_year),
            Self::May => 120 + u16::from(leap_year),
            Self::June => 151 + u16::from(leap_year),
            Self::July => 181 + u16::from(leap_year),
            Self::August => 212 + u16::from(leap_year),
            Self::September => 243 + u16::from(leap_year),
            Self::October => 273 + u16::from(leap_year),
            Self::November => 304 + u16::from(leap_year),
            Self::December => 334 + u16::from(leap_year),
        }
    }

    const fn days_in(self, leap_year: bool) -> u8 {
        match self {
            Self::February if leap_year => 29,
            Self::February => 28,
            Self::April | Self::June | Self::September | Self::November => 30,
            Self::January
            | Self::March
            | Self::May
            | Self::July
            | Self::August
            | Self::October
            | Self::December => 31,
        }
    }
}

impl TryFrom<u8> for GregorianMonth {
    type Error = GregorianFieldsError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::January),
            2 => Ok(Self::February),
            3 => Ok(Self::March),
            4 => Ok(Self::April),
            5 => Ok(Self::May),
            6 => Ok(Self::June),
            7 => Ok(Self::July),
            8 => Ok(Self::August),
            9 => Ok(Self::September),
            10 => Ok(Self::October),
            11 => Ok(Self::November),
            12 => Ok(Self::December),
            invalid => Err(GregorianFieldsError::Month(invalid)),
        }
    }
}

/// Gregorian fields in an epoch's own declared time scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GregorianFields {
    year: i32,
    month: GregorianMonth,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl GregorianFields {
    /// Read and validate calendar components in `epoch.time_scale`.
    pub(super) fn from_epoch(epoch: hifitime::Epoch) -> Result<Self, GregorianFieldsError> {
        let (year, month, day, hour, minute, second, nanosecond) =
            epoch.to_gregorian(epoch.time_scale);
        let month = GregorianMonth::try_from(month)?;
        let leap_year = is_gregorian_leap_year(year);
        if day == 0 || day > month.days_in(leap_year) {
            return Err(GregorianFieldsError::Day {
                year,
                month: month.number(),
                day,
            });
        }
        if hour > 23 {
            return Err(GregorianFieldsError::Hour(hour));
        }
        if minute > 59 {
            return Err(GregorianFieldsError::Minute(minute));
        }
        if second > 60 {
            return Err(GregorianFieldsError::Second(second));
        }
        if nanosecond >= 1_000_000_000 {
            return Err(GregorianFieldsError::Nanosecond(nanosecond));
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }

    pub(super) fn year(self) -> i64 {
        i64::from(self.year)
    }

    pub(super) fn month(self) -> i64 {
        i64::from(self.month.number())
    }

    pub(super) fn day(self) -> i64 {
        i64::from(self.day)
    }

    pub(super) fn hour(self) -> i64 {
        i64::from(self.hour)
    }

    pub(super) fn minute(self) -> i64 {
        i64::from(self.minute)
    }

    pub(super) fn second(self) -> i64 {
        i64::from(self.second)
    }

    /// ISO 8601 weekday number: Monday = 1 through Sunday = 7.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "i32 years and validated month/day fields keep the Gregorian calendar calculation within i64"
    )]
    pub(super) fn iso_weekday(self) -> i64 {
        // Convert the proleptic Gregorian date to a day offset from
        // 1970-01-01 (Thursday), then shift to ISO's Monday-one numbering.
        let month = i64::from(self.month.number());
        let year = i64::from(self.year) - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted_month = if month > 2 { month - 3 } else { month + 9 };
        let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days_since_unix_epoch = era * 146_097 + day_of_era - 719_468;
        (days_since_unix_epoch + 3).rem_euclid(7) + 1
    }

    /// One-based day of year in the same declared time scale.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "validated Gregorian fields produce a day in 1..=366"
    )]
    pub(super) fn day_of_year(self) -> i64 {
        i64::from(self.month.days_before(is_gregorian_leap_year(self.year))) + i64::from(self.day)
    }
}

const fn is_gregorian_leap_year(year: i32) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// Convert a timezone-based datetime resolved during semantic lowering to UTC.
#[must_use]
pub(super) fn datetime_from_zoned(datetime: &ZonedDateTimeLiteral) -> hifitime::Epoch {
    epoch_from_timestamp(datetime.timestamp())
}

/// Convert an already parsed offset-bearing datetime literal to UTC.
#[must_use]
pub(super) fn datetime_from_offset(datetime: OffsetDateTimeLiteral) -> hifitime::Epoch {
    epoch_from_timestamp(datetime.timestamp())
}

fn epoch_from_timestamp(timestamp: jiff::Timestamp) -> hifitime::Epoch {
    hifitime::Epoch::from_unix_duration(hifitime::Duration::from_total_nanoseconds(
        timestamp.as_nanosecond(),
    ))
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::semantic::time_scale::TimeScale;

    use crate::eval_expr::datetime::{
        GregorianFields, checked_epoch_add_seconds, checked_epoch_difference_seconds,
        checked_epoch_from_unix_seconds, checked_epoch_subtract_seconds, nanoseconds_from_seconds,
        seconds_from_nanoseconds, unix_seconds,
    };

    fn tai_nanoseconds(nanoseconds: i128) -> hifitime::Epoch {
        hifitime::Epoch::from_duration(
            hifitime::Duration::from_total_nanoseconds(nanoseconds),
            hifitime::TimeScale::TAI,
        )
    }

    #[test]
    fn differences_are_correctly_rounded_and_antisymmetric() {
        // Far from the reference epoch, where f64 seconds of each instant
        // cannot resolve a nanosecond.
        let base = 3_800_000_000 * 1_000_000_000_i128;
        let a = tai_nanoseconds(base);
        let one_ns_later = tai_nanoseconds(base + 1);
        assert_eq!(
            checked_epoch_difference_seconds(one_ns_later, a).unwrap(),
            1e-9
        );
        assert_eq!(
            checked_epoch_difference_seconds(a, one_ns_later).unwrap(),
            -1e-9
        );
        let b = tai_nanoseconds(base + 300_000_000);
        assert_eq!(checked_epoch_difference_seconds(b, a).unwrap(), 0.3);
        assert_eq!(checked_epoch_difference_seconds(a, b).unwrap(), -0.3);
    }

    #[test]
    fn nanosecond_counts_convert_to_correctly_rounded_seconds() {
        let limit = 1_i128 << 53;
        for nanoseconds in [
            0,
            1,
            -1,
            999_999_999,
            limit,
            -limit,
            limit + 1,
            i128::from(i64::MAX),
        ] {
            let expected = num_rational::BigRational::new(nanoseconds.into(), 1_000_000_000.into());
            let expected = num_traits::ToPrimitive::to_f64(&expected).unwrap();
            assert_eq!(
                seconds_from_nanoseconds(nanoseconds, "test")
                    .unwrap()
                    .to_bits(),
                expected.to_bits(),
                "{nanoseconds}"
            );
        }
    }

    #[test]
    fn seconds_round_to_the_nearest_nanosecond_with_ties_to_even() {
        assert_eq!(nanoseconds_from_seconds(2.9e-9, "test").unwrap(), 3);
        assert_eq!(nanoseconds_from_seconds(-2.9e-9, "test").unwrap(), -3);
        assert_eq!(nanoseconds_from_seconds(0.3, "test").unwrap(), 300_000_000);
        // 2^-10 s is exactly 976562.5 ns; 3 * 2^-10 s is 2929687.5 ns.
        assert_eq!(
            nanoseconds_from_seconds(2.0_f64.powi(-10), "test").unwrap(),
            976_562
        );
        assert_eq!(
            nanoseconds_from_seconds(3.0 * 2.0_f64.powi(-10), "test").unwrap(),
            2_929_688
        );
        assert_eq!(
            nanoseconds_from_seconds(-(2.0_f64.powi(-10)), "test").unwrap(),
            -976_562
        );
        assert!(nanoseconds_from_seconds(f64::NAN, "test").is_err());
        assert!(nanoseconds_from_seconds(1e300, "test").is_err());
    }

    #[test]
    fn unix_seconds_resolve_a_nanosecond_before_the_epoch() {
        let epoch = checked_epoch_from_unix_seconds(-1e-9).unwrap();
        assert_eq!(unix_seconds(epoch).unwrap(), -1e-9);
        let shifted =
            checked_epoch_add_seconds(checked_epoch_from_unix_seconds(0.0).unwrap(), 2.9e-9)
                .unwrap();
        assert_eq!(shifted.to_unix_duration().total_nanoseconds(), 3);
    }

    #[test]
    fn every_supported_scale_extracts_its_own_calendar_coordinate() {
        for scale in TimeScale::ALL {
            let epoch =
                hifitime::Epoch::maybe_from_gregorian(2024, 1, 1, 0, 0, 0, 0, scale.to_hifitime())
                    .unwrap();
            let fields = GregorianFields::from_epoch(epoch).unwrap();
            assert_eq!(fields.year(), 2024, "{scale}");
            assert_eq!(fields.month(), 1, "{scale}");
            assert_eq!(fields.day(), 1, "{scale}");
            assert_eq!(fields.hour(), 0, "{scale}");
            assert_eq!(fields.minute(), 0, "{scale}");
            assert_eq!(fields.second(), 0, "{scale}");
            assert_eq!(fields.iso_weekday(), 1, "{scale}");
            assert_eq!(fields.day_of_year(), 1, "{scale}");
        }
    }

    #[test]
    fn datetime_arithmetic_accepts_boundaries_and_rejects_saturation() {
        let one_second_ns = 1_000_000_000_i128;
        let near_max = hifitime::Epoch::from_duration(
            hifitime::Duration::from_total_nanoseconds(
                hifitime::Duration::MAX
                    .total_nanoseconds()
                    .checked_sub(one_second_ns)
                    .unwrap(),
            ),
            hifitime::TimeScale::UTC,
        );
        let near_min = hifitime::Epoch::from_duration(
            hifitime::Duration::from_total_nanoseconds(
                hifitime::Duration::MIN
                    .total_nanoseconds()
                    .checked_add(one_second_ns)
                    .unwrap(),
            ),
            hifitime::TimeScale::UTC,
        );

        assert_eq!(
            checked_epoch_add_seconds(near_max, 1.0).unwrap().duration,
            hifitime::Duration::MAX
        );
        assert!(checked_epoch_add_seconds(near_max, 2.0).is_err());
        assert_eq!(
            checked_epoch_subtract_seconds(near_min, 1.0)
                .unwrap()
                .duration,
            hifitime::Duration::MIN
        );
        assert!(checked_epoch_subtract_seconds(near_min, 2.0).is_err());
        assert!(
            checked_epoch_difference_seconds(
                hifitime::Epoch::from_duration(hifitime::Duration::MAX, hifitime::TimeScale::UTC,),
                hifitime::Epoch::from_duration(hifitime::Duration::MIN, hifitime::TimeScale::UTC,),
            )
            .is_err()
        );
    }

    #[test]
    fn unix_time_constructor_rejects_saturating_inputs() {
        assert!(checked_epoch_from_unix_seconds(1.0e300).is_err());
        assert!(checked_epoch_from_unix_seconds(-1.0e300).is_err());
        assert!(checked_epoch_from_unix_seconds(f64::NAN).is_err());
        assert!(checked_epoch_from_unix_seconds(0.0).is_ok());
    }

    #[test]
    fn weekday_and_day_of_year_follow_iso_gregorian_rules() {
        let cases = [
            ((2024, 11, 5), 2, 310),
            ((2023, 12, 31), 7, 365),
            ((2024, 12, 31), 2, 366),
            ((2000, 2, 29), 2, 60),
            ((1900, 3, 1), 4, 60),
        ];
        for ((year, month, day), weekday, day_of_year) in cases {
            let epoch = hifitime::Epoch::maybe_from_gregorian(
                year,
                month,
                day,
                12,
                0,
                0,
                0,
                hifitime::TimeScale::UTC,
            )
            .unwrap();
            let fields = GregorianFields::from_epoch(epoch).unwrap();
            assert_eq!(fields.iso_weekday(), weekday);
            assert_eq!(fields.day_of_year(), day_of_year);
        }
    }
}
