//! Reproducible IANA timezone lookup backed by Graphcal's bundled tzdb.
//!
//! The host's zoneinfo installation is deliberately never consulted. Every
//! compiler and evaluator path receives this registry through
//! [`Registry`](crate::registry::types::Registry)
//! and therefore resolves names against the tzdb release pinned in Cargo.

use jiff::tz::{TimeZone, TimeZoneDatabase};

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

    /// Resolve an IANA timezone name against the bundled registry.
    ///
    /// # Errors
    ///
    /// Returns a Jiff timezone lookup error when `name` is not present in the
    /// pinned database.
    pub fn get(&self, name: &str) -> Result<TimeZone, jiff::Error> {
        self.database.get(name)
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
        let tokyo = registry.get("Asia/Tokyo").unwrap();
        assert_eq!(tokyo.iana_name(), Some("Asia/Tokyo"));
        assert!(registry.get("Not/A_Timezone").is_err());
    }

    #[test]
    fn lookup_canonicalizes_case_without_using_host_zoneinfo() {
        let registry = TimeZoneRegistry::bundled();
        let new_york = registry.get("america/new_york").unwrap();
        assert_eq!(new_york.iana_name(), Some("America/New_York"));
    }
}
