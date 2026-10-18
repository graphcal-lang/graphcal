//! Datetime instants on the wire.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A time scale an instant is read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeScale {
    /// Coordinated Universal Time.
    #[serde(rename = "UTC")]
    Utc,
    /// International Atomic Time.
    #[serde(rename = "TAI")]
    Tai,
    /// Terrestrial Time.
    #[serde(rename = "TT")]
    Tt,
    /// Barycentric Dynamical Time.
    #[serde(rename = "TDB")]
    Tdb,
    /// Ephemeris Time (NAIF/SPICE).
    #[serde(rename = "ET")]
    Et,
    /// GPS Time.
    #[serde(rename = "GPST")]
    Gpst,
    /// Galileo System Time.
    #[serde(rename = "GST")]
    Gst,
    /// `BeiDou` Time.
    #[serde(rename = "BDT")]
    Bdt,
    /// QZSS Time.
    #[serde(rename = "QZSST")]
    Qzsst,
}

impl TimeScale {
    /// The scale's standard abbreviation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utc => "UTC",
            Self::Tai => "TAI",
            Self::Tt => "TT",
            Self::Tdb => "TDB",
            Self::Et => "ET",
            Self::Gpst => "GPST",
            Self::Gst => "GST",
            Self::Bdt => "BDT",
            Self::Qzsst => "QZSST",
        }
    }
}

impl fmt::Display for TimeScale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A datetime instant in one time scale.
///
/// A value is the number of seconds from J2000 (2000-01-01T12:00:00) to the
/// instant, both read in `scale`: the difference a Graphcal model computes
/// for `@t - <J2000 in scale>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "DatetimeTypeRepr", into = "DatetimeTypeRepr")]
pub struct DatetimeType {
    scale: TimeScale,
}

impl DatetimeType {
    /// Instants read in `scale`.
    #[must_use]
    pub const fn new(scale: TimeScale) -> Self {
        Self { scale }
    }

    /// The time scale.
    #[must_use]
    pub const fn scale(self) -> TimeScale {
        self.scale
    }
}

/// The only epoch spelling: J2000. Any other spelling fails to deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Epoch {
    J2000,
}

/// The only value unit spelling: seconds. Any other spelling fails to
/// deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Seconds {
    #[serde(rename = "s")]
    S,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatetimeTypeRepr {
    scale: TimeScale,
    epoch: Epoch,
    unit: Seconds,
}

impl From<DatetimeTypeRepr> for DatetimeType {
    fn from(repr: DatetimeTypeRepr) -> Self {
        let DatetimeTypeRepr {
            scale,
            epoch: Epoch::J2000,
            unit: Seconds::S,
        } = repr;
        Self::new(scale)
    }
}

impl From<DatetimeType> for DatetimeTypeRepr {
    fn from(datetime: DatetimeType) -> Self {
        Self {
            scale: datetime.scale,
            epoch: Epoch::J2000,
            unit: Seconds::S,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_json_names_scale_epoch_and_unit() {
        let datetime = DatetimeType::new(TimeScale::Tai);
        let json = serde_json::to_string(&datetime).unwrap();
        assert_eq!(json, r#"{"scale":"TAI","epoch":"J2000","unit":"s"}"#);
        assert_eq!(
            serde_json::from_str::<DatetimeType>(&json).unwrap(),
            datetime
        );
        assert!(
            serde_json::from_str::<DatetimeType>(r#"{"scale":"TAI","epoch":"J1900","unit":"s"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<DatetimeType>(r#"{"scale":"TAI","epoch":"J2000","unit":"ms"}"#)
                .is_err()
        );
    }
}
