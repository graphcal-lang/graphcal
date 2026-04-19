//! Parsing and validation of `graphcal.toml` manifest files.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors that can occur when parsing a manifest file.
#[derive(Debug, Clone, Error)]
pub enum ManifestError {
    #[error("failed to read graphcal.toml: {message}")]
    IoError { message: String },

    #[error("invalid TOML in graphcal.toml: {message}")]
    TomlParseError { message: String },

    #[error("missing required field [package].name in graphcal.toml")]
    MissingPackageName,

    #[error("invalid package name '{name}': must be lower_snake_case")]
    InvalidPackageName { name: String },
}

/// The parsed `graphcal.toml` manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The package name (required, `lower_snake_case`).
    pub package_name: String,
    /// The source directory relative to the project root (defaults to `"src"`).
    pub source_dir: PathBuf,
}

/// Parse a `graphcal.toml` manifest from a file path.
///
/// # Errors
///
/// Returns a [`ManifestError`] if the file cannot be read, contains invalid TOML,
/// or is missing required fields.
pub fn parse_manifest(path: &Path) -> Result<Manifest, ManifestError> {
    let content = std::fs::read_to_string(path).map_err(|e| ManifestError::IoError {
        message: e.to_string(),
    })?;

    parse_manifest_str(&content)
}

/// Parse manifest from a TOML string.
///
/// This is the I/O-free entry point — the caller is responsible for reading the
/// file contents. [`parse_manifest`] is a convenience wrapper that reads from disk.
///
/// # Errors
///
/// Returns a [`ManifestError`] if the content is invalid TOML or missing required fields.
pub fn parse_manifest_str(content: &str) -> Result<Manifest, ManifestError> {
    let arena = toml_spanner::Arena::new();
    let root = toml_spanner::parse(content, &arena).map_err(|e| ManifestError::TomlParseError {
        message: e.to_string(),
    })?;

    // Extract [package].name (required).
    let name = root["package"]["name"]
        .as_str()
        .ok_or(ManifestError::MissingPackageName)?;

    if !is_valid_package_name(name) {
        return Err(ManifestError::InvalidPackageName {
            name: name.to_string(),
        });
    }

    // Extract source_dir (optional, defaults to "src").
    let source_dir = PathBuf::from(root["package"]["source_dir"].as_str().unwrap_or("src"));

    Ok(Manifest {
        package_name: name.to_string(),
        source_dir,
    })
}

/// A valid package name follows `lower_snake_case` rules.
fn is_valid_package_name(s: &str) -> bool {
    crate::syntax::names::is_lower_snake_case(s)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = parse_manifest_str("[package]\nname = \"my_package\"\n").unwrap();
        assert_eq!(manifest.package_name, "my_package");
        assert_eq!(manifest.source_dir, PathBuf::from("src"));
    }

    #[test]
    fn parse_manifest_with_custom_source_dir() {
        let manifest =
            parse_manifest_str("[package]\nname = \"my_package\"\nsource_dir = \"lib\"\n").unwrap();
        assert_eq!(manifest.package_name, "my_package");
        assert_eq!(manifest.source_dir, PathBuf::from("lib"));
    }

    #[test]
    fn missing_package_section() {
        let result = parse_manifest_str("");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }

    #[test]
    fn missing_package_name() {
        let result = parse_manifest_str("[package]\nsource_dir = \"src\"\n");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }

    #[test]
    fn invalid_package_name_uppercase() {
        let result = parse_manifest_str("[package]\nname = \"MyPackage\"\n");
        assert!(matches!(
            result,
            Err(ManifestError::InvalidPackageName { .. })
        ));
    }

    #[test]
    fn invalid_package_name_hyphen() {
        let result = parse_manifest_str("[package]\nname = \"my-package\"\n");
        assert!(matches!(
            result,
            Err(ManifestError::InvalidPackageName { .. })
        ));
    }

    #[test]
    fn valid_package_names() {
        assert!(is_valid_package_name("my_package"));
        assert!(is_valid_package_name("package"));
        assert!(is_valid_package_name("package_v2"));
        assert!(is_valid_package_name("p"));
    }

    #[test]
    fn invalid_package_names() {
        assert!(!is_valid_package_name("MyPackage"));
        assert!(!is_valid_package_name("PACKAGE"));
        assert!(!is_valid_package_name("_package"));
        assert!(!is_valid_package_name("2package"));
        assert!(!is_valid_package_name("my-package"));
        assert!(!is_valid_package_name(""));
    }

    #[test]
    fn invalid_toml() {
        let result = parse_manifest_str("this is not valid toml [[[");
        assert!(matches!(result, Err(ManifestError::TomlParseError { .. })));
    }

    #[test]
    fn empty_manifest_is_missing_package() {
        // An empty file (current marker behavior) has no [package] section.
        let result = parse_manifest_str("");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }
}
