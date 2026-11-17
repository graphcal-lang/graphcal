//! Parsed datetime literal forms at the source-to-semantic boundary.
//!
//! Graphcal's constructors intentionally accept different literal categories:
//! an offset-bearing instant for one-argument `datetime`, and a scale/offset-
//! free civil coordinate for timezone-based `datetime` and `epoch<S>`. Keeping
//! those forms distinct makes contradictory interpretation sources impossible
//! after HIR lowering.

use crate::semantic::time_scale::TimeScale;
use crate::semantic::time_zone::{IanaTimeZoneId, TimeZoneRegistry};
use thiserror::Error;

/// Error returned when parsing an offset-bearing datetime literal.
#[derive(Debug, Clone, Error)]
pub enum ParseOffsetDateTimeLiteralError {
    #[error(transparent)]
    Invalid(#[from] jiff::Error),
    #[error(
        "expected RFC 3339 syntax `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` with components in range"
    )]
    InvalidRfc3339,
    #[error("an explicit `Z` or numeric offset is required")]
    MissingOffset,
    #[error("a named timezone annotation is not allowed when the offset determines the instant")]
    TimeZoneAnnotation,
    #[error("{LEAP_SECOND_MESSAGE}")]
    LeapSecond,
}

/// Why a seconds field of 60 is rejected rather than clamped to 59.
const LEAP_SECOND_MESSAGE: &str = "seconds = 60 (a leap second) is not supported: a Graphcal datetime cannot represent a leap-second instant";

/// Why source text is not an RFC 3339 `full-date "T" partial-time` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateTimePrefixError {
    /// The text does not start with `YYYY-MM-DDTHH:MM:SS[.fraction]` with
    /// every component in range.
    Syntax,
    /// The prefix is well formed except that its seconds field is `60`.
    ///
    /// jiff silently clamps a leap second to `:59`, so the parsed value cannot
    /// tell the two apart; the source spelling is the only witness.
    LeapSecond,
}

/// Split off the RFC 3339 `full-date "T" partial-time` prefix
/// (`YYYY-MM-DDTHH:MM:SS[.fraction]`) that both datetime literal kinds share,
/// returning the bytes that follow it.
fn split_date_time_prefix(source: &str) -> Result<&[u8], DateTimePrefixError> {
    let [
        y0,
        y1,
        y2,
        y3,
        b'-',
        mo0,
        mo1,
        b'-',
        d0,
        d1,
        b'T' | b't',
        h0,
        h1,
        b':',
        mi0,
        mi1,
        b':',
        s0,
        s1,
        rest @ ..,
    ] = source.as_bytes()
    else {
        return Err(DateTimePrefixError::Syntax);
    };
    if ![y0, y1, y2, y3].into_iter().all(u8::is_ascii_digit)
        || !two_ascii_digits_in_range([*mo0, *mo1], 1, 12)
        || !two_ascii_digits_in_range([*d0, *d1], 1, 31)
        || !two_ascii_digits_in_range([*h0, *h1], 0, 23)
        || !two_ascii_digits_in_range([*mi0, *mi1], 0, 59)
    {
        return Err(DateTimePrefixError::Syntax);
    }
    match [*s0, *s1] {
        [b'6', b'0'] => return Err(DateTimePrefixError::LeapSecond),
        seconds if !two_ascii_digits_in_range(seconds, 0, 59) => {
            return Err(DateTimePrefixError::Syntax);
        }
        _ => {}
    }
    match rest {
        [b'.', fraction @ ..] => {
            let fraction_len = fraction
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            match fraction_len {
                0 => Err(DateTimePrefixError::Syntax),
                _ => Ok(&fraction[fraction_len..]),
            }
        }
        _ => Ok(rest),
    }
}

/// Require `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` exactly.
fn validate_rfc3339_syntax(source: &str) -> Result<(), ParseOffsetDateTimeLiteralError> {
    let offset = split_date_time_prefix(source).map_err(|error| match error {
        DateTimePrefixError::Syntax => ParseOffsetDateTimeLiteralError::InvalidRfc3339,
        DateTimePrefixError::LeapSecond => ParseOffsetDateTimeLiteralError::LeapSecond,
    })?;
    match offset {
        [b'Z' | b'z'] => Ok(()),
        [sign, hour_tens, hour_ones, b':', minute_tens, minute_ones]
            if matches!(sign, b'+' | b'-')
                && two_ascii_digits_in_range([*hour_tens, *hour_ones], 0, 23)
                && two_ascii_digits_in_range([*minute_tens, *minute_ones], 0, 59) =>
        {
            Ok(())
        }
        _ => Err(ParseOffsetDateTimeLiteralError::InvalidRfc3339),
    }
}

