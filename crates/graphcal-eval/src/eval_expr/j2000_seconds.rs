//! Datetimes as seconds since J2000, read in one time scale.
//!
//! The number is what a model computes for `@t - <J2000 in scale>`: the
//! instant and J2000 (2000-01-01T12:00:00) are both read in `scale` and
//! subtracted with Graphcal's own datetime arithmetic.

use graphcal_compiler::semantic::time_scale::TimeScale;
use thiserror::Error;

use super::datetime::{
    DatetimeRangeError, checked_epoch_add_seconds, checked_epoch_difference_seconds,
};

/// A datetime that has no representation as seconds since J2000, or a
/// number of seconds that names no representable instant.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("{0}")]
pub struct J2000SecondsError(DatetimeRangeError);

fn j2000(scale: TimeScale) -> Result<hifitime::Epoch, J2000SecondsError> {
    hifitime::Epoch::maybe_from_gregorian(2000, 1, 1, 12, 0, 0, 0, scale.to_hifitime()).map_err(
        |_| {
            J2000SecondsError(DatetimeRangeError::OutOfRange {
                context: "the J2000 epoch",
            })
        },
    )
}

/// Seconds from J2000 to `epoch`, both read in `scale`.
///
/// # Errors
///
/// Returns [`J2000SecondsError`] when the difference is not representable.
pub fn seconds_since_j2000(
    epoch: hifitime::Epoch,
    scale: TimeScale,
) -> Result<f64, J2000SecondsError> {
    checked_epoch_difference_seconds(epoch.to_time_scale(scale.to_hifitime()), j2000(scale)?)
        .map_err(J2000SecondsError)
}

/// The instant `seconds` after J2000, read in `scale`.
///
/// # Errors
///
/// Returns [`J2000SecondsError`] when `seconds` is not finite or the instant
/// is outside the representable datetime range.
pub fn epoch_from_j2000_seconds(
    seconds: f64,
    scale: TimeScale,
) -> Result<hifitime::Epoch, J2000SecondsError> {
    checked_epoch_add_seconds(j2000(scale)?, seconds).map_err(J2000SecondsError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn j2000_is_zero_and_seconds_round_trip_in_every_scale() {
        for scale in TimeScale::ALL {
            let origin = j2000(scale).unwrap();
            assert_eq!(seconds_since_j2000(origin, scale).unwrap(), 0.0, "{scale}");
            let later = epoch_from_j2000_seconds(86_400.5, scale).unwrap();
            assert_eq!(
                seconds_since_j2000(later, scale).unwrap(),
                86_400.5,
                "{scale}"
            );
        }
        assert!(epoch_from_j2000_seconds(f64::NAN, TimeScale::TAI).is_err());
        assert!(epoch_from_j2000_seconds(1e300, TimeScale::TAI).is_err());
    }

    #[test]
    fn readings_differ_between_scales_by_their_offset() {
        // TT runs 32.184 s ahead of TAI, so TAI's J2000 reads 32.184 s after
        // TT's J2000.
        let tai_j2000_in_tt =
            seconds_since_j2000(j2000(TimeScale::TAI).unwrap(), TimeScale::TT).unwrap();
        assert!((tai_j2000_in_tt - 32.184).abs() < 1e-6, "{tai_j2000_in_tt}");
    }
}
