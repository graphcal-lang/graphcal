//! Parsing and validation of `graphcal.toml` manifest files.

use std::{path::PathBuf, str::FromStr};

use thiserror::Error;

/// Errors that can occur when parsing a manifest file.
#[derive(Debug, Clone, Error)]
pub enum ManifestError {
    #[error("invalid TOML in graphcal.toml: {message}")]
    TomlParseError { message: String },

    #[error("missing required field [package].name in graphcal.toml")]
    MissingPackageName,

    #[error("invalid package name '{name}': must be lower_snake_case")]
    InvalidPackageName { name: String },

    #[error(
        "invalid source_dir '{dir}': must be a relative path inside the \
         project root (no absolute paths or `..` components)"
    )]
    InvalidSourceDir { dir: PathBuf },
}

#[derive(Debug, serde::Deserialize)]
struct RawManifest {
    package: Option<RawPackage>,
}

#[derive(Debug, serde::Deserialize)]
struct RawPackage {
    name: Option<String>,
    source_dir: Option<String>,
}

/// The parsed `graphcal.toml` manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The package name (required, `lower_snake_case`).
    package_name: String,
    /// The source directory relative to the project root (defaults to `"src"`).
    source_dir: PathBuf,
}

impl Manifest {
    /// Construct a validated manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] if the package name or source directory is invalid.
    fn new(package_name: String, source_dir: PathBuf) -> Result<Self, ManifestError> {
        if !is_valid_package_name(&package_name) {
            return Err(ManifestError::InvalidPackageName { name: package_name });
        }

        let escapes_root = source_dir.is_absolute()
            || source_dir.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
        if escapes_root {
            return Err(ManifestError::InvalidSourceDir { dir: source_dir });
        }

        Ok(Self {
            package_name,
            source_dir,
        })
    }

    /// The package name.
    #[must_use]
    pub fn package_name(&self) -> &str {
        &self.package_name
    }

    /// The source directory relative to the project root.
    #[must_use]
    pub fn source_dir(&self) -> &std::path::Path {
        &self.source_dir
    }
}

impl TryFrom<RawManifest> for Manifest {
    type Error = ManifestError;

    fn try_from(raw: RawManifest) -> Result<Self, Self::Error> {
        // Extract [package].name (required).
        let package = raw.package.ok_or(ManifestError::MissingPackageName)?;
        let name = package.name.ok_or(ManifestError::MissingPackageName)?;

        // Extract source_dir (optional, defaults to "src"). The package name two
        // lines above is strictly validated; the source dir gets the same
        // treatment — a manifest must not be able to point module resolution
        // outside the project root via an absolute path or `..` components.
        let source_dir = PathBuf::from(package.source_dir.unwrap_or_else(|| "src".to_string()));

        Self::new(name, source_dir)
    }
}

impl FromStr for Manifest {
    type Err = ManifestError;

    fn from_str(content: &str) -> Result<Self, Self::Err> {
        let raw =
            toml::from_str::<RawManifest>(content).map_err(|e| ManifestError::TomlParseError {
                message: e.to_string(),
            })?;

        Self::try_from(raw)
    }
}

/// A valid package name follows `lower_snake_case` rules.
fn is_valid_package_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Result<Manifest, ManifestError> {
        content.parse()
    }

    #[test]
    fn parse_minimal_manifest() {
        let manifest = parse("[package]\nname = \"my_package\"\n").unwrap();
        assert_eq!(manifest.package_name(), "my_package");
        assert_eq!(manifest.source_dir(), PathBuf::from("src"));
    }

    #[test]
    fn parse_manifest_with_custom_source_dir() {
        let manifest = parse("[package]\nname = \"my_package\"\nsource_dir = \"lib\"\n").unwrap();
        assert_eq!(manifest.package_name(), "my_package");
        assert_eq!(manifest.source_dir(), PathBuf::from("lib"));
    }

    #[test]
    fn missing_package_section() {
        let result = parse("");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }

    #[test]
    fn missing_package_name() {
        let result = parse("[package]\nsource_dir = \"src\"\n");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }

    #[test]
    fn invalid_package_name_uppercase() {
        let result = parse("[package]\nname = \"MyPackage\"\n");
        assert!(matches!(
            result,
            Err(ManifestError::InvalidPackageName { .. })
        ));
    }

    #[test]
    fn invalid_package_name_hyphen() {
        let result = parse("[package]\nname = \"my-package\"\n");
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
        let result = parse("this is not valid toml [[[");
        assert!(matches!(result, Err(ManifestError::TomlParseError { .. })));
    }

    #[test]
    fn empty_manifest_is_missing_package() {
        // An empty file (current marker behavior) has no [package] section.
        let result = parse("");
        assert!(matches!(result, Err(ManifestError::MissingPackageName)));
    }

    #[test]
    fn source_dir_escaping_the_root_is_rejected() {
        // Regression: a malicious manifest could point module resolution
        // outside the project root.
        for dir in ["../elsewhere", "/etc", "a/../../b", "./../x"] {
            let toml = format!("[package]\nname = \"pkg\"\nsource_dir = \"{dir}\"\n");
            assert!(
                matches!(parse(&toml), Err(ManifestError::InvalidSourceDir { .. })),
                "source_dir `{dir}` must be rejected"
            );
        }
    }

    #[test]
    fn relative_source_dir_is_accepted() {
        let toml = "[package]\nname = \"pkg\"\nsource_dir = \"lib/nested\"\n";
        let manifest = parse(toml).unwrap();
        assert_eq!(
            manifest.source_dir(),
            std::path::PathBuf::from("lib/nested")
        );
    }
}
