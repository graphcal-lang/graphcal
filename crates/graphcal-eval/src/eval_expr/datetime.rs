//! Datetime construction and declared-scale Gregorian field extraction.

use graphcal_compiler::datetime_literal::{
    CivilDateTimeLiteral, OffsetDateTimeLiteral, ZonedDateTimeLiteral,
};
use graphcal_compiler::registry::time_scale::TimeScale;
use thiserror::Error;

/// Failure to transfer validated civil coordinates into hifitime.
#[derive(Debug, Error)]
pub(super) enum EpochConstructionError {
    #[error("parsed civil datetime component is out of range: {0}")]
    Component(#[from] std::num::TryFromIntError),
    #[error("hifitime rejected parsed civil datetime coordinates: {0}")]
    Hifitime(#[from] hifitime::HifitimeError),
}

/// A numeric datetime conversion or arithmetic operation exceeded hifitime's
/// representable duration range.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub(super) enum DatetimeRangeError {
    #[error("{context} must be finite, got {value}")]
    NonFinite { context: &'static str, value: f64 },
    #[error("{context} is outside the representable datetime range")]
    OutOfRange { context: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumericEpochKind {
    JulianDate,
    ModifiedJulianDate,
    UnixSeconds,
}

fn checked_duration_from_seconds(
    seconds: f64,
    context: &'static str,
) -> Result<hifitime::Duration, DatetimeRangeError> {
    if !seconds.is_finite() {
        return Err(DatetimeRangeError::NonFinite {
            context,
            value: seconds,
        });
    }
    let min_seconds = hifitime::Duration::MIN.to_seconds();
    let max_seconds = hifitime::Duration::MAX.to_seconds();
    if seconds < min_seconds || seconds > max_seconds {
        return Err(DatetimeRangeError::OutOfRange { context });
    }
    Ok(hifitime::Duration::from_seconds(seconds))
}

fn checked_duration_from_days(
    days: f64,
    context: &'static str,
) -> Result<hifitime::Duration, DatetimeRangeError> {
    if !days.is_finite() {
        return Err(DatetimeRangeError::NonFinite {
            context,
            value: days,
        });
    }
    checked_duration_from_seconds(days * 86_400.0, context)
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
    let min_ns = hifitime::Duration::MIN.total_nanoseconds();
    let max_ns = hifitime::Duration::MAX.total_nanoseconds();
    if total_ns < min_ns || total_ns > max_ns {
        return Err(DatetimeRangeError::OutOfRange { context });
    }
    Ok(hifitime::Epoch::from_duration(
        hifitime::Duration::from_total_nanoseconds(total_ns),
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
    let min_ns = hifitime::Duration::MIN.total_nanoseconds();
    let max_ns = hifitime::Duration::MAX.total_nanoseconds();
    if difference_ns < min_ns || difference_ns > max_ns {
        return Err(DatetimeRangeError::OutOfRange {
            context: "datetime difference",
        });
    }
    Ok(hifitime::Duration::from_total_nanoseconds(difference_ns).to_seconds())
}

pub(super) fn checked_epoch_from_numeric(
    value: f64,
    kind: NumericEpochKind,
) -> Result<hifitime::Epoch, DatetimeRangeError> {
    let (base, delta, context) = match kind {
        NumericEpochKind::JulianDate => (
            hifitime::Epoch::from_jde_utc(0.0),
            checked_duration_from_days(value, "Julian date")?,
            "Julian date conversion",
        ),
        NumericEpochKind::ModifiedJulianDate => (
            hifitime::Epoch::from_mjd_utc(0.0),
            checked_duration_from_days(value, "modified Julian date")?,
            "modified Julian date conversion",
        ),
        NumericEpochKind::UnixSeconds => (
            hifitime::Epoch::from_unix_seconds(0.0),
            checked_duration_from_seconds(value, "Unix timestamp")?,
            "Unix timestamp conversion",
        ),
    };
    checked_epoch_with_delta(base, delta, false, context)
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

/// Interpret parsed Gregorian civil coordinates in an explicit scientific
/// time scale.
pub(super) fn epoch_from_civil_datetime(
    datetime: CivilDateTimeLiteral,
    scale: TimeScale,
) -> Result<hifitime::Epoch, EpochConstructionError> {
    epoch_from_civil(datetime.datetime(), scale)
}

fn epoch_from_timestamp(timestamp: jiff::Timestamp) -> hifitime::Epoch {
    hifitime::Epoch::from_unix_duration(hifitime::Duration::from_total_nanoseconds(
        timestamp.as_nanosecond(),
    ))
}

fn epoch_from_civil(
    datetime: jiff::civil::DateTime,
    scale: TimeScale,
) -> Result<hifitime::Epoch, EpochConstructionError> {
    Ok(hifitime::Epoch::maybe_from_gregorian(
        i32::from(datetime.year()),
        u8::try_from(datetime.month())?,
        u8::try_from(datetime.day())?,
        u8::try_from(datetime.hour())?,
        u8::try_from(datetime.minute())?,
        u8::try_from(datetime.second())?,
        u32::try_from(datetime.subsec_nanosecond())?,
        scale.to_hifitime(),
    )?)
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::registry::time_scale::TimeScale;

    use crate::eval_expr::datetime::{
        GregorianFields, NumericEpochKind, checked_epoch_add_seconds,
        checked_epoch_difference_seconds, checked_epoch_from_numeric,
        checked_epoch_subtract_seconds,
    };

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
    fn numeric_epoch_constructors_reject_saturating_inputs() {
        for kind in [
            NumericEpochKind::JulianDate,
            NumericEpochKind::ModifiedJulianDate,
            NumericEpochKind::UnixSeconds,
        ] {
            assert!(checked_epoch_from_numeric(1.0e300, kind).is_err());
            assert!(checked_epoch_from_numeric(-1.0e300, kind).is_err());
        }
        assert!(checked_epoch_from_numeric(0.0, NumericEpochKind::UnixSeconds).is_ok());
        assert!(checked_epoch_from_numeric(2_460_000.0, NumericEpochKind::JulianDate).is_ok());
        assert!(checked_epoch_from_numeric(60_000.0, NumericEpochKind::ModifiedJulianDate).is_ok());
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
