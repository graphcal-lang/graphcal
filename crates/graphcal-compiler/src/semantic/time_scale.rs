//! Time scale definitions for the `Datetime` primitive type.
//!
//! Maps Graphcal's `TimeScale` enum to supported `hifitime::TimeScale` variants.
//! UTC is the default for civil use; aerospace users opt into TAI, TT, TDB, etc.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

macro_rules! define_time_scales {
    (@unit $_variant:ident) => { () };
    (@count $($variant:ident),+ $(,)?) => {
        <[()]>::len(&[$(define_time_scales!(@unit $variant)),+])
    };
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident => $name:literal
        ),+ $(,)?
    ) => {
        /// Time scales supported by Graphcal.
        ///
        /// Each variant maps to a supported [`hifitime::TimeScale`] variant.
        /// `UTC` is the default for civil datetime values.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum TimeScale {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl TimeScale {
            /// Every time scale supported by Graphcal.
            pub const ALL: [Self; define_time_scales!(@count $($variant),+)] = [
                $(Self::$variant),+
            ];

            /// All supported time scale names, for diagnostics and validation.
            pub(crate) const ALL_NAMES: &'static [&'static str] = &[$($name),+];

            /// Returns `true` if this is the default civil time scale (UTC).
            #[must_use]
            pub(crate) const fn is_utc(self) -> bool {
                matches!(self, Self::UTC)
            }

            /// Render the supported spellings for diagnostics.
            #[must_use]
            pub(crate) fn expected_names() -> String {
                Self::ALL_NAMES.join(", ")
            }

            /// Returns the string name of this time scale.
            #[must_use]
            const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        impl fmt::Display for TimeScale {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }

        impl FromStr for TimeScale {
            type Err = ParseTimeScaleError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($name => Ok(Self::$variant),)+
                    _ => Err(ParseTimeScaleError {
                        input: s.to_string(),
                    }),
                }
            }
        }
    };
}

define_time_scales! {
    /// Coordinated Universal Time — default for civil use.
    UTC => "UTC",
    /// International Atomic Time — continuous, the internal reference.
    TAI => "TAI",
    /// Terrestrial Time — TAI + 32.184 s, used in orbital mechanics.
    TT => "TT",
    /// Barycentric Dynamical Time — used for solar system ephemerides.
    TDB => "TDB",
    /// Ephemeris Time (NAIF/SPICE variant, ≈ TDB).
    ET => "ET",
    /// GPS Time — TAI − 19 s.
    GPST => "GPST",
    /// Galileo System Time.
    GST => "GST",
    /// `BeiDou` Time.
    BDT => "BDT",
    /// QZSS Time.
    QZSST => "QZSST",
}

impl TimeScale {
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
    ///
    /// # Errors
    ///
    /// Returns an error when `ts` is a `hifitime` time scale that Graphcal does
    /// not support.
    const fn from_hifitime(
        ts: hifitime::TimeScale,
    ) -> Result<Self, UnsupportedHifitimeTimeScaleError> {
        match ts {
            hifitime::TimeScale::UTC => Ok(Self::UTC),
            hifitime::TimeScale::TAI => Ok(Self::TAI),
            hifitime::TimeScale::TT => Ok(Self::TT),
            hifitime::TimeScale::TDB => Ok(Self::TDB),
            hifitime::TimeScale::ET => Ok(Self::ET),
            hifitime::TimeScale::GPST => Ok(Self::GPST),
            hifitime::TimeScale::GST => Ok(Self::GST),
            hifitime::TimeScale::BDT => Ok(Self::BDT),
            hifitime::TimeScale::QZSST => Ok(Self::QZSST),
            _ => Err(UnsupportedHifitimeTimeScaleError(ts)),
        }
    }
}

/// Error returned when parsing an unknown Graphcal time scale name from text.
///
/// This is a source/user-input boundary error: the input string is not one of
/// Graphcal's supported time scale names.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown time scale `{input}`; expected one of: {}", TimeScale::ALL_NAMES.join(", "))]
pub struct ParseTimeScaleError {
    /// The unrecognized input string.
    input: String,
}

/// Error returned when converting an unsupported `hifitime` time scale.
///
/// This is an external-library boundary error: `hifitime` recognized the value
/// as a valid time scale, but Graphcal does not support that time scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unsupported hifitime time scale `{0:?}`")]
pub struct UnsupportedHifitimeTimeScaleError(hifitime::TimeScale);

impl TryFrom<hifitime::TimeScale> for TimeScale {
    type Error = UnsupportedHifitimeTimeScaleError;

    fn try_from(ts: hifitime::TimeScale) -> Result<Self, Self::Error> {
        Self::from_hifitime(ts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_roundtrip() {
        for scale in TimeScale::ALL {
            let spelling = scale.to_string();
            let parsed: TimeScale = spelling.parse().unwrap();
            assert_eq!(scale, parsed);
        }
        assert_eq!(
            TimeScale::ALL.map(|scale| scale.to_string()).as_slice(),
            TimeScale::ALL_NAMES
        );
    }

    #[test]
    fn from_str_unknown() {
        let err = "INVALID".parse::<TimeScale>().unwrap_err();
        assert_eq!(err.input, "INVALID");
        assert!(err.to_string().contains("unknown time scale"));
    }

    #[test]
    fn hifitime_roundtrip() {
        for scale in TimeScale::ALL {
            let hifitime = scale.to_hifitime();
            let back = TimeScale::from_hifitime(hifitime).unwrap();
            assert_eq!(scale, back);
        }
    }

    #[test]
    fn unsupported_hifitime_scale_is_error() {
        let err = TimeScale::from_hifitime(hifitime::TimeScale::TCG).unwrap_err();
        assert_eq!(err.0, hifitime::TimeScale::TCG);
    }

    #[test]
    fn utc_is_default() {
        assert!(TimeScale::UTC.is_utc());
        assert!(!TimeScale::TT.is_utc());
    }
}