/// Require `YYYY-MM-DDTHH:MM:SS[.fraction]` exactly: the offset form without
/// its offset.
fn validate_civil_syntax(source: &str) -> Result<(), ParseCivilDateTimeLiteralError> {
    match split_date_time_prefix(source) {
        Ok([]) => Ok(()),
        Ok([_, ..]) | Err(DateTimePrefixError::Syntax) => {
            Err(ParseCivilDateTimeLiteralError::InvalidSyntax)
        }
        Err(DateTimePrefixError::LeapSecond) => Err(ParseCivilDateTimeLiteralError::LeapSecond),
    }
}

fn two_ascii_digits_in_range(digits: [u8; 2], min: u8, max: u8) -> bool {
    digits.iter().all(u8::is_ascii_digit) && {
        let value = (digits[0] - b'0') * 10 + (digits[1] - b'0');
        (min..=max).contains(&value)
    }
}

/// Error returned when parsing an offset/zone-free civil datetime literal.
#[derive(Debug, Clone, Error)]
pub enum ParseCivilDateTimeLiteralError {
    #[error(transparent)]
    Invalid(#[from] jiff::Error),
    #[error("an offset is not allowed in a local civil datetime literal")]
    Offset,
    #[error("an explicit local time is required; write `T00:00:00` for midnight")]
    MissingTime,
    #[error("a timezone annotation is not allowed in a local civil datetime literal")]
    TimeZoneAnnotation,
    #[error("{LEAP_SECOND_MESSAGE}")]
    LeapSecond,
    #[error(
        "expected civil datetime syntax `YYYY-MM-DDTHH:MM:SS[.fraction]` with components in range and nothing before or after it"
    )]
    InvalidSyntax,
}

/// Error returned when resolving local civil coordinates in an IANA timezone.
#[derive(Debug, Clone, Error)]
pub enum ResolveZonedDateTimeLiteralError {
    #[error("validated timezone `{time_zone}` could not be loaded: {source}")]
    TimeZoneRegistryInvariant {
        time_zone: IanaTimeZoneId,
        #[source]
        source: jiff::Error,
    },
    #[error(
        "local civil datetime `{datetime}` does not exist in timezone `{time_zone}` because its offset jumps from {before} to {after}"
    )]
    Nonexistent {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
    },
    #[error(
        "local civil datetime `{datetime}` occurs twice in timezone `{time_zone}` with offsets {before} and {after}"
    )]
    Repeated {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
    },
    #[error(
        "local civil datetime `{datetime}` in timezone `{time_zone}` is out of range: {source}"
    )]
    OutOfRange {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        #[source]
        source: jiff::Error,
    },
}

/// An RFC 3339 datetime literal with an explicit `Z` or numeric UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OffsetDateTimeLiteral(jiff::Timestamp);

impl OffsetDateTimeLiteral {
    /// Parse an offset-bearing datetime literal.
    ///
    /// # Errors
    ///
    /// Returns an error if `source` is invalid, does not carry an explicit
    /// offset, or also carries a named timezone annotation.
    pub fn parse(source: &str) -> Result<Self, ParseOffsetDateTimeLiteralError> {
        let parser = jiff::fmt::temporal::DateTimeParser::new();
        let pieces = parser.parse_pieces(source)?;
        if pieces.offset().is_none() {
            return Err(ParseOffsetDateTimeLiteralError::MissingOffset);
        }
        if pieces.time_zone_annotation().is_some() {
            return Err(ParseOffsetDateTimeLiteralError::TimeZoneAnnotation);
        }
        validate_rfc3339_syntax(source)?;
        parser.parse_timestamp(source).map(Self).map_err(Into::into)
    }

    /// The parsed instant represented by this literal.
    #[must_use]
    pub const fn timestamp(self) -> jiff::Timestamp {
        self.0
    }
}

/// A local Gregorian civil datetime with no offset, timezone, or time-scale
/// suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CivilDateTimeLiteral(jiff::civil::DateTime);

