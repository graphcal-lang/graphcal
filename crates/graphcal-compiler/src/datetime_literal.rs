//! Parsed datetime literal forms at the source-to-semantic boundary.
//!
//! Graphcal's constructors intentionally accept different literal categories:
//! an offset-bearing instant for one-argument `datetime`, and a scale/offset-
//! free civil coordinate for timezone-based `datetime` and `epoch<S>`. Keeping
//! those forms distinct makes contradictory interpretation sources impossible
//! after HIR lowering.

use crate::registry::time_scale::TimeScale;
use thiserror::Error;

/// Error returned when parsing an offset-bearing datetime literal.
#[derive(Debug, Error)]
pub enum ParseOffsetDateTimeLiteralError {
    #[error(transparent)]
    Invalid(#[from] jiff::Error),
    #[error("an explicit `Z` or numeric offset is required")]
    MissingOffset,
    #[error("a named timezone annotation is not allowed when the offset determines the instant")]
    TimeZoneAnnotation,
}

/// Error returned when parsing an offset/zone-free civil datetime literal.
#[derive(Debug, Error)]
pub enum ParseCivilDateTimeLiteralError {
    #[error(transparent)]
    Invalid(#[from] jiff::Error),
    #[error("an offset is not allowed in a local civil datetime literal")]
    Offset,
    #[error("a timezone annotation is not allowed in a local civil datetime literal")]
    TimeZoneAnnotation,
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
    /// Returns an error if `source` is invalid or carries an offset or named
    /// timezone annotation.
    pub fn parse(source: &str) -> Result<Self, ParseCivilDateTimeLiteralError> {
        let parser = jiff::fmt::temporal::DateTimeParser::new();
        let pieces = parser.parse_pieces(source)?;
        if pieces.offset().is_some() {
            return Err(ParseCivilDateTimeLiteralError::Offset);
        }
        if pieces.time_zone_annotation().is_some() {
            return Err(ParseCivilDateTimeLiteralError::TimeZoneAnnotation);
        }
        Ok(Self(pieces.date().to_datetime(
            pieces.time().unwrap_or_else(jiff::civil::Time::midnight),
        )))
    }

    /// The parsed Gregorian civil coordinate represented by this literal.
    #[must_use]
    pub const fn datetime(self) -> jiff::civil::DateTime {
        self.0
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
                "timezone-based datetime literal; expected local civil time without an offset, timezone annotation, or time scale",
            ),
            Self::Epoch(scale) => write!(
                f,
                "epoch<{scale}> literal; expected a scale-free civil time without an offset, timezone, or time-scale suffix"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_literal_requires_an_explicit_offset() {
        assert!(OffsetDateTimeLiteral::parse("2024-11-05T12:00:00Z").is_ok());
        assert!(OffsetDateTimeLiteral::parse("2024-11-05T12:00:00+09:00").is_ok());
        assert!(OffsetDateTimeLiteral::parse("2024-11-05T12:00:00").is_err());
        for scale in TimeScale::ALL_NAMES {
            let source = format!("2024-11-05T12:00:00 {scale}");
            assert!(OffsetDateTimeLiteral::parse(&source).is_err());
        }
        assert!(OffsetDateTimeLiteral::parse("2024-11-05T12:00:00+09:00[Asia/Tokyo]").is_err());
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
    fn invalid_calendar_coordinates_are_rejected() {
        assert!(OffsetDateTimeLiteral::parse("2023-02-30T00:00:00Z").is_err());
        assert!(CivilDateTimeLiteral::parse("2023-02-30T00:00:00").is_err());
    }
}
