//! Reproducible IANA timezone lookup backed by Graphcal's bundled tzdb.
//!
//! The host's zoneinfo installation is deliberately never consulted. Every
//! compiler and evaluator path receives this registry through
//! [`Registry`](crate::registry::types::Registry)
//! and therefore resolves names against the tzdb release pinned in Cargo.

use jiff::tz::{TimeZone, TimeZoneDatabase};
use thiserror::Error;

/// A canonical IANA timezone identifier validated against Graphcal's bundled
/// timezone database.
///
/// Construction is intentionally private to [`TimeZoneRegistry`], so an
/// unknown or non-canonical identifier cannot enter the typed compiler core.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IanaTimeZoneId(String);

impl IanaTimeZoneId {
    /// Return the canonical IANA identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IanaTimeZoneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a source timezone literal is absent from the bundled
/// IANA registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown timezone `{name}`")]
pub struct UnknownIanaTimeZoneError {
    name: String,
}

impl UnknownIanaTimeZoneError {
    /// The source spelling that failed lookup.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The IANA timezone registry bundled with the Graphcal toolchain.
///
/// Clones share Jiff's parsed-zone cache. Constructing this registry is
/// infallible because `graphcal-compiler` always enables Jiff's
/// `tzdb-bundle-always` feature.
#[derive(Clone)]
pub struct TimeZoneRegistry {
    database: TimeZoneDatabase,
}

impl TimeZoneRegistry {
    /// Construct a registry backed only by the bundled, Cargo-pinned tzdb.
    #[must_use]
    pub fn bundled() -> Self {
        let database = TimeZoneDatabase::bundled();
        debug_assert!(
            !database.is_definitively_empty(),
            "tzdb-bundle-always must provide Graphcal's timezone database"
        );
        Self { database }
    }

    /// The bundled IANA tzdb release, such as `"2026c"`.
    #[must_use]
    pub fn version(&self) -> Option<&'static str> {
        jiff_tzdb::VERSION
    }

    /// Validate and canonicalize a source timezone literal.
    ///
    /// This is the only string-to-timezone crossing into Graphcal's typed
    /// core. Subsequent lookup accepts only the validated identifier.
    ///
    /// # Errors
    ///
    /// Returns [`UnknownIanaTimeZoneError`] when `name` is not present in the
    /// pinned database.
    pub fn parse_iana_id(&self, name: &str) -> Result<IanaTimeZoneId, UnknownIanaTimeZoneError> {
        self.database
            .get(name)
            .ok()
            .and_then(|time_zone| time_zone.iana_name().map(str::to_owned))
            .map(IanaTimeZoneId)
            .ok_or_else(|| UnknownIanaTimeZoneError {
                name: name.to_string(),
            })
    }

    /// Load a timezone by its already validated canonical identifier.
    ///
    /// # Errors
    ///
    /// Returns a Jiff lookup error only if the bundled registry violates the
    /// invariant established by [`Self::parse_iana_id`].
    pub fn get(&self, id: &IanaTimeZoneId) -> Result<TimeZone, jiff::Error> {
        self.database.get(id.as_str())
    }
}

impl Default for TimeZoneRegistry {
    fn default() -> Self {
        Self::bundled()
    }
}

impl std::fmt::Debug for TimeZoneRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeZoneRegistry")
            .field("tzdb_version", &self.version())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_has_inspectable_version() {
        let registry = TimeZoneRegistry::bundled();
        assert!(registry.version().is_some());
    }

    #[test]
    fn bundled_registry_resolves_known_zone_and_rejects_unknown_zone() {
        let registry = TimeZoneRegistry::bundled();
        let tokyo_id = registry.parse_iana_id("Asia/Tokyo").unwrap();
        let tokyo = registry.get(&tokyo_id).unwrap();
        assert_eq!(tokyo.iana_name(), Some("Asia/Tokyo"));
        assert!(registry.parse_iana_id("Not/A_Timezone").is_err());
    }

    #[test]
    fn lookup_canonicalizes_case_without_using_host_zoneinfo() {
        let registry = TimeZoneRegistry::bundled();
        let new_york_id = registry.parse_iana_id("america/new_york").unwrap();
        assert_eq!(new_york_id.as_str(), "America/New_York");
        let new_york = registry.get(&new_york_id).unwrap();
        assert_eq!(new_york.iana_name(), Some("America/New_York"));
    }
}