impl CivilDateTimeLiteral {
    /// Parse a scale-free and offset-free civil datetime literal.
    ///
    /// # Errors
    ///
    /// Returns an error if `source` is invalid, carries an offset or named
    /// timezone annotation, has no time, or is not spelled exactly
    /// `YYYY-MM-DDTHH:MM:SS[.fraction]`.
    pub fn parse(source: &str) -> Result<Self, ParseCivilDateTimeLiteralError> {
        let parser = jiff::fmt::temporal::DateTimeParser::new();
        let pieces = parser.parse_pieces(source)?;
        if pieces.offset().is_some() {
            return Err(ParseCivilDateTimeLiteralError::Offset);
        }
        if pieces.time_zone_annotation().is_some() {
            return Err(ParseCivilDateTimeLiteralError::TimeZoneAnnotation);
        }
        let time = pieces
            .time()
            .ok_or(ParseCivilDateTimeLiteralError::MissingTime)?;
        validate_civil_syntax(source)?;
        Ok(Self(pieces.date().to_datetime(time)))
    }

    /// The parsed Gregorian civil coordinate represented by this literal.
    #[must_use]
    pub const fn datetime(self) -> jiff::civil::DateTime {
        self.0
    }
}

impl std::fmt::Display for CivilDateTimeLiteral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// An unambiguous instant resolved from local civil coordinates and a
/// validated IANA timezone during semantic lowering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ZonedDateTimeLiteral {
    timestamp: jiff::Timestamp,
    time_zone: IanaTimeZoneId,
}

impl ZonedDateTimeLiteral {
    /// Resolve local civil coordinates using Graphcal's explicit timezone
    /// registry without silently choosing either side of a gap or fold.
    ///
    /// # Errors
    ///
    /// Returns a typed error for nonexistent or repeated local times, a
    /// registry invariant failure, or an instant outside Jiff's range.
    pub fn resolve(
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        time_zones: &TimeZoneRegistry,
    ) -> Result<Self, ResolveZonedDateTimeLiteralError> {
        let zone = time_zones.get(&time_zone).map_err(|source| {
            ResolveZonedDateTimeLiteralError::TimeZoneRegistryInvariant {
                time_zone: time_zone.clone(),
                source,
            }
        })?;
        let ambiguous = zone.to_ambiguous_timestamp(datetime.datetime());
        match ambiguous.offset() {
            jiff::tz::AmbiguousOffset::Unambiguous { .. } => ambiguous
                .unambiguous()
                .map(|timestamp| Self {
                    timestamp,
                    time_zone: time_zone.clone(),
                })
                .map_err(|source| ResolveZonedDateTimeLiteralError::OutOfRange {
                    datetime,
                    time_zone,
                    source,
                }),
            jiff::tz::AmbiguousOffset::Gap { before, after } => {
                Err(ResolveZonedDateTimeLiteralError::Nonexistent {
                    datetime,
                    time_zone,
                    before,
                    after,
                })
            }
            jiff::tz::AmbiguousOffset::Fold { before, after } => {
                Err(ResolveZonedDateTimeLiteralError::Repeated {
                    datetime,
                    time_zone,
                    before,
                    after,
                })
            }
        }
    }

    /// The unique instant represented by this resolved literal.
    #[must_use]
    pub const fn timestamp(&self) -> jiff::Timestamp {
        self.timestamp
    }

    /// The canonical timezone used to resolve the source civil coordinates.
    #[must_use]
    pub const fn time_zone(&self) -> &IanaTimeZoneId {
        &self.time_zone
    }
}

