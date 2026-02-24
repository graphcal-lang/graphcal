//! Time scale definitions for the `Datetime` primitive type.
//!
//! Maps Graphcal's `TimeScale` enum to `hifitime::TimeScale`.
//! UTC is the default for civil use; aerospace users opt into TAI, TT, TDB, etc.

use std::fmt;
use std::str::FromStr;

/// Time scales supported by Graphcal.
///
/// Each variant maps 1:1 to a [`hifitime::TimeScale`] variant.
/// `UTC` is the default for civil datetime values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeScale {
    /// Coordinated Universal Time — default for civil use.
    UTC,
    /// International Atomic Time — continuous, the internal reference.
    TAI,
    /// Terrestrial Time — TAI + 32.184 s, used in orbital mechanics.
    TT,
    /// Barycentric Dynamical Time — used for solar system ephemerides.
    TDB,
    /// Ephemeris Time (NAIF/SPICE variant, ≈ TDB).
    ET,
    /// GPS Time — TAI − 19 s.
    GPST,
    /// Galileo System Time.
    GST,
    /// `BeiDou` Time.
    BDT,
    /// QZSS Time.
    QZSST,
}

impl TimeScale {
    /// All supported time scale names, for error messages and validation.
    pub const ALL_NAMES: &[&str] = &[
        "UTC", "TAI", "TT", "TDB", "ET", "GPST", "GST", "BDT", "QZSST",
    ];

    /// Returns `true` if this is the default civil time scale (UTC).
    #[must_use]
    pub const fn is_utc(self) -> bool {
        matches!(self, Self::UTC)
    }

    /// Convert to the corresponding `hifitime::TimeScale`.
    #[must_use]
    pub const fn to_hifitime(self) -> hifitime::TimeScale {
        match self {
            Self::UTC => hifitime::TimeScale::UTC,
            Self::TAI => hifitime::TimeScale::TAI,
            Self::TT => hifitime::TimeScale::TT,
            Self::TDB => hifitime::TimeScale::TDB,
            Self::ET => hifitime::TimeScale::ET,
            Self::GPST => hifitime::TimeScale::GPST,
            Self::GST => hifitime::TimeScale::GST,
            Self::BDT => hifitime::TimeScale::BDT,
            Self::QZSST => hifitime::TimeScale::QZSST,
        }
    }

    /// Convert from `hifitime::TimeScale`.
    #[must_use]
    pub const fn from_hifitime(ts: hifitime::TimeScale) -> Self {
        match ts {
            hifitime::TimeScale::TAI => Self::TAI,
            hifitime::TimeScale::TT => Self::TT,
            hifitime::TimeScale::TDB => Self::TDB,
            hifitime::TimeScale::ET => Self::ET,
            hifitime::TimeScale::GPST => Self::GPST,
            hifitime::TimeScale::GST => Self::GST,
            hifitime::TimeScale::BDT => Self::BDT,
            hifitime::TimeScale::QZSST => Self::QZSST,
            // hifitime::TimeScale is non_exhaustive; UTC and unknown variants map to UTC
            _ => Self::UTC,
        }
    }
}

impl fmt::Display for TimeScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UTC => write!(f, "UTC"),
            Self::TAI => write!(f, "TAI"),
            Self::TT => write!(f, "TT"),
            Self::TDB => write!(f, "TDB"),
            Self::ET => write!(f, "ET"),
            Self::GPST => write!(f, "GPST"),
            Self::GST => write!(f, "GST"),
            Self::BDT => write!(f, "BDT"),
            Self::QZSST => write!(f, "QZSST"),
        }
    }
}

/// Error returned when parsing an unknown time scale name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseTimeScaleError {
    /// The unrecognized input string.
    pub input: String,
}

impl fmt::Display for ParseTimeScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown time scale `{}`; expected one of: {}",
            self.input,
            TimeScale::ALL_NAMES.join(", ")
        )
    }
}

impl std::error::Error for ParseTimeScaleError {}

impl FromStr for TimeScale {
    type Err = ParseTimeScaleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "UTC" => Ok(Self::UTC),
            "TAI" => Ok(Self::TAI),
            "TT" => Ok(Self::TT),
            "TDB" => Ok(Self::TDB),
            "ET" => Ok(Self::ET),
            "GPST" => Ok(Self::GPST),
            "GST" => Ok(Self::GST),
            "BDT" => Ok(Self::BDT),
            "QZSST" => Ok(Self::QZSST),
            _ => Err(ParseTimeScaleError {
                input: s.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn display_roundtrip() {
        let scales = [
            TimeScale::UTC,
            TimeScale::TAI,
            TimeScale::TT,
            TimeScale::TDB,
            TimeScale::ET,
            TimeScale::GPST,
            TimeScale::GST,
            TimeScale::BDT,
            TimeScale::QZSST,
        ];
        for scale in &scales {
            let s = scale.to_string();
            let parsed: TimeScale = s.parse().unwrap();
            assert_eq!(*scale, parsed);
        }
    }

    #[test]
    fn from_str_unknown() {
        let err = "INVALID".parse::<TimeScale>().unwrap_err();
        assert_eq!(err.input, "INVALID");
        assert!(err.to_string().contains("unknown time scale"));
    }

    #[test]
    fn hifitime_roundtrip() {
        let scales = [
            TimeScale::UTC,
            TimeScale::TAI,
            TimeScale::TT,
            TimeScale::TDB,
            TimeScale::ET,
            TimeScale::GPST,
            TimeScale::GST,
            TimeScale::BDT,
            TimeScale::QZSST,
        ];
        for scale in &scales {
            let hf = scale.to_hifitime();
            let back = TimeScale::from_hifitime(hf);
            assert_eq!(*scale, back);
        }
    }

    #[test]
    fn utc_is_default() {
        assert!(TimeScale::UTC.is_utc());
        assert!(!TimeScale::TT.is_utc());
    }
}
