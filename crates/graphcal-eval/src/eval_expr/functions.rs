use graphcal_compiler::datetime_literal::{CivilDateTimeLiteral, OffsetDateTimeLiteral};
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::time_zone::{IanaTimeZoneId, TimeZoneRegistry};
use thiserror::Error;

/// Failure to resolve an already validated civil datetime and timezone.
#[derive(Debug, Error)]
pub(super) enum DatetimeConstructionError {
    #[error("bundled timezone lookup invariant failed: {0}")]
    TimeZoneLookup(#[source] jiff::Error),
    #[error("failed to resolve civil datetime in `{time_zone}`: {source}")]
    CivilResolution {
        time_zone: IanaTimeZoneId,
        #[source]
        source: jiff::Error,
    },
}

/// Failure to transfer validated civil coordinates into hifitime.
#[derive(Debug, Error)]
pub(super) enum EpochConstructionError {
    #[error("parsed civil datetime component is out of range: {0}")]
    Component(#[from] std::num::TryFromIntError),
    #[error("hifitime rejected parsed civil datetime coordinates: {0}")]
    Hifitime(#[from] hifitime::HifitimeError),
}

/// Resolve a parsed local civil datetime in a validated IANA timezone and
/// return the corresponding UTC `hifitime::Epoch`.
pub(super) fn datetime_with_timezone(
    datetime: CivilDateTimeLiteral,
    time_zone_id: &IanaTimeZoneId,
    time_zones: &TimeZoneRegistry,
) -> Result<hifitime::Epoch, DatetimeConstructionError> {
    let time_zone = time_zones
        .get(time_zone_id)
        .map_err(DatetimeConstructionError::TimeZoneLookup)?;
    let zoned = time_zone.to_zoned(datetime.datetime()).map_err(|source| {
        DatetimeConstructionError::CivilResolution {
            time_zone: time_zone_id.clone(),
            source,
        }
    })?;
    Ok(epoch_from_timestamp(zoned.timestamp()))
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