/// Error returned when civil coordinates have no instant in a time scale.
#[derive(Debug, Error)]
pub enum ResolveEpochLiteralError {
    #[error("civil datetime component is out of range: {0}")]
    Component(#[from] std::num::TryFromIntError),
    #[error("civil datetime has no instant in its time scale: {0}")]
    Hifitime(#[from] hifitime::HifitimeError),
}

/// An `epoch<S>("…")` literal: civil coordinates interpreted in an explicit
/// scientific time scale, resolved to its instant during HIR lowering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpochLiteral {
    civil: CivilDateTimeLiteral,
    scale: TimeScale,
    epoch: hifitime::Epoch,
}

impl EpochLiteral {
    /// Interpret the Gregorian civil coordinates `civil` in `scale`.
    ///
    /// # Errors
    ///
    /// Returns an error if the coordinates do not denote an instant that
    /// hifitime can represent in `scale`.
    pub fn resolve(
        civil: CivilDateTimeLiteral,
        scale: TimeScale,
    ) -> Result<Self, ResolveEpochLiteralError> {
        let datetime = civil.datetime();
        let epoch = hifitime::Epoch::maybe_from_gregorian(
            i32::from(datetime.year()),
            u8::try_from(datetime.month())?,
            u8::try_from(datetime.day())?,
            u8::try_from(datetime.hour())?,
            u8::try_from(datetime.minute())?,
            u8::try_from(datetime.second())?,
            u32::try_from(datetime.subsec_nanosecond())?,
            scale.to_hifitime(),
        )?;
        Ok(Self {
            civil,
            scale,
            epoch,
        })
    }

    /// The time scale the coordinates are interpreted in.
    #[must_use]
    pub const fn scale(self) -> TimeScale {
        self.scale
    }

    /// The instant the coordinates denote in the time scale.
    #[must_use]
    pub const fn epoch(self) -> hifitime::Epoch {
        self.epoch
    }
}

/// The constructor context that determines a datetime literal's required
/// interpretation shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeLiteralExpectation {
    /// `datetime("...")`: one instant with an explicit offset.
    OffsetDateTime,
    /// `datetime("...", timezone)`: one local civil coordinate.
    ZonedCivilDateTime,
    /// `epoch<S>("...")`: one scale-free civil coordinate interpreted in `S`.
    Epoch(TimeScale),
}

