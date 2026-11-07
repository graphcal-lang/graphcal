//! Re-expressing an instant in another time scale.
//!
//! hifitime looks its leap-second table up with an instant's TAI reading,
//! but the table is keyed by UTC readings. Converting into UTC therefore
//! switches to a new TAI−UTC offset ΔAT seconds too early, which shifts the
//! UTC reading of the ΔAT seconds before each leap second by one second.
//! Conversions into UTC are corrected here. Every other conversion,
//! including every conversion out of UTC, is hifitime's own, which reads the
//! table with the UTC reading it is keyed by.

use graphcal_compiler::semantic::time_scale::TimeScale;

const ONE_SECOND: hifitime::Duration = hifitime::Duration::from_parts(0, 1_000_000_000);

/// `epoch` re-expressed in `scale`.
#[must_use]
pub fn to_time_scale(epoch: hifitime::Epoch, scale: TimeScale) -> hifitime::Epoch {
    if epoch.time_scale == scale.to_hifitime() {
        return epoch;
    }
    match scale {
        TimeScale::UTC => utc_from_tai(epoch.to_tai_duration()),
        TimeScale::TAI
        | TimeScale::TT
        | TimeScale::TDB
        | TimeScale::ET
        | TimeScale::GPST
        | TimeScale::GST
        | TimeScale::BDT
        | TimeScale::QZSST => epoch.to_time_scale(scale.to_hifitime()),
    }
}

/// The UTC reading of the instant `tai` after J1900 in TAI.
///
/// hifitime's own reading is either right or, during the ΔAT seconds before
/// a leap second, one second early; the right reading is the candidate that
/// converts back to `tai`. An instant inside an inserted leap second has no
/// UTC reading (`23:59:60` is not representable), and neither candidate
/// converts back; it keeps hifitime's reading, which repeats `23:59:59` of
/// the day the leap second belongs to.
fn utc_from_tai(tai: hifitime::Duration) -> hifitime::Epoch {
    let early = hifitime::Epoch::from_tai_duration(tai).to_time_scale(hifitime::TimeScale::UTC);
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "hifitime durations saturate; a saturated candidate never converts back to `tai`"
    )]
    let late = hifitime::Epoch::from_duration(early.duration + ONE_SECOND, early.time_scale);
    [early, late]
        .into_iter()
        .find(|candidate| candidate.to_tai_duration() == tai)
        .unwrap_or(early)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tai(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> hifitime::Epoch {
        hifitime::Epoch::maybe_from_gregorian(
            year,
            month,
            day,
            hour,
            minute,
            second,
            0,
            hifitime::TimeScale::TAI,
        )
        .unwrap()
    }

    fn utc_reading(epoch: hifitime::Epoch) -> (i32, u8, u8, u8, u8, u8, u32) {
        let utc = to_time_scale(epoch, TimeScale::UTC);
        assert_eq!(utc.time_scale, hifitime::TimeScale::UTC);
        utc.to_gregorian(hifitime::TimeScale::UTC)
    }

    #[test]
    fn tai_before_a_leap_second_keeps_the_old_offset() {
        // TAI−UTC was 35 s until the leap second at 2015-06-30T23:59:60 UTC
        // (TAI 2015-07-01T00:00:35), and 36 s from UTC midnight (TAI 00:00:36).
        assert_eq!(
            utc_reading(tai(2015, 7, 1, 0, 0, 0)),
            (2015, 6, 30, 23, 59, 25, 0)
        );
        assert_eq!(
            utc_reading(tai(2015, 7, 1, 0, 0, 34)),
            (2015, 6, 30, 23, 59, 59, 0)
        );
        assert_eq!(
            utc_reading(tai(2015, 7, 1, 0, 0, 36)),
            (2015, 7, 1, 0, 0, 0, 0)
        );
        assert_eq!(
            utc_reading(tai(2017, 1, 1, 0, 0, 0)),
            (2016, 12, 31, 23, 59, 24, 0)
        );
        assert_eq!(
            utc_reading(tai(2017, 1, 1, 0, 0, 37)),
            (2017, 1, 1, 0, 0, 0, 0)
        );
    }

    #[test]
    fn an_inserted_leap_second_reads_as_the_repeated_last_second_of_its_day() {
        assert_eq!(
            utc_reading(tai(2015, 7, 1, 0, 0, 35)),
            (2015, 6, 30, 23, 59, 59, 0)
        );
        assert_eq!(
            utc_reading(tai(2017, 1, 1, 0, 0, 36)),
            (2016, 12, 31, 23, 59, 59, 0)
        );
    }

    #[test]
    fn every_utc_reading_round_trips_through_every_scale() {
        let around_leap_seconds = [
            hifitime::Epoch::from_gregorian_utc(1971, 12, 31, 23, 59, 55, 0),
            hifitime::Epoch::from_gregorian_utc(1972, 1, 1, 0, 0, 0, 0),
            hifitime::Epoch::from_gregorian_utc(2015, 6, 30, 23, 59, 59, 500_000_000),
            hifitime::Epoch::from_gregorian_utc(2015, 7, 1, 0, 0, 0, 0),
            hifitime::Epoch::from_gregorian_utc(2016, 12, 31, 23, 59, 30, 0),
            hifitime::Epoch::from_gregorian_utc(2017, 1, 1, 0, 0, 0, 1),
            hifitime::Epoch::from_gregorian_utc(2024, 11, 5, 12, 0, 0, 0),
        ];
        for utc in around_leap_seconds {
            for scale in TimeScale::ALL {
                let there = to_time_scale(utc, scale);
                assert_eq!(there.time_scale, scale.to_hifitime(), "{scale}");
                let back = to_time_scale(there, TimeScale::UTC);
                // TDB and ET go through floating-point series; allow their
                // sub-microsecond round-off, and require exactness elsewhere.
                let error = (back.duration - utc.duration).abs();
                let tolerance = match scale {
                    TimeScale::TDB | TimeScale::ET => hifitime::Duration::from_parts(0, 1_000),
                    _ => hifitime::Duration::ZERO,
                };
                assert!(error <= tolerance, "{utc} via {scale}: {back}");
            }
        }
    }
}