impl std::fmt::Display for DatetimeLiteralExpectation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OffsetDateTime => f.write_str(
                "one-argument datetime literal; expected RFC 3339 with `Z` or a numeric offset; use `epoch<S>(\"...\")` for scientific time-scale coordinates",
            ),
            Self::ZonedCivilDateTime => f.write_str(
                "timezone-based datetime literal; expected local civil time `YYYY-MM-DDTHH:MM:SS[.fraction]` without an offset, timezone annotation, or time scale",
            ),
            Self::Epoch(scale) => write!(
                f,
                "epoch<{scale}> literal; expected a scale-free civil time `YYYY-MM-DDTHH:MM:SS[.fraction]` without an offset, timezone, or time-scale suffix"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_literal_accepts_rfc3339_forms() {
        for source in [
            "2024-11-05T12:00:00Z",
            "2024-11-05T12:00:00+09:00",
            "2024-11-05T12:00:00.123456789Z",
            "2024-11-05T12:00:00+23:59",
            "2024-11-05t12:00:00z",
        ] {
            assert!(
                OffsetDateTimeLiteral::parse(source).is_ok(),
                "expected `{source}` to be accepted"
            );
        }
    }

    #[test]
    fn offset_literal_rejects_forms_outside_rfc3339() {
        for source in [
            "20241105T120000Z",
            "2024-11-05T12:00:00+09:00:30",
            "2024-11-05T12:00:00+25:00",
        ] {
            assert!(
                matches!(
                    OffsetDateTimeLiteral::parse(source),
                    Err(ParseOffsetDateTimeLiteralError::InvalidRfc3339)
                ),
                "expected `{source}` to be rejected as non-RFC 3339"
            );
        }
    }

    #[test]
    fn offset_literal_requires_an_explicit_offset() {
        assert!(matches!(
            OffsetDateTimeLiteral::parse("2024-11-05T12:00:00"),
            Err(ParseOffsetDateTimeLiteralError::MissingOffset)
        ));
        for scale in TimeScale::ALL_NAMES {
            let source = format!("2024-11-05T12:00:00 {scale}");
            assert!(OffsetDateTimeLiteral::parse(&source).is_err());
        }
        assert!(matches!(
            OffsetDateTimeLiteral::parse("2024-11-05T12:00:00+09:00[Asia/Tokyo]"),
            Err(ParseOffsetDateTimeLiteralError::TimeZoneAnnotation)
        ));
    }

    #[test]
    fn civil_literal_rejects_other_interpretation_sources() {
        assert!(CivilDateTimeLiteral::parse("2024-11-05T12:00:00").is_ok());
        assert!(CivilDateTimeLiteral::parse("2024-11-05T12:00:00Z").is_err());
        assert!(CivilDateTimeLiteral::parse("2024-11-05T12:00:00+09:00").is_err());
        for scale in TimeScale::ALL_NAMES {
            let source = format!("2024-11-05T12:00:00 {scale}");
            assert!(CivilDateTimeLiteral::parse(&source).is_err());
        }
        assert!(CivilDateTimeLiteral::parse("2024-11-05T12:00:00[Asia/Tokyo]").is_err());
    }

    #[test]
    fn civil_literal_requires_an_explicit_time() {
        assert!(matches!(
            CivilDateTimeLiteral::parse("2024-11-05"),
            Err(ParseCivilDateTimeLiteralError::MissingTime)
        ));
        assert!(CivilDateTimeLiteral::parse("2024-11-05T00:00:00").is_ok());
    }

    #[test]
    fn leap_second_literals_are_rejected_not_clamped() {
        for source in [
            "2016-12-31T23:59:60Z",
            "2016-12-31T23:59:60.5Z",
            "2016-12-31T23:59:60+09:00",
            "2016-12-31t23:59:60z",
        ] {
            assert!(
                matches!(
                    OffsetDateTimeLiteral::parse(source),
                    Err(ParseOffsetDateTimeLiteralError::LeapSecond)
                ),
                "expected `{source}` to be rejected as a leap second"
            );
        }
        for source in [
            "2016-12-31T23:59:60",
            "2016-12-31T23:59:60.25",
            "2016-12-31t23:59:60",
        ] {
            assert!(
                matches!(
                    CivilDateTimeLiteral::parse(source),
                    Err(ParseCivilDateTimeLiteralError::LeapSecond)
                ),
                "expected `{source}` to be rejected as a leap second"
            );
        }
        // Outside the strict shape a `60` seconds field is a syntax error like
        // any other non-canonical spelling.
        assert!(matches!(
            OffsetDateTimeLiteral::parse("20161231T235960Z"),
            Err(ParseOffsetDateTimeLiteralError::InvalidRfc3339)
        ));
        for source in ["20161231T235960", "2016-12-31 23:59:60"] {
            assert!(
                matches!(
                    CivilDateTimeLiteral::parse(source),
                    Err(ParseCivilDateTimeLiteralError::InvalidSyntax)
                ),
                "expected `{source}` to be rejected as non-canonical syntax"
            );
        }
        // `:59` and a `60` in the fraction stay valid; sixty minutes or hours
        // are ordinary range errors.
        for source in ["2016-12-31T23:59:59", "2016-12-31T23:59:59.60"] {
            assert!(
                CivilDateTimeLiteral::parse(source).is_ok(),
                "expected `{source}` to be accepted"
            );
        }
        assert!(OffsetDateTimeLiteral::parse("2016-12-31T23:59:59.60Z").is_ok());
        for source in ["2016-12-31T23:60:00", "2016-12-31T60:00:00"] {
            assert!(
                matches!(
                    CivilDateTimeLiteral::parse(source),
                    Err(ParseCivilDateTimeLiteralError::Invalid(_)
                        | ParseCivilDateTimeLiteralError::InvalidSyntax)
                ),
                "expected `{source}` to be rejected as out of range"
            );
        }
    }

    #[test]
    fn civil_literal_accepts_only_the_canonical_shape() {
        for (source, expected) in [
            ("2024-11-05T12:00:00", "2024-11-05T12:00:00"),
            ("2024-11-05t12:00:00", "2024-11-05T12:00:00"),
            ("2024-11-05T12:00:00.5", "2024-11-05T12:00:00.5"),
            (
                "2024-11-05T12:00:00.123456789",
                "2024-11-05T12:00:00.123456789",
            ),
        ] {
            let parsed = CivilDateTimeLiteral::parse(source)
                .unwrap_or_else(|error| panic!("expected `{source}` to be accepted: {error}"));
            assert_eq!(parsed.to_string(), expected);
        }
    }

    #[test]
    fn civil_literal_rejects_lax_spellings() {
        for source in [
            "2024-11-05 12:00:00",
            "2024-11-05T12:00",
            "2024-11-05T12",
            "20241105T120000",
            "20241105 1200",
            "20241105T12:00:00",
            "2024-11-05T12:00:00,5",
            "+002024-11-05T12:00:00",
            "-002024-11-05T12:00:00",
            "2024-11-05T12:00:00[u-ca=hebrew]",
            "2024-11-05T12:00:00[foo=bar]",
        ] {
            assert!(
                matches!(
                    CivilDateTimeLiteral::parse(source),
                    Err(ParseCivilDateTimeLiteralError::InvalidSyntax)
                ),
                "expected `{source}` to be rejected as non-canonical syntax, got {:?}",
                CivilDateTimeLiteral::parse(source)
            );
        }
        for source in [
            " 2024-11-05T12:00:00",
            "2024-11-05T12:00:00 ",
            "2024-11-05T12:00:00.",
            "2024-11-05T12:00:00.1234567890",
        ] {
            assert!(
                CivilDateTimeLiteral::parse(source).is_err(),
                "expected `{source}` to be rejected"
            );
        }
    }

    #[test]
    fn civil_literal_keeps_specific_errors_ahead_of_the_shape_check() {
        assert!(matches!(
            CivilDateTimeLiteral::parse("20241105T120000Z"),
            Err(ParseCivilDateTimeLiteralError::Offset)
        ));
        assert!(matches!(
            CivilDateTimeLiteral::parse("2024-11-05 12:00+09:00"),
            Err(ParseCivilDateTimeLiteralError::Offset)
        ));
        assert!(matches!(
            CivilDateTimeLiteral::parse("2024-11-05 12:00[Asia/Tokyo]"),
            Err(ParseCivilDateTimeLiteralError::TimeZoneAnnotation)
        ));
        assert!(matches!(
            CivilDateTimeLiteral::parse("20241105"),
            Err(ParseCivilDateTimeLiteralError::MissingTime)
        ));
    }

    #[test]
    fn invalid_calendar_coordinates_are_rejected() {
        assert!(OffsetDateTimeLiteral::parse("2023-02-30T00:00:00Z").is_err());
        assert!(CivilDateTimeLiteral::parse("2023-02-30T00:00:00").is_err());
    }

    #[test]
    fn zoned_literal_resolves_only_unambiguous_civil_times() {
        let time_zones = TimeZoneRegistry::bundled();
        let time_zone = time_zones.parse_iana_id("America/New_York").unwrap();
        let datetime = CivilDateTimeLiteral::parse("2024-07-15T17:30:00").unwrap();
        let resolved =
            ZonedDateTimeLiteral::resolve(datetime, time_zone.clone(), &time_zones).unwrap();

        assert_eq!(resolved.time_zone(), &time_zone);
        assert_eq!(
            resolved.timestamp(),
            "2024-07-15T21:30:00Z".parse::<jiff::Timestamp>().unwrap()
        );
    }

    #[test]
    fn zoned_literal_distinguishes_nonexistent_and_repeated_civil_times() {
        let time_zones = TimeZoneRegistry::bundled();
        let time_zone = time_zones.parse_iana_id("America/New_York").unwrap();

        let gap = CivilDateTimeLiteral::parse("2024-03-10T02:30:00").unwrap();
        assert!(matches!(
            ZonedDateTimeLiteral::resolve(gap, time_zone.clone(), &time_zones),
            Err(ResolveZonedDateTimeLiteralError::Nonexistent { .. })
        ));

        let fold = CivilDateTimeLiteral::parse("2024-11-03T01:30:00").unwrap();
        assert!(matches!(
            ZonedDateTimeLiteral::resolve(fold, time_zone, &time_zones),
            Err(ResolveZonedDateTimeLiteralError::Repeated { .. })
        ));
    }

    #[test]
    fn epoch_literal_interprets_civil_coordinates_in_its_scale() {
        let civil = CivilDateTimeLiteral::parse("2000-01-01T12:00:00.25").unwrap();
        for scale in TimeScale::ALL {
            let literal = EpochLiteral::resolve(civil, scale).unwrap();
            assert_eq!(literal.scale(), scale);
            assert_eq!(literal.civil, civil);
            assert_eq!(
                literal.epoch(),
                hifitime::Epoch::maybe_from_gregorian(
                    2000,
                    1,
                    1,
                    12,
                    0,
                    0,
                    250_000_000,
                    scale.to_hifitime()
                )
                .unwrap(),
                "{scale}"
            );
        }
    }
}
