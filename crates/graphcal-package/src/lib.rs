//! Pure package-management domain model for Graphcal.
//!
//! This crate intentionally contains no Git, filesystem, cache, or CLI I/O.
//! Callers provide manifest and lockfile text, source metadata, and materialized
//! package manifests; this crate validates and resolves the typed package graph.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;
use toml::{Table, Value};
use url::{Host, Url};

/// Graphcal's first lockfile schema version.
pub const LOCK_VERSION: u64 = 1;
/// Current standard-library identity recorded by the MVP lockfile.
pub const STDLIB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A package's real `[package].name`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

/// A local dependency alias visible in one package instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DependencyName(String);

/// Opaque identifier for one locked package instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageInstanceId(String);

/// Full immutable Git commit hash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitCommitHash(String);

/// Canonical SHA-256 digest stored as exactly 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest([u8; 32]);

/// Parsed remote Git repository URL.
///
/// Only HTTPS, SSH URL, and SSH scp-like transports are representable. Local
/// paths and arbitrary URL schemes cannot enter the package graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitUrl(GitUrlKind);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum GitUrlKind {
    Https {
        host: Host<String>,
        port: Option<u16>,
        path: GitRepositoryPath,
    },
    Ssh {
        user: Option<SshUser>,
        host: Host<String>,
        port: Option<u16>,
        path: GitRepositoryPath,
    },
    Scp {
        user: SshUser,
        host: Host<String>,
        path: GitRepositoryPath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SshUser(String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GitRepositoryPath {
    components: Vec<GitRepositoryPathComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GitRepositoryPathComponent(String);

/// Supported transport of a validated remote Git URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitTransport {
    /// TLS-protected HTTP Git transport.
    Https,
    /// Explicit `ssh://` URL.
    Ssh,
    /// SSH scp-like `user@host:path` syntax.
    Scp,
}

/// Portable package-relative source directory.
///
/// The directory is stored as validated `/`-separated components rather than
/// a host [`PathBuf`]. This keeps manifest and lockfile identity independent of
/// the operating system that parses them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageSourceDirectory {
    components: Vec<SourceDirectoryComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SourceDirectoryComponent(String);

macro_rules! impl_string_newtype {
    ($ty:ty) => {
        impl $ty {
            /// Borrow the validated string representation.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

impl_string_newtype!(PackageName);
impl_string_newtype!(DependencyName);
impl_string_newtype!(PackageInstanceId);
impl_string_newtype!(GitCommitHash);

impl PackageSourceDirectory {
    /// Parse a non-empty portable relative directory.
    ///
    /// # Errors
    ///
    /// Returns [`PackageSourceDirectoryError`] for absolute paths, empty or
    /// dot components, backslashes, control characters, or Windows-prefix
    /// ambiguity.
    pub fn new(value: &str) -> Result<Self, PackageSourceDirectoryError> {
        let invalid = |reason| PackageSourceDirectoryError {
            value: value.to_string(),
            reason,
        };
        if value.is_empty() {
            return Err(invalid(PackageSourceDirectoryErrorReason::Empty));
        }
        if value.starts_with('/') {
            return Err(invalid(PackageSourceDirectoryErrorReason::Absolute));
        }
        if value.contains('\\') {
            return Err(invalid(PackageSourceDirectoryErrorReason::Backslash));
        }
        if value.contains(':') {
            return Err(invalid(PackageSourceDirectoryErrorReason::PrefixAmbiguity));
        }
        let components = value
            .split('/')
            .map(|component| {
                if component.is_empty() {
                    Err(invalid(PackageSourceDirectoryErrorReason::EmptyComponent))
                } else if component == "." {
                    Err(invalid(PackageSourceDirectoryErrorReason::CurrentDirectory))
                } else if component == ".." {
                    Err(invalid(PackageSourceDirectoryErrorReason::ParentDirectory))
                } else if component.chars().any(char::is_control) {
                    Err(invalid(PackageSourceDirectoryErrorReason::ControlCharacter))
                } else {
                    Ok(SourceDirectoryComponent(component.to_string()))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { components })
    }

    /// Materialize the validated components as a host relative path.
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        self.components
            .iter()
            .fold(PathBuf::new(), |mut path, component| {
                path.push(&component.0);
                path
            })
    }

    /// Join these validated components below a host filesystem root.
    #[must_use]
    pub fn join_to(&self, root: &Path) -> PathBuf {
        root.join(self.to_path_buf())
    }
}

impl fmt::Display for PackageSourceDirectory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                formatter.write_str("/")?;
            }
            formatter.write_str(&component.0)?;
        }
        Ok(())
    }
}

impl PackageName {
    /// Construct a package name using Graphcal lower-snake package rules.
    ///
    /// # Errors
    ///
    /// Returns [`PackageNameError`] when `value` is not a valid package name.
    pub fn new(value: impl Into<String>) -> Result<Self, PackageNameError> {
        let value = value.into();
        if is_valid_name(&value) {
            Ok(Self(value))
        } else {
            Err(PackageNameError { value })
        }
    }
}

impl DependencyName {
    /// Construct a dependency name using the source-visible identifier rules.
    ///
    /// # Errors
    ///
    /// Returns [`DependencyNameError`] when `value` is not a valid dependency
    /// alias.
    fn new(value: impl Into<String>) -> Result<Self, DependencyNameError> {
        let value = value.into();
        if is_valid_name(&value) {
            Ok(Self(value))
        } else {
            Err(DependencyNameError { value })
        }
    }
}

impl PackageInstanceId {
    /// Construct an opaque package instance id.
    ///
    /// The format is deliberately only validated as an opaque TOML-friendly
    /// token. Readers must not parse package semantics out of it.
    ///
    /// # Errors
    ///
    /// Returns [`PackageInstanceIdError`] when `value` is empty or contains
    /// characters outside the stable lockfile token alphabet.
    pub fn new(value: impl Into<String>) -> Result<Self, PackageInstanceIdError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if valid {
            Ok(Self(value))
        } else {
            Err(PackageInstanceIdError { value })
        }
    }
}

impl GitCommitHash {
    /// Construct a full SHA-1 Git commit hash.
    ///
    /// # Errors
    ///
    /// Returns [`GitRevError`] when `value` is not a 40-character hex hash.
    fn new(value: impl Into<String>) -> Result<Self, GitRevError> {
        let value = value.into();
        if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(GitRevError { value })
        }
    }
}

impl Sha256Digest {
    /// Parse exactly 64 lowercase hexadecimal digits.
    ///
    /// # Errors
    ///
    /// Returns [`Sha256DigestError`] for any non-canonical spelling.
    pub fn new(value: impl Into<String>) -> Result<Self, Sha256DigestError> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.len() != 64 {
            return Err(Sha256DigestError { value });
        }
        let mut digest = [0_u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            let (Some(high), Some(low)) = (hex_nibble(pair[0]), hex_nibble(pair[1])) else {
                return Err(Sha256DigestError { value });
            };
            digest[index] = (high << 4) | low;
        }
        Ok(Self(digest))
    }

    /// Construct a digest from its exact binary representation.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact binary representation.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Non-canonical SHA-256 spelling.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid SHA-256 digest `{value}`: must be 64 lowercase hex digits")]
pub struct Sha256DigestError {
    value: String,
}

impl GitUrl {
    /// Parse a supported remote Git URL into typed transport, authority, and
    /// repository-path components.
    ///
    /// # Errors
    ///
    /// Returns [`GitUrlError`] for malformed/local URLs, unsupported schemes,
    /// credentials, empty repositories, or option-shaped components.
    fn new(value: impl Into<String>) -> Result<Self, GitUrlError> {
        let value = value.into();
        let invalid = |reason| GitUrlError {
            value: value.clone(),
            reason,
        };
        if value.is_empty() {
            return Err(invalid(GitUrlErrorReason::Empty));
        }
        if value.trim() != value
            || value.chars().any(char::is_control)
            || value.contains('\\')
            || value.split('/').any(is_ambiguous_raw_path_component)
        {
            return Err(invalid(GitUrlErrorReason::Malformed));
        }
        if value.starts_with('-') {
            return Err(invalid(GitUrlErrorReason::OptionShaped));
        }

        if value.contains("://") {
            parse_standard_git_url(&value).map(Self).map_err(invalid)
        } else {
            parse_scp_git_url(&value).map(Self).map_err(invalid)
        }
    }

    /// Remote transport selected by this URL.
    #[must_use]
    pub const fn transport(&self) -> GitTransport {
        match &self.0 {
            GitUrlKind::Https { .. } => GitTransport::Https,
            GitUrlKind::Ssh { .. } => GitTransport::Ssh,
            GitUrlKind::Scp { .. } => GitTransport::Scp,
        }
    }
}

impl fmt::Display for GitUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            GitUrlKind::Https { host, port, path } => {
                write!(formatter, "https://{host}")?;
                if let Some(port) = port {
                    write!(formatter, ":{port}")?;
                }
                formatter.write_str("/")?;
                path.fmt(formatter)
            }
            GitUrlKind::Ssh {
                user,
                host,
                port,
                path,
            } => {
                formatter.write_str("ssh://")?;
                if let Some(user) = user {
                    write!(formatter, "{}@", user.0)?;
                }
                write!(formatter, "{host}")?;
                if let Some(port) = port {
                    write!(formatter, ":{port}")?;
                }
                formatter.write_str("/")?;
                path.fmt(formatter)
            }
            GitUrlKind::Scp { user, host, path } => {
                write!(formatter, "{}@{host}:", user.0)?;
                path.fmt(formatter)
            }
        }
    }
}

impl fmt::Display for GitRepositoryPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                formatter.write_str("/")?;
            }
            formatter.write_str(&component.0)?;
        }
        Ok(())
    }
}

fn parse_standard_git_url(value: &str) -> Result<GitUrlKind, GitUrlErrorReason> {
    let parsed = Url::parse(value).map_err(|_| GitUrlErrorReason::Malformed)?;
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err(GitUrlErrorReason::UnsupportedScheme(
            parsed.scheme().to_string(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(GitUrlErrorReason::QueryOrFragment);
    }
    let host = parsed
        .host()
        .map(|host| host.to_owned())
        .ok_or(GitUrlErrorReason::MissingHost)?;
    reject_option_shaped(&host.to_string())?;
    let path = parse_repository_path(
        parsed
            .path_segments()
            .ok_or(GitUrlErrorReason::MissingRepositoryPath)?,
    )?;

    match parsed.scheme() {
        "https" => {
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(GitUrlErrorReason::Credentials);
            }
            Ok(GitUrlKind::Https {
                host,
                port: parsed.port(),
                path,
            })
        }
        "ssh" => {
            if parsed.password().is_some() {
                return Err(GitUrlErrorReason::Credentials);
            }
            let user = parse_ssh_user(parsed.username())?;
            Ok(GitUrlKind::Ssh {
                user,
                host,
                port: parsed.port(),
                path,
            })
        }
        scheme => Err(GitUrlErrorReason::UnsupportedScheme(scheme.to_string())),
    }
}

fn parse_scp_git_url(value: &str) -> Result<GitUrlKind, GitUrlErrorReason> {
    let (authority, path) = value.split_once(':').ok_or(GitUrlErrorReason::Malformed)?;
    if authority.is_empty() || path.starts_with('/') {
        return Err(GitUrlErrorReason::Malformed);
    }
    let (user, host_text) = match authority.rsplit_once('@') {
        Some((user, host)) => (user, host),
        None if path.contains('@') => return Err(GitUrlErrorReason::Credentials),
        None => return Err(GitUrlErrorReason::Malformed),
    };
    if host_text.contains('@') {
        return Err(GitUrlErrorReason::Malformed);
    }
    reject_option_shaped(host_text)?;
    let host = Host::parse(host_text).map_err(|_| GitUrlErrorReason::Malformed)?;
    let user = parse_required_ssh_user(user)?;
    let path = parse_repository_path(path.split('/'))?;
    Ok(GitUrlKind::Scp { user, host, path })
}

fn parse_ssh_user(value: &str) -> Result<Option<SshUser>, GitUrlErrorReason> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse_required_ssh_user(value).map(Some)
    }
}

fn parse_required_ssh_user(value: &str) -> Result<SshUser, GitUrlErrorReason> {
    if value.is_empty() || value.contains('@') {
        return Err(GitUrlErrorReason::Malformed);
    }
    reject_option_shaped(value)?;
    Ok(SshUser(value.to_string()))
}

fn is_ambiguous_raw_path_component(component: &str) -> bool {
    let component = component
        .split(['?', '#'])
        .next()
        .unwrap_or(component)
        .to_ascii_lowercase();
    component.contains("%2f")
        || component.contains("%5c")
        || matches!(
            component.as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
}

fn parse_repository_path<'a>(
    components: impl IntoIterator<Item = &'a str>,
) -> Result<GitRepositoryPath, GitUrlErrorReason> {
    let mut components = components.into_iter().collect::<Vec<_>>();
    if components.last() == Some(&"") {
        components.pop();
    }
    if components.is_empty() {
        return Err(GitUrlErrorReason::MissingRepositoryPath);
    }
    let components = components
        .into_iter()
        .map(|component| {
            if component.is_empty() || is_ambiguous_raw_path_component(component) {
                Err(GitUrlErrorReason::Malformed)
            } else {
                reject_option_shaped(component)?;
                Ok(GitRepositoryPathComponent(component.to_string()))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GitRepositoryPath { components })
}

fn reject_option_shaped(value: &str) -> Result<(), GitUrlErrorReason> {
    if value.starts_with('-') {
        Err(GitUrlErrorReason::OptionShaped)
    } else {
        Ok(())
    }
}

/// Package name validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid package name `{value}`: must be lower_snake_case")]
pub struct PackageNameError {
    value: String,
}

/// Dependency alias validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid dependency name `{value}`: must be lower_snake_case")]
pub struct DependencyNameError {
    value: String,
}

/// Package instance id validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid package instance id `{value}`")]
pub struct PackageInstanceIdError {
    value: String,
}

/// Git revision validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid Git rev `{value}`: must be a full 40-character commit hash")]
pub struct GitRevError {
    value: String,
}

/// Git URL validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid Git URL `{value}`: {reason}")]
pub struct GitUrlError {
    value: String,
    reason: GitUrlErrorReason,
}

/// Portable source-directory validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid package source directory `{value}`: {reason}")]
pub struct PackageSourceDirectoryError {
    value: String,
    reason: PackageSourceDirectoryErrorReason,
}

/// Failed invariant for a portable package source directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSourceDirectoryErrorReason {
    /// The spelling had no components.
    Empty,
    /// The spelling started at a filesystem root.
    Absolute,
    /// Repeated or trailing `/` introduced an empty component.
    EmptyComponent,
    /// A `.` component made normalization implicit.
    CurrentDirectory,
    /// A `..` component could escape the package root.
    ParentDirectory,
    /// A backslash would be a separator only on some hosts.
    Backslash,
    /// A colon could be interpreted as a platform path prefix.
    PrefixAmbiguity,
    /// A control character cannot be a portable directory component.
    ControlCharacter,
}

impl fmt::Display for PackageSourceDirectoryErrorReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "must not be empty",
            Self::Absolute => "must be relative",
            Self::EmptyComponent => "must not contain empty components",
            Self::CurrentDirectory => "must not contain `.` components",
            Self::ParentDirectory => "must not contain `..` components",
            Self::Backslash => "must use `/`, not backslash separators",
            Self::PrefixAmbiguity => "must not contain a platform path prefix",
            Self::ControlCharacter => "must not contain control characters",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitUrlErrorReason {
    Empty,
    Malformed,
    UnsupportedScheme(String),
    MissingHost,
    MissingRepositoryPath,
    Credentials,
    QueryOrFragment,
    OptionShaped,
}

impl fmt::Display for GitUrlErrorReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("must not be empty"),
            Self::Malformed => formatter.write_str("must be a well-formed remote Git URL"),
            Self::UnsupportedScheme(scheme) => write!(
                formatter,
                "unsupported scheme `{scheme}` (expected HTTPS or SSH)"
            ),
            Self::MissingHost => formatter.write_str("must name a remote host"),
            Self::MissingRepositoryPath => {
                formatter.write_str("must name a repository path on the remote host")
            }
            Self::Credentials => formatter.write_str("must not include embedded credentials"),
            Self::QueryOrFragment => {
                formatter.write_str("must not include a query string or fragment")
            }
            Self::OptionShaped => {
                formatter.write_str("must not contain option-shaped authority or path components")
            }
        }
    }
}

fn is_valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parsed `graphcal.toml` package section and direct dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    /// Real package name.
    pub name: PackageName,
    /// Source directory relative to the package root.
    pub source_dir: PackageSourceDirectory,
    /// Direct dependencies keyed by local source-visible alias.
    pub dependencies: BTreeMap<DependencyName, DependencySpec>,
}

/// One direct dependency declaration in `graphcal.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// Real package name expected at the fetched source. `None` means it must
    /// match the dependency alias.
    package: Option<PackageName>,
    /// Exact-rev Git source.
    pub git: GitDependency,
}

impl DependencySpec {
    /// Real package name expected for this dependency target.
    #[must_use]
    pub fn expected_package_name(&self, alias: &DependencyName) -> PackageName {
        self.package
            .clone()
            .unwrap_or_else(|| PackageName(alias.as_str().to_string()))
    }
}

/// Exact-rev Git dependency source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDependency {
    /// Repository URL.
    pub url: GitUrl,
    /// Required immutable revision.
    pub rev: GitCommitHash,
}

/// Manifest parse or validation error.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// TOML syntax failed before semantic validation could run.
    #[error("invalid TOML in graphcal.toml: {message}")]
    TomlParseError { message: String },
    /// Required field was not present.
    #[error("missing required field `{field}` in graphcal.toml")]
    MissingField { field: &'static str },
    /// Field had an unexpected TOML type.
    #[error("field `{field}` in graphcal.toml must be {expected}")]
    InvalidType {
        field: String,
        expected: &'static str,
    },
    /// A manifest table contained a field outside the supported schema.
    #[error("unsupported field `{field}` in {table}")]
    UnsupportedField { table: &'static str, field: String },
    /// Package name is invalid.
    #[error(transparent)]
    PackageName(#[from] PackageNameError),
    /// Dependency alias is invalid.
    #[error(transparent)]
    DependencyName(#[from] DependencyNameError),
    /// Git revision is not an exact full commit.
    #[error(transparent)]
    GitRev(#[from] GitRevError),
    /// Git URL is invalid.
    #[error(transparent)]
    GitUrl(#[from] GitUrlError),
    /// Source directory is not a portable package-relative directory.
    #[error("invalid source_dir `{dir}`: {reason}")]
    InvalidSourceDir {
        /// Rejected source spelling.
        dir: String,
        /// Portable-directory invariant that failed.
        reason: PackageSourceDirectoryErrorReason,
    },
    /// Unsupported or floating dependency field was present.
    #[error("unsupported dependency field `{field}` for dependency `{dependency}`")]
    UnsupportedDependencyField { dependency: String, field: String },
    /// A dependency table used the package's own name as a direct dependency
    /// alias, making self-reference ambiguous.
    #[error(
        "dependency `{dependency}` in package `{package}` is ambiguous with the package self-reference"
    )]
    SelfDependencyAlias {
        package: PackageName,
        dependency: DependencyName,
    },
}

/// Parse a Graphcal manifest from TOML text.
///
/// # Errors
///
/// Returns [`ManifestError`] for TOML, required-field, type, source-dir, or
/// dependency validation failures.
pub fn parse_manifest_str(content: &str) -> Result<PackageManifest, ManifestError> {
    let root = content
        .parse::<Table>()
        .map_err(|e| ManifestError::TomlParseError {
            message: e.to_string(),
        })?;
    reject_unknown_fields(&root, &["package", "dependencies"], "graphcal.toml")?;

    let package = match root.get("package") {
        Some(Value::Table(package)) => package,
        Some(_) => {
            return Err(ManifestError::InvalidType {
                field: "[package]".to_string(),
                expected: "a table",
            });
        }
        None => {
            return Err(ManifestError::MissingField {
                field: "[package].name",
            });
        }
    };
    reject_unknown_fields(package, &["name", "source_dir"], "[package]")?;

    let name = PackageName::new(manifest_required_string(package, "name", "[package].name")?)?;
    let source_dir_str =
        manifest_optional_string(package, "source_dir", "[package].source_dir")?.unwrap_or("src");
    let source_dir = parse_source_dir(source_dir_str)?;
    let dependencies = parse_manifest_dependencies(root.get("dependencies"), &name)?;

    Ok(PackageManifest {
        name,
        source_dir,
        dependencies,
    })
}

fn reject_unknown_fields(
    table: &Table,
    allowed: &[&str],
    table_name: &'static str,
) -> Result<(), ManifestError> {
    table
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
        .map_or(Ok(()), |field| {
            Err(ManifestError::UnsupportedField {
                table: table_name,
                field: field.clone(),
            })
        })
}

fn manifest_required_string<'a>(
    table: &'a Table,
    key: &str,
    field: &'static str,
) -> Result<&'a str, ManifestError> {
    manifest_optional_string(table, key, field)?.ok_or(ManifestError::MissingField { field })
}

fn manifest_optional_string<'a>(
    table: &'a Table,
    key: &str,
    field: &'static str,
) -> Result<Option<&'a str>, ManifestError> {
    match table.get(key) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(ManifestError::InvalidType {
            field: field.to_string(),
            expected: "a string",
        }),
        None => Ok(None),
    }
}

fn parse_source_dir(value: &str) -> Result<PackageSourceDirectory, ManifestError> {
    PackageSourceDirectory::new(value).map_err(|error| ManifestError::InvalidSourceDir {
        dir: value.to_string(),
        reason: error.reason,
    })
}

fn parse_manifest_dependencies(
    item: Option<&Value>,
    package_name: &PackageName,
) -> Result<BTreeMap<DependencyName, DependencySpec>, ManifestError> {
    let Some(item) = item else {
        return Ok(BTreeMap::new());
    };
    let Some(table) = item.as_table() else {
        return Err(ManifestError::InvalidType {
            field: "[dependencies]".to_string(),
            expected: "a table",
        });
    };

    table
        .iter()
        .map(|(key, value)| {
            let dep_name = DependencyName::new(key)?;
            if dep_name.as_str() == package_name.as_str() {
                return Err(ManifestError::SelfDependencyAlias {
                    package: package_name.clone(),
                    dependency: dep_name,
                });
            }
            let spec = parse_dependency_spec(dep_name.as_str(), value)?;
            Ok((dep_name, spec))
        })
        .collect()
}

fn parse_dependency_spec(dependency: &str, item: &Value) -> Result<DependencySpec, ManifestError> {
    let Some(table) = item.as_table() else {
        return Err(ManifestError::InvalidType {
            field: format!("[dependencies].{dependency}"),
            expected: "an inline table",
        });
    };

    for key in table.keys() {
        match key.as_str() {
            "git" | "rev" | "package" => {}
            other => {
                return Err(ManifestError::UnsupportedDependencyField {
                    dependency: dependency.to_string(),
                    field: other.to_string(),
                });
            }
        }
    }

    let git = match table.get("git") {
        Some(Value::String(url)) => GitUrl::new(url).map_err(ManifestError::GitUrl)?,
        Some(_) => {
            return Err(ManifestError::InvalidType {
                field: format!("[dependencies].{dependency}.git"),
                expected: "a string",
            });
        }
        None => {
            return Err(ManifestError::MissingField {
                field: "[dependencies].<name>.git",
            });
        }
    };
    let rev = match table.get("rev") {
        Some(Value::String(rev)) => GitCommitHash::new(rev).map_err(ManifestError::GitRev)?,
        Some(_) => {
            return Err(ManifestError::InvalidType {
                field: format!("[dependencies].{dependency}.rev"),
                expected: "a string",
            });
        }
        None => {
            return Err(ManifestError::MissingField {
                field: "[dependencies].<name>.rev",
            });
        }
    };
    let package = table
        .get("package")
        .map(|package| {
            package
                .as_str()
                .ok_or_else(|| ManifestError::InvalidType {
                    field: format!("[dependencies].{dependency}.package"),
                    expected: "a string",
                })
                .and_then(|name| PackageName::new(name).map_err(ManifestError::PackageName))
        })
        .transpose()?;

    Ok(DependencySpec {
        package,
        git: GitDependency { url: git, rev },
    })
}

/// A tool-maintained package-instance lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    /// Lockfile schema version.
    pub lock_version: u64,
    /// Tool that created the lockfile. Informational only.
    pub created_by: String,
    /// Graphcal toolchain version used for resolution.
    pub graphcal_version: String,
    /// Standard library version used for resolution.
    pub stdlib_version: String,
    /// Root package instance id.
    pub root: PackageInstanceId,
    /// Locked package instances.
    pub packages: Vec<LockedPackage>,
    /// Pinned wasm plugin artifacts of the root package.
    ///
    /// Dependency packages need no entries: a vendored `.wasm` inside a Git
    /// dependency is already covered by that package's source tree hash.
    pub plugins: Vec<LockedPlugin>,
}

/// One pinned wasm plugin artifact: the root-package-relative path exactly
/// as spelled by `import plugin "…"`, plus the SHA-256 of the file bytes.
///
/// The lockfile is the trust boundary for plugin code (issue #25): new or
/// changed plugin binaries can only enter a project through a reviewable
/// pin diff, and a mismatch at load time is a hard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPlugin {
    path: String,
    sha256: Sha256Digest,
}

impl LockedPlugin {
    /// Construct a validated plugin pin.
    ///
    /// # Errors
    ///
    /// Returns [`LockedPluginError`] when `path` is not a plain relative
    /// `.wasm` path (no `.`/`..`/absolute/prefix components, `/` separators
    /// only) or `sha256` is not 64 lowercase hex digits.
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "deliberately matches the loader's case-sensitive `.wasm` plugin-path classification"
    )]
    pub fn new(
        path: impl Into<String>,
        sha256: impl Into<String>,
    ) -> Result<Self, LockedPluginError> {
        let path = path.into();
        let plain_relative = !path.contains('\\')
            && Path::new(&path)
                .components()
                .any(|component| matches!(component, Component::Normal(_)))
            && Path::new(&path)
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if !plain_relative || !path.ends_with(".wasm") {
            return Err(LockedPluginError::InvalidPath { path });
        }
        let sha256 = Sha256Digest::new(sha256).map_err(LockedPluginError::InvalidSha256)?;
        Ok(Self { path, sha256 })
    }

    /// The root-package-relative plugin path, exactly as imported.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Lowercase-hex SHA-256 of the plugin file bytes.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

/// Validation error from [`LockedPlugin::new`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LockedPluginError {
    /// The path is not a plain relative `.wasm` path.
    #[error(
        "invalid plugin path `{path}`: must be a plain relative path to a `.wasm` file inside the package root"
    )]
    InvalidPath {
        /// The rejected path.
        path: String,
    },
    /// The digest is not 64 lowercase hex digits.
    #[error("invalid plugin sha256: {0}")]
    InvalidSha256(Sha256DigestError),
}

impl Lockfile {
    /// Validate this lockfile's graph invariants.
    ///
    /// # Errors
    ///
    /// Returns [`LockValidationError`] when the lockfile is structurally
    /// invalid or incompatible with the provided active versions.
    pub fn validate(
        &self,
        active_graphcal_version: &str,
        active_stdlib_version: &str,
    ) -> Result<(), LockValidationError> {
        if self.lock_version != LOCK_VERSION {
            return Err(LockValidationError::UnsupportedLockVersion {
                version: self.lock_version,
            });
        }
        if self.graphcal_version != active_graphcal_version {
            return Err(LockValidationError::GraphcalVersionMismatch {
                lockfile: self.graphcal_version.clone(),
                active: active_graphcal_version.to_string(),
            });
        }
        if self.stdlib_version != active_stdlib_version {
            return Err(LockValidationError::StdlibVersionMismatch {
                lockfile: self.stdlib_version.clone(),
                active: active_stdlib_version.to_string(),
            });
        }

        let mut ids = BTreeSet::new();
        for package in &self.packages {
            if !ids.insert(package.id.clone()) {
                return Err(LockValidationError::DuplicatePackageId {
                    id: package.id.clone(),
                });
            }
            if package
                .dependencies
                .contains_key(&DependencyName(package.name.as_str().to_string()))
            {
                return Err(LockValidationError::SelfDependencyAlias {
                    package: package.id.clone(),
                    dependency: DependencyName(package.name.as_str().to_string()),
                });
            }
        }
        if !ids.contains(&self.root) {
            return Err(LockValidationError::MissingRoot {
                root: self.root.clone(),
            });
        }
        for package in &self.packages {
            for (dependency, target) in &package.dependencies {
                if !ids.contains(target) {
                    return Err(LockValidationError::MissingDependencyTarget {
                        package: package.id.clone(),
                        dependency: dependency.clone(),
                        target: target.clone(),
                    });
                }
            }
        }
        self.reject_dependency_cycles()?;
        self.reject_conflicting_git_source_hashes()?;
        self.reject_duplicate_canonical_instances()?;

        let mut plugin_paths = BTreeSet::new();
        for plugin in &self.plugins {
            if !plugin_paths.insert(plugin.path.as_str()) {
                return Err(LockValidationError::DuplicatePluginPath {
                    path: plugin.path.clone(),
                });
            }
        }
        Ok(())
    }

    fn reject_dependency_cycles(&self) -> Result<(), LockValidationError> {
        let packages = self
            .packages
            .iter()
            .map(|package| (&package.id, package))
            .collect::<BTreeMap<_, _>>();
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for package in &self.packages {
            visit_lock_package_for_cycles(&package.id, &packages, &mut visiting, &mut visited)?;
        }
        Ok(())
    }

    fn reject_conflicting_git_source_hashes(&self) -> Result<(), LockValidationError> {
        let mut sources =
            BTreeMap::<CanonicalGitSource, (&Sha256Digest, &PackageInstanceId)>::new();
        for package in &self.packages {
            let PackageSource::Git {
                url,
                commit,
                tree_hashes,
                ..
            } = &package.source
            else {
                continue;
            };
            let source = CanonicalGitSource {
                url: url.clone(),
                commit: commit.clone(),
            };
            match sources.get(&source) {
                Some((expected, first)) if *expected != &tree_hashes.sha256 => {
                    return Err(LockValidationError::ConflictingGitSourceDigest(Box::new(
                        GitSourceDigestConflict {
                            first: (*first).clone(),
                            second: package.id.clone(),
                            url: url.clone(),
                            commit: commit.clone(),
                            first_digest: **expected,
                            second_digest: tree_hashes.sha256,
                        },
                    )));
                }
                Some(_) => {}
                None => {
                    sources.insert(source, (&tree_hashes.sha256, &package.id));
                }
            }
        }
        Ok(())
    }

    fn reject_duplicate_canonical_instances(&self) -> Result<(), LockValidationError> {
        let mut canonical = BTreeMap::<CanonicalPackageInstance, &PackageInstanceId>::new();
        for package in &self.packages {
            let key = CanonicalPackageInstance {
                name: package.name.clone(),
                source_dir: package.source_dir.clone(),
                source: package.source.canonical_key(),
                dependencies: package.dependencies.clone(),
            };
            if let Some(existing) = canonical.insert(key, &package.id) {
                return Err(LockValidationError::DuplicateCanonicalPackage {
                    first: existing.clone(),
                    second: package.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// Build a contextual package graph from this already validated lockfile.
    ///
    /// # Errors
    ///
    /// Returns [`LockValidationError`] if the lockfile is structurally invalid
    /// for the provided active versions.
    pub fn package_graph(
        &self,
        active_graphcal_version: &str,
        active_stdlib_version: &str,
    ) -> Result<PackageGraph, LockValidationError> {
        self.validate(active_graphcal_version, active_stdlib_version)?;
        let packages = self
            .packages
            .iter()
            .cloned()
            .map(|package| (package.id.clone(), package))
            .collect();
        Ok(PackageGraph {
            root: self.root.clone(),
            packages,
        })
    }

    /// Serialize the lockfile in deterministic TOML form.
    #[must_use]
    pub fn to_deterministic_toml(&self) -> String {
        let mut out = String::new();
        push_kv_u64(&mut out, "lock_version", self.lock_version);
        push_kv_string(&mut out, "created_by", &self.created_by);
        push_kv_string(&mut out, "graphcal_version", &self.graphcal_version);
        push_kv_string(&mut out, "stdlib_version", &self.stdlib_version);
        push_kv_string(&mut out, "root", self.root.as_str());

        let mut packages = self.packages.clone();
        packages.sort_by(|a, b| a.id.cmp(&b.id));
        for package in packages {
            out.push_str("\n[[package]]\n");
            push_kv_string(&mut out, "id", package.id.as_str());
            push_kv_string(&mut out, "name", package.name.as_str());
            push_kv_string(&mut out, "source_dir", &package.source_dir.to_string());
            out.push_str("\n[package.source]\n");
            match &package.source {
                PackageSource::Path { path } => {
                    push_kv_string(&mut out, "type", "path");
                    push_kv_string(&mut out, "path", path);
                }
                PackageSource::Git {
                    url,
                    requested_rev,
                    commit,
                    tree_hashes,
                } => {
                    push_kv_string(&mut out, "type", "git");
                    push_kv_string(&mut out, "url", &url.to_string());
                    push_kv_string(&mut out, "requested_rev", requested_rev.as_str());
                    push_kv_string(&mut out, "commit", commit.as_str());
                    push_kv_inline_table(
                        &mut out,
                        "tree_hashes",
                        &[("sha256", &tree_hashes.sha256.to_string())],
                    );
                }
            }
            if !package.dependencies.is_empty() {
                out.push_str("\n[package.dependencies]\n");
                for (name, target) in package.dependencies {
                    push_kv_string(&mut out, name.as_str(), target.as_str());
                }
            }
        }

        let mut plugins = self.plugins.clone();
        plugins.sort_by(|a, b| a.path.cmp(&b.path));
        for plugin in plugins {
            out.push_str("\n[[plugin]]\n");
            push_kv_string(&mut out, "path", plugin.path());
            push_kv_string(&mut out, "sha256", &plugin.sha256().to_string());
        }
        out
    }
}

fn visit_lock_package_for_cycles<'a>(
    id: &'a PackageInstanceId,
    packages: &BTreeMap<&'a PackageInstanceId, &'a LockedPackage>,
    visiting: &mut BTreeSet<&'a PackageInstanceId>,
    visited: &mut BTreeSet<&'a PackageInstanceId>,
) -> Result<(), LockValidationError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return Err(LockValidationError::DependencyCycle {
            package: id.clone(),
        });
    }
    if let Some(package) = packages.get(id) {
        for target in package.dependencies.values() {
            visit_lock_package_for_cycles(target, packages, visiting, visited)?;
        }
    }
    visiting.remove(id);
    visited.insert(id);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalPackageInstance {
    name: PackageName,
    source_dir: PackageSourceDirectory,
    source: CanonicalPackageSource,
    dependencies: BTreeMap<DependencyName, PackageInstanceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    /// Opaque package instance id.
    pub id: PackageInstanceId,
    /// Locked real package name.
    pub name: PackageName,
    /// Locked source directory.
    pub source_dir: PackageSourceDirectory,
    /// Source metadata.
    pub source: PackageSource,
    /// Direct dependency edges keyed by local aliases.
    pub dependencies: BTreeMap<DependencyName, PackageInstanceId>,
}

/// Source table for one locked package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    /// Root path source.
    Path {
        /// Root path as written to the lockfile. Only `"."` is required for
        /// the MVP root package.
        path: String,
    },
    /// Git source locked to an immutable commit and normalized tree hash.
    Git {
        /// Repository URL.
        url: GitUrl,
        /// Manifest-requested revision.
        requested_rev: GitCommitHash,
        /// Immutable fetched commit. The MVP requires this to equal
        /// `requested_rev`.
        commit: GitCommitHash,
        /// Normalized source tree hashes.
        tree_hashes: SourceTreeHashes,
    },
}

impl PackageSource {
    fn canonical_key(&self) -> CanonicalPackageSource {
        match self {
            Self::Path { path } => CanonicalPackageSource::Path { path: path.clone() },
            Self::Git { url, commit, .. } => CanonicalPackageSource::Git {
                url: url.clone(),
                commit: commit.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonicalPackageSource {
    Path { path: String },
    Git { url: GitUrl, commit: GitCommitHash },
}

/// Hashes for a locked Git source tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTreeHashes {
    /// SHA-256 digest of the normalized source tree.
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalGitSource {
    url: GitUrl,
    commit: GitCommitHash,
}

/// Lockfile validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LockValidationError {
    /// Unsupported lock schema.
    #[error("unsupported graphcal.lock version {version}")]
    UnsupportedLockVersion { version: u64 },
    /// Active Graphcal toolchain version differs from the lockfile.
    #[error("graphcal.lock was resolved with Graphcal {lockfile}, active version is {active}")]
    GraphcalVersionMismatch { lockfile: String, active: String },
    /// Active stdlib version differs from the lockfile.
    #[error("graphcal.lock was resolved with stdlib {lockfile}, active version is {active}")]
    StdlibVersionMismatch { lockfile: String, active: String },
    /// Two packages used the same opaque id.
    #[error("duplicate package instance id `{id}` in graphcal.lock")]
    DuplicatePackageId { id: PackageInstanceId },
    /// Root id does not appear in package entries.
    #[error("graphcal.lock root `{root}` does not match any package entry")]
    MissingRoot { root: PackageInstanceId },
    /// A dependency edge points to a missing package instance.
    #[error("package `{package}` dependency `{dependency}` points to missing `{target}`")]
    MissingDependencyTarget {
        package: PackageInstanceId,
        dependency: DependencyName,
        target: PackageInstanceId,
    },
    /// A dependency alias conflicts with package self-reference syntax.
    #[error("package `{package}` dependency `{dependency}` conflicts with self-reference")]
    SelfDependencyAlias {
        package: PackageInstanceId,
        dependency: DependencyName,
    },
    /// The locked dependency graph contains a cycle.
    #[error("dependency cycle involving package `{package}` in graphcal.lock")]
    DependencyCycle { package: PackageInstanceId },
    /// The same immutable Git source is pinned to contradictory tree digests.
    #[error(transparent)]
    ConflictingGitSourceDigest(Box<GitSourceDigestConflict>),
    /// Two ids describe the same canonical package instance.
    #[error("package ids `{first}` and `{second}` describe the same canonical package instance")]
    DuplicateCanonicalPackage {
        first: PackageInstanceId,
        second: PackageInstanceId,
    },
    /// Two plugin pins name the same path.
    #[error("duplicate plugin pin for `{path}` in graphcal.lock")]
    DuplicatePluginPath {
        /// The duplicated plugin path.
        path: String,
    },
    /// A locked package has no materialized manifest to validate against.
    #[error("locked package `{package}` has no materialized graphcal.toml manifest")]
    MissingPackageManifest { package: PackageInstanceId },
    /// A supplied manifest does not correspond to any locked package.
    #[error("manifest for package `{package}` has no graphcal.lock package entry")]
    ManifestWithoutLockedPackage { package: PackageInstanceId },
    /// A lock entry and its materialized manifest disagree on package identity.
    #[error(
        "locked package `{package}` is named `{locked}`, but its manifest is named `{manifest}`"
    )]
    LockedManifestNameMismatch {
        package: PackageInstanceId,
        locked: PackageName,
        manifest: PackageName,
    },
    /// A lock entry and its materialized manifest select different source trees.
    #[error(
        "locked package `{package}` uses source_dir `{locked}`, but its manifest uses `{manifest}`"
    )]
    LockedManifestSourceDirectoryMismatch {
        package: PackageInstanceId,
        locked: PackageSourceDirectory,
        manifest: PackageSourceDirectory,
    },
    /// A lock edge was not declared by the importing manifest.
    #[error("package `{package}` dependency `{dependency}` is not declared in its manifest")]
    UndeclaredDependencyEdge {
        package: PackageInstanceId,
        dependency: DependencyName,
    },
    /// A manifest dependency has no corresponding lock edge.
    #[error("package `{package}` manifest dependency `{dependency}` is missing from graphcal.lock")]
    MissingDependencyEdge {
        package: PackageInstanceId,
        dependency: DependencyName,
    },
    /// A locked target package name does not satisfy its manifest dependency
    /// entry.
    #[error(
        "package `{package}` dependency `{dependency}` expected package `{expected}` but lock target `{target}` is `{actual}`"
    )]
    PackageNameMismatch {
        package: PackageInstanceId,
        dependency: DependencyName,
        expected: PackageName,
        target: PackageInstanceId,
        actual: PackageName,
    },
    /// A lock edge targets a Git source that does not match the importing
    /// manifest dependency spec.
    #[error("package `{package}` dependency `{dependency}` does not match its manifest Git source")]
    DependencySourceMismatch {
        package: PackageInstanceId,
        dependency: DependencyName,
    },
}

/// Validate and bind a lockfile to every materialized package manifest.
///
/// # Errors
///
/// Returns [`LockValidationError`] unless lock entries and manifests form a
/// total bijection with identical package names, source directories, and
/// dependency alias sets, and every dependency target satisfies its manifest
/// source contract.
pub fn validate_lock_against_manifests(
    lockfile: &Lockfile,
    manifests: &BTreeMap<PackageInstanceId, PackageManifest>,
    active_graphcal_version: &str,
    active_stdlib_version: &str,
) -> Result<ValidatedPackageGraph, LockValidationError> {
    let graph = lockfile.package_graph(active_graphcal_version, active_stdlib_version)?;

    for package in manifests.keys() {
        if graph.package(package).is_none() {
            return Err(LockValidationError::ManifestWithoutLockedPackage {
                package: package.clone(),
            });
        }
    }
    let matched_packages = lockfile
        .packages
        .iter()
        .map(|package| {
            let manifest = manifests.get(&package.id).ok_or_else(|| {
                LockValidationError::MissingPackageManifest {
                    package: package.id.clone(),
                }
            })?;
            if package.name != manifest.name {
                return Err(LockValidationError::LockedManifestNameMismatch {
                    package: package.id.clone(),
                    locked: package.name.clone(),
                    manifest: manifest.name.clone(),
                });
            }
            if package.source_dir != manifest.source_dir {
                return Err(LockValidationError::LockedManifestSourceDirectoryMismatch {
                    package: package.id.clone(),
                    locked: package.source_dir.clone(),
                    manifest: manifest.source_dir.clone(),
                });
            }
            Ok((package, manifest))
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (package, manifest) in matched_packages {
        for dependency in package.dependencies.keys() {
            if !manifest.dependencies.contains_key(dependency) {
                return Err(LockValidationError::UndeclaredDependencyEdge {
                    package: package.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
        for (dependency, spec) in &manifest.dependencies {
            let target_id = package.dependencies.get(dependency).ok_or_else(|| {
                LockValidationError::MissingDependencyEdge {
                    package: package.id.clone(),
                    dependency: dependency.clone(),
                }
            })?;
            let target = graph.package(target_id).ok_or_else(|| {
                LockValidationError::MissingDependencyTarget {
                    package: package.id.clone(),
                    dependency: dependency.clone(),
                    target: target_id.clone(),
                }
            })?;
            let expected = spec.expected_package_name(dependency);
            if target.name != expected {
                return Err(LockValidationError::PackageNameMismatch {
                    package: package.id.clone(),
                    dependency: dependency.clone(),
                    expected,
                    target: target.id.clone(),
                    actual: target.name.clone(),
                });
            }
            match &target.source {
                PackageSource::Git {
                    url,
                    requested_rev,
                    commit,
                    ..
                } if url == &spec.git.url
                    && requested_rev == &spec.git.rev
                    && commit == &spec.git.rev => {}
                PackageSource::Git { .. } | PackageSource::Path { .. } => {
                    return Err(LockValidationError::DependencySourceMismatch {
                        package: package.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
    }

    Ok(ValidatedPackageGraph {
        graph,
        manifests: manifests.clone(),
    })
}

/// A package graph whose lock entries are totally matched to materialized
/// manifests.
///
/// Construction is only available through [`validate_lock_against_manifests`],
/// so consumers cannot resolve modules through source metadata that differs
/// from the manifest used for integrity verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPackageGraph {
    graph: PackageGraph,
    manifests: BTreeMap<PackageInstanceId, PackageManifest>,
}

impl ValidatedPackageGraph {
    /// Root package instance id.
    #[must_use]
    pub const fn root(&self) -> &PackageInstanceId {
        self.graph.root()
    }

    /// Look up a locked package that has a matching manifest.
    #[must_use]
    pub fn package(&self, id: &PackageInstanceId) -> Option<&LockedPackage> {
        self.graph.package(id)
    }

    /// Look up the manifest bound to a locked package.
    #[must_use]
    pub fn manifest(&self, id: &PackageInstanceId) -> Option<&PackageManifest> {
        self.manifests.get(id)
    }

    /// Resolve a module path through the validated contextual lock graph.
    ///
    /// # Errors
    ///
    /// Returns [`PackageResolveError`] under the same conditions as
    /// [`PackageGraph::resolve_module_path`].
    pub fn resolve_module_path(
        &self,
        current: &PackageInstanceId,
        segments: &[String],
    ) -> Result<ResolvedPackageModule, PackageResolveError> {
        self.graph.resolve_module_path(current, segments)
    }
}

/// Contextual package-instance graph consumed by package-aware loaders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraph {
    root: PackageInstanceId,
    packages: BTreeMap<PackageInstanceId, LockedPackage>,
}

impl PackageGraph {
    /// Root package instance id.
    #[must_use]
    pub const fn root(&self) -> &PackageInstanceId {
        &self.root
    }

    /// Look up a locked package by id.
    #[must_use]
    pub fn package(&self, id: &PackageInstanceId) -> Option<&LockedPackage> {
        self.packages.get(id)
    }

    /// Resolve a module path in the dependency namespace of `current`.
    ///
    /// # Errors
    ///
    /// Returns [`PackageResolveError`] if the current package id is missing,
    /// the path is empty, the first segment is unknown, or an ambiguity is
    /// detected.
    pub fn resolve_module_path(
        &self,
        current: &PackageInstanceId,
        segments: &[String],
    ) -> Result<ResolvedPackageModule, PackageResolveError> {
        let current_package = self.packages.get(current).ok_or_else(|| {
            PackageResolveError::UnknownCurrentPackage {
                package: current.clone(),
            }
        })?;
        let [first, rest @ ..] = segments else {
            return Err(PackageResolveError::EmptyPath {
                package: current.clone(),
            });
        };
        if first == current_package.name.as_str() {
            if current_package
                .dependencies
                .contains_key(&DependencyName(first.clone()))
            {
                return Err(PackageResolveError::SelfReferenceAmbiguity {
                    package: current.clone(),
                    name: first.clone(),
                });
            }
            return Ok(ResolvedPackageModule {
                package: current.clone(),
                module_segments: rest.to_vec(),
                relation: PackageResolutionRelation::SelfReference,
            });
        }
        let dependency = DependencyName::new(first.clone()).map_err(|_| {
            PackageResolveError::UnknownDependency {
                package: current.clone(),
                name: first.clone(),
            }
        })?;
        let target = current_package
            .dependencies
            .get(&dependency)
            .ok_or_else(|| PackageResolveError::UnknownDependency {
                package: current.clone(),
                name: first.clone(),
            })?;
        Ok(ResolvedPackageModule {
            package: target.clone(),
            module_segments: rest.to_vec(),
            relation: PackageResolutionRelation::Dependency { name: dependency },
        })
    }
}

/// Result of resolving the first module segment through a package instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackageModule {
    /// Package instance that owns the remaining module path.
    pub package: PackageInstanceId,
    /// Module path under the selected package's `source_dir`, excluding the
    /// self/dependency selector segment.
    pub module_segments: Vec<String>,
    /// How the first segment resolved.
    relation: PackageResolutionRelation,
}

/// First-segment package resolution relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageResolutionRelation {
    /// First segment matched the current package's own real name.
    SelfReference,
    /// First segment matched a direct dependency alias of the current package.
    Dependency {
        /// Local dependency alias.
        name: DependencyName,
    },
}

/// Package module resolution error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageResolveError {
    /// The requested current package id does not exist in the graph.
    #[error("unknown current package instance `{package}`")]
    UnknownCurrentPackage { package: PackageInstanceId },
    /// The module path had no first segment.
    #[error("empty module path in package `{package}`")]
    EmptyPath { package: PackageInstanceId },
    /// The first segment is neither self-reference nor direct dependency alias.
    #[error("unknown dependency `{name}` in package `{package}`")]
    UnknownDependency {
        package: PackageInstanceId,
        name: String,
    },
    /// A dependency alias conflicts with the current package self-reference.
    #[error("package `{package}` has ambiguous self-reference/dependency name `{name}`")]
    SelfReferenceAmbiguity {
        package: PackageInstanceId,
        name: String,
    },
}

/// Lockfile parse error.
#[derive(Debug, Error)]
pub enum LockfileParseError {
    /// TOML syntax failed.
    #[error("invalid TOML in graphcal.lock: {message}")]
    TomlParseError { message: String },
    /// Required field was not present.
    #[error("missing required field `{field}` in graphcal.lock")]
    MissingField { field: &'static str },
    /// Field had an unexpected TOML type.
    #[error("field `{field}` in graphcal.lock must be {expected}")]
    InvalidType {
        field: String,
        expected: &'static str,
    },
    /// A table contains a field outside the current lock schema.
    #[error("unknown field `{field}` in `{table}` in graphcal.lock")]
    UnknownField { table: String, field: String },
    /// Package name validation failed.
    #[error(transparent)]
    PackageName(#[from] PackageNameError),
    /// Dependency name validation failed.
    #[error(transparent)]
    DependencyName(#[from] DependencyNameError),
    /// Package instance id validation failed.
    #[error(transparent)]
    PackageInstanceId(#[from] PackageInstanceIdError),
    /// Git revision validation failed.
    #[error(transparent)]
    GitRev(#[from] GitRevError),
    /// Git URL validation failed.
    #[error(transparent)]
    GitUrl(#[from] GitUrlError),
    /// Source directory is not a portable package-relative directory.
    #[error("invalid source_dir `{dir}` in graphcal.lock: {reason}")]
    InvalidSourceDir {
        /// Rejected source spelling.
        dir: String,
        /// Portable-directory invariant that failed.
        reason: PackageSourceDirectoryErrorReason,
    },
    /// Package source type was not recognized.
    #[error("unsupported package source type `{source_type}` in graphcal.lock")]
    UnsupportedSourceType { source_type: String },
    /// Source-tree digest validation failed.
    #[error(transparent)]
    Sha256Digest(#[from] Sha256DigestError),
    /// Plugin pin validation failed.
    #[error(transparent)]
    Plugin(#[from] LockedPluginError),
}

/// Parse a lockfile from TOML text.
///
/// # Errors
///
/// Returns [`LockfileParseError`] for TOML, type, and field validation errors.
pub fn parse_lockfile_str(content: &str) -> Result<Lockfile, LockfileParseError> {
    let root = content
        .parse::<Table>()
        .map_err(|e| LockfileParseError::TomlParseError {
            message: e.to_string(),
        })?;

    let lock_version = required_u64(root.get("lock_version"), "lock_version")?;
    let strict_schema = lock_version == LOCK_VERSION;
    reject_unknown_lock_fields(
        &root,
        "root",
        &[
            "lock_version",
            "created_by",
            "graphcal_version",
            "stdlib_version",
            "root",
            "package",
            "plugin",
        ],
        strict_schema,
    )?;
    let created_by = required_string(root.get("created_by"), "created_by")?.to_string();
    let graphcal_version =
        required_string(root.get("graphcal_version"), "graphcal_version")?.to_string();
    let stdlib_version = required_string(root.get("stdlib_version"), "stdlib_version")?.to_string();
    let root_id = PackageInstanceId::new(required_string(root.get("root"), "root")?)?;
    let package_array = root
        .get("package")
        .and_then(Value::as_array)
        .ok_or(LockfileParseError::MissingField { field: "package" })?;
    let packages = package_array
        .iter()
        .enumerate()
        .map(|(index, package)| parse_locked_package(index, package, strict_schema))
        .collect::<Result<Vec<_>, _>>()?;
    let plugins = match root.get("plugin") {
        Some(plugin) => parse_locked_plugins(plugin, strict_schema)?,
        None => Vec::new(),
    };

    Ok(Lockfile {
        lock_version,
        created_by,
        graphcal_version,
        stdlib_version,
        root: root_id,
        packages,
        plugins,
    })
}

fn parse_locked_plugins(
    item: &Value,
    strict_schema: bool,
) -> Result<Vec<LockedPlugin>, LockfileParseError> {
    let Some(array) = item.as_array() else {
        return Err(LockfileParseError::InvalidType {
            field: "plugin".to_string(),
            expected: "an array of tables",
        });
    };
    array
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let Some(table) = entry.as_table() else {
                return Err(LockfileParseError::InvalidType {
                    field: format!("plugin[{index}]"),
                    expected: "a table",
                });
            };
            reject_unknown_lock_fields(
                table,
                &format!("plugin[{index}]"),
                &["path", "sha256"],
                strict_schema,
            )?;
            let path = required_string(table.get("path"), "plugin.path")?;
            let sha256 = required_string(table.get("sha256"), "plugin.sha256")?;
            Ok(LockedPlugin::new(path, sha256)?)
        })
        .collect()
}

fn parse_locked_package(
    index: usize,
    item: &Value,
    strict_schema: bool,
) -> Result<LockedPackage, LockfileParseError> {
    let Some(table) = item.as_table() else {
        return Err(LockfileParseError::InvalidType {
            field: format!("package[{index}]"),
            expected: "a table",
        });
    };
    reject_unknown_lock_fields(
        table,
        &format!("package[{index}]"),
        &["id", "name", "source_dir", "source", "dependencies"],
        strict_schema,
    )?;
    let id = PackageInstanceId::new(required_string(table.get("id"), "package.id")?)?;
    let name = PackageName::new(required_string(table.get("name"), "package.name")?)?;
    let source_dir = parse_lock_source_dir(required_string(
        table.get("source_dir"),
        "package.source_dir",
    )?)?;
    let source_table =
        table
            .get("source")
            .and_then(Value::as_table)
            .ok_or(LockfileParseError::MissingField {
                field: "package.source",
            })?;
    let source = parse_package_source(source_table, strict_schema)?;
    let dependencies = match table.get("dependencies") {
        Some(dependencies) => parse_package_dependencies(dependencies)?,
        None => BTreeMap::new(),
    };

    Ok(LockedPackage {
        id,
        name,
        source_dir,
        source,
        dependencies,
    })
}

fn parse_lock_source_dir(value: &str) -> Result<PackageSourceDirectory, LockfileParseError> {
    PackageSourceDirectory::new(value).map_err(|error| LockfileParseError::InvalidSourceDir {
        dir: value.to_string(),
        reason: error.reason,
    })
}

fn parse_package_source(
    table: &Table,
    strict_schema: bool,
) -> Result<PackageSource, LockfileParseError> {
    let source_type = required_string(table.get("type"), "package.source.type")?;
    match source_type {
        "path" => {
            reject_unknown_lock_fields(table, "package.source", &["type", "path"], strict_schema)?;
            Ok(PackageSource::Path {
                path: required_string(table.get("path"), "package.source.path")?.to_string(),
            })
        }
        "git" => {
            reject_unknown_lock_fields(
                table,
                "package.source",
                &["type", "url", "requested_rev", "commit", "tree_hashes"],
                strict_schema,
            )?;
            let url = GitUrl::new(required_string(table.get("url"), "package.source.url")?)?;
            let requested_rev = GitCommitHash::new(required_string(
                table.get("requested_rev"),
                "package.source.requested_rev",
            )?)?;
            let commit = GitCommitHash::new(required_string(
                table.get("commit"),
                "package.source.commit",
            )?)?;
            let hashes = table.get("tree_hashes").and_then(Value::as_table).ok_or(
                LockfileParseError::MissingField {
                    field: "package.source.tree_hashes",
                },
            )?;
            reject_unknown_lock_fields(
                hashes,
                "package.source.tree_hashes",
                &["sha256"],
                strict_schema,
            )?;
            let sha256 = Sha256Digest::new(required_string(
                hashes.get("sha256"),
                "package.source.tree_hashes.sha256",
            )?)?;
            Ok(PackageSource::Git {
                url,
                requested_rev,
                commit,
                tree_hashes: SourceTreeHashes { sha256 },
            })
        }
        other => Err(LockfileParseError::UnsupportedSourceType {
            source_type: other.to_string(),
        }),
    }
}

fn parse_package_dependencies(
    item: &Value,
) -> Result<BTreeMap<DependencyName, PackageInstanceId>, LockfileParseError> {
    let Some(table) = item.as_table() else {
        return Err(LockfileParseError::InvalidType {
            field: "package.dependencies".to_string(),
            expected: "a table",
        });
    };
    table
        .iter()
        .map(|(key, value)| {
            let name = DependencyName::new(key)?;
            let target = PackageInstanceId::new(value.as_str().ok_or_else(|| {
                LockfileParseError::InvalidType {
                    field: format!("package.dependencies.{key}"),
                    expected: "a string",
                }
            })?)?;
            Ok((name, target))
        })
        .collect()
}

/// Details of contradictory hashes pinned for one canonical Git source.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "package ids `{first}` and `{second}` pin Git source `{url}` at `{commit}` to conflicting SHA-256 digests `{first_digest}` and `{second_digest}`"
)]
pub struct GitSourceDigestConflict {
    /// First package instance using the source.
    pub first: PackageInstanceId,
    /// Second package instance using the source.
    pub second: PackageInstanceId,
    /// Canonical remote repository URL.
    pub url: GitUrl,
    /// Immutable Git commit.
    pub commit: GitCommitHash,
    /// Digest pinned by the first package.
    pub first_digest: Sha256Digest,
    /// Contradictory digest pinned by the second package.
    pub second_digest: Sha256Digest,
}

fn reject_unknown_lock_fields(
    table: &Table,
    table_name: &str,
    allowed: &[&str],
    strict_schema: bool,
) -> Result<(), LockfileParseError> {
    if !strict_schema {
        return Ok(());
    }
    table
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
        .map_or(Ok(()), |field| {
            Err(LockfileParseError::UnknownField {
                table: table_name.to_string(),
                field: field.clone(),
            })
        })
}

fn required_string<'a>(
    item: Option<&'a Value>,
    field: &'static str,
) -> Result<&'a str, LockfileParseError> {
    let item = item.ok_or(LockfileParseError::MissingField { field })?;
    item.as_str()
        .ok_or_else(|| LockfileParseError::InvalidType {
            field: field.to_string(),
            expected: "a string",
        })
}

fn required_u64(item: Option<&Value>, field: &'static str) -> Result<u64, LockfileParseError> {
    let item = item.ok_or(LockfileParseError::MissingField { field })?;
    item.as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| LockfileParseError::InvalidType {
            field: field.to_string(),
            expected: "an integer",
        })
}

fn push_kv_string(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    push_escaped_string(out, value);
    out.push_str("\"\n");
}

fn push_kv_u64(out: &mut String, key: &str, value: u64) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(&value.to_string());
    out.push('\n');
}

fn push_kv_inline_table(out: &mut String, key: &str, pairs: &[(&str, &str)]) {
    out.push_str(key);
    out.push_str(" = { ");
    for (idx, (name, value)) in pairs.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(name);
        out.push_str(" = \"");
        push_escaped_string(out, value);
        out.push('"');
    }
    out.push_str(" }\n");
}

fn push_escaped_string(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            other if other.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04X}", other as u32);
            }
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAPHCAL_VERSION: &str = STDLIB_VERSION;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn dep(name: &str) -> DependencyName {
        DependencyName::new(name).unwrap()
    }

    fn id(value: &str) -> PackageInstanceId {
        PackageInstanceId::new(value).unwrap()
    }

    fn hash(value: char) -> GitCommitHash {
        GitCommitHash::new(value.to_string().repeat(40)).unwrap()
    }

    fn url(value: &str) -> GitUrl {
        GitUrl::new(value).unwrap()
    }

    fn source_dir(value: &str) -> PackageSourceDirectory {
        PackageSourceDirectory::new(value).unwrap()
    }

    fn digest(value: char) -> Sha256Digest {
        Sha256Digest::new(value.to_string().repeat(64)).unwrap()
    }

    fn git_source(url_text: &str, rev_char: char) -> PackageSource {
        git_source_with_digest(url_text, rev_char, rev_char)
    }

    fn git_source_with_digest(url_text: &str, rev_char: char, digest_char: char) -> PackageSource {
        PackageSource::Git {
            url: url(url_text),
            requested_rev: hash(rev_char),
            commit: hash(rev_char),
            tree_hashes: SourceTreeHashes {
                sha256: digest(digest_char),
            },
        }
    }

    fn path_source() -> PackageSource {
        PackageSource::Path {
            path: ".".to_string(),
        }
    }

    fn package(
        id_text: &str,
        name: &str,
        source: PackageSource,
        dependencies: BTreeMap<DependencyName, PackageInstanceId>,
    ) -> LockedPackage {
        LockedPackage {
            id: id(id_text),
            name: pkg(name),
            source_dir: source_dir("src"),
            source,
            dependencies,
        }
    }

    fn lockfile(packages: Vec<LockedPackage>) -> Lockfile {
        Lockfile {
            lock_version: LOCK_VERSION,
            created_by: "graphcal".to_string(),
            graphcal_version: GRAPHICAL_VERSION.to_string(),
            stdlib_version: STDLIB_VERSION.to_string(),
            root: id("pkg-mission"),
            plugins: Vec::new(),
            packages,
        }
    }

    fn matched_lock_and_manifests() -> (Lockfile, BTreeMap<PackageInstanceId, PackageManifest>) {
        let root_dependencies = BTreeMap::from([(dep("units"), id("pkg-units"))]);
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_dependencies),
            package(
                "pkg-units",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ]);
        let dependency_spec = DependencySpec {
            package: None,
            git: GitDependency {
                url: url("https://github.com/acme/units.git"),
                rev: hash('1'),
            },
        };
        let manifests = BTreeMap::from([
            (
                id("pkg-mission"),
                PackageManifest {
                    name: pkg("mission"),
                    source_dir: source_dir("src"),
                    dependencies: BTreeMap::from([(dep("units"), dependency_spec)]),
                },
            ),
            (
                id("pkg-units"),
                PackageManifest {
                    name: pkg("units"),
                    source_dir: source_dir("src"),
                    dependencies: BTreeMap::new(),
                },
            ),
        ]);
        (lock, manifests)
    }

    const GRAPHICAL_VERSION: &str = GRAPHCAL_VERSION;

    #[test]
    fn manifest_accepts_exact_rev_git_dependency_and_alias() {
        let manifest = parse_manifest_str(
            r#"
[package]
name = "mission"
source_dir = "src"

[dependencies]
units_v1 = { package = "units", git = "https://github.com/acme/units.git", rev = "1111111111111111111111111111111111111111" }
"#,
        )
        .unwrap();

        assert_eq!(manifest.name, pkg("mission"));
        let spec = manifest.dependencies.get(&dep("units_v1")).unwrap();
        assert_eq!(spec.expected_package_name(&dep("units_v1")), pkg("units"));
        assert_eq!(spec.git.rev, hash('1'));
    }

    #[test]
    fn source_directories_are_portable_non_empty_component_paths() {
        let nested = PackageSourceDirectory::new("source/generated").unwrap();
        assert_eq!(nested.to_string(), "source/generated");
        assert_eq!(
            nested.join_to(Path::new("package")),
            PathBuf::from("package/source/generated")
        );

        for invalid in [
            "",
            ".",
            "..",
            "/src",
            "src/",
            "src//generated",
            "src/./generated",
            "src/../outside",
            "src\\..\\outside",
            "C:/src",
            "src\ngenerated",
        ] {
            assert!(
                PackageSourceDirectory::new(invalid).is_err(),
                "`{invalid}` must not have host-dependent path semantics"
            );
        }
    }

    #[test]
    fn manifest_and_lock_reject_non_portable_source_directories() {
        let manifest = "[package]\nname = \"mission\"\nsource_dir = \"src\\\\..\\\\outside\"\n";
        assert!(matches!(
            parse_manifest_str(manifest),
            Err(ManifestError::InvalidSourceDir {
                reason: PackageSourceDirectoryErrorReason::Backslash,
                ..
            })
        ));
        assert!(matches!(
            parse_lock_source_dir("src/../outside"),
            Err(LockfileParseError::InvalidSourceDir {
                reason: PackageSourceDirectoryErrorReason::ParentDirectory,
                ..
            })
        ));
    }

    #[test]
    fn manifest_rejects_wrong_typed_values_instead_of_treating_them_as_absent() {
        let cases = [
            ("package = \"mission\"\n", "[package]", "a table"),
            ("[package]\nname = 123\n", "[package].name", "a string"),
            (
                "[package]\nname = \"mission\"\nsource_dir = 123\n",
                "[package].source_dir",
                "a string",
            ),
            (
                "dependencies = []\n[package]\nname = \"mission\"\n",
                "[dependencies]",
                "a table",
            ),
            (
                "[package]\nname = \"mission\"\n[dependencies]\nunits = { git = 1, rev = \"1111111111111111111111111111111111111111\" }\n",
                "[dependencies].units.git",
                "a string",
            ),
            (
                "[package]\nname = \"mission\"\n[dependencies]\nunits = { git = \"https://github.com/acme/units.git\", rev = 1 }\n",
                "[dependencies].units.rev",
                "a string",
            ),
        ];

        for (source, expected_field, expected_type) in cases {
            let err = parse_manifest_str(source).unwrap_err();
            assert!(
                matches!(
                    err,
                    ManifestError::InvalidType {
                        ref field,
                        expected,
                    } if field == expected_field && expected == expected_type
                ),
                "unexpected error for {expected_field}: {err:?}"
            );
        }
    }

    #[test]
    fn manifest_rejects_unknown_root_and_package_fields() {
        for (source, expected_table, expected_field) in [
            (
                "[package]\nname = \"mission\"\nunknown = true\n",
                "[package]",
                "unknown",
            ),
            (
                "[package]\nname = \"mission\"\n[tool]\nmode = \"unsafe\"\n",
                "graphcal.toml",
                "tool",
            ),
        ] {
            let err = parse_manifest_str(source).unwrap_err();
            assert!(matches!(
                err,
                ManifestError::UnsupportedField { table, field }
                    if table == expected_table && field == expected_field
            ));
        }
    }

    #[test]
    fn manifest_rejects_floating_refs() {
        let err = parse_manifest_str(
            r#"
[package]
name = "mission"

[dependencies]
orbital = { git = "https://github.com/acme/orbital.git", branch = "main" }
"#,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ManifestError::UnsupportedDependencyField { field, .. } if field == "branch"
        ));
    }

    #[test]
    fn manifest_requires_full_commit_hash() {
        let err = parse_manifest_str(
            r#"
[package]
name = "mission"

[dependencies]
orbital = { git = "https://github.com/acme/orbital.git", rev = "abc123" }
"#,
        )
        .unwrap_err();

        assert!(matches!(err, ManifestError::GitRev(_)));
    }

    #[test]
    fn git_urls_parse_into_supported_remote_transports() {
        let https = GitUrl::new("HTTPS://GitHub.COM/acme/orbital.git/").unwrap();
        assert_eq!(https.transport(), GitTransport::Https);
        assert_eq!(https.to_string(), "https://github.com/acme/orbital.git");

        let ssh = GitUrl::new("ssh://git@github.com:2222/acme/orbital.git").unwrap();
        assert_eq!(ssh.transport(), GitTransport::Ssh);
        assert_eq!(
            ssh.to_string(),
            "ssh://git@github.com:2222/acme/orbital.git"
        );

        let scp = GitUrl::new("git@github.com:acme/orbital.git").unwrap();
        assert_eq!(scp.transport(), GitTransport::Scp);
        assert_eq!(scp.to_string(), "git@github.com:acme/orbital.git");
    }

    #[test]
    fn git_urls_reject_local_malformed_unsupported_and_option_shaped_inputs() {
        let cases = [
            (
                "file:///etc",
                GitUrlErrorReason::UnsupportedScheme("file".to_string()),
            ),
            ("../../private-repo", GitUrlErrorReason::Malformed),
            (
                "ftp://host/repo",
                GitUrlErrorReason::UnsupportedScheme("ftp".to_string()),
            ),
            (
                "http://host/repo",
                GitUrlErrorReason::UnsupportedScheme("http".to_string()),
            ),
            ("not a URL", GitUrlErrorReason::Malformed),
            ("https://host", GitUrlErrorReason::MissingRepositoryPath),
            (
                "https://host/repo?ref=main",
                GitUrlErrorReason::QueryOrFragment,
            ),
            (
                "https://host/repo#fragment",
                GitUrlErrorReason::QueryOrFragment,
            ),
            ("https://host/org/../repo", GitUrlErrorReason::Malformed),
            (
                "ssh://git@host/-oProxyCommand=evil",
                GitUrlErrorReason::OptionShaped,
            ),
            (
                "git@-oProxyCommand=evil:org/repo",
                GitUrlErrorReason::OptionShaped,
            ),
            ("git@host:../repo", GitUrlErrorReason::Malformed),
        ];
        for (invalid, expected_reason) in cases {
            assert_eq!(
                GitUrl::new(invalid).unwrap_err().reason,
                expected_reason,
                "wrong rejection reason for `{invalid}`"
            );
        }
    }

    #[test]
    fn manifest_rejects_scp_like_credential_url() {
        let err = GitUrl::new("user:token@example.com/acme/orbital.git").unwrap_err();
        assert_eq!(err.reason, GitUrlErrorReason::Credentials);
    }

    #[test]
    fn manifest_rejects_credential_url() {
        let mixed_case_ssh =
            GitUrl::new("SSH://git:secret@github.com/acme/orbital.git").unwrap_err();
        assert_eq!(mixed_case_ssh.reason, GitUrlErrorReason::Credentials);

        let err = parse_manifest_str(
            r#"
[package]
name = "mission"

[dependencies]
orbital = { git = "https://token@example.com/acme/orbital.git", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
"#,
        )
        .unwrap_err();

        assert!(matches!(err, ManifestError::GitUrl(_)));
    }

    #[test]
    fn manifest_rejects_dependency_alias_matching_self_name() {
        let err = parse_manifest_str(
            r#"
[package]
name = "mission"

[dependencies]
mission = { git = "https://github.com/acme/mission.git", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
"#,
        )
        .unwrap_err();

        assert!(matches!(err, ManifestError::SelfDependencyAlias { .. }));
    }

    #[test]
    fn lockfile_validates_graph_edges_and_versions() {
        let mut root_deps = BTreeMap::new();
        root_deps.insert(dep("orbital"), id("pkg-orbital"));
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_deps),
            package(
                "pkg-orbital",
                "orbital",
                git_source("https://github.com/acme/orbital.git", 'a'),
                BTreeMap::new(),
            ),
        ]);

        lock.validate(GRAPHCAL_VERSION, STDLIB_VERSION).unwrap();
    }

    #[test]
    fn lockfile_rejects_graphcal_and_stdlib_version_mismatches() {
        let lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            BTreeMap::new(),
        )]);

        let graphcal_err = lock.validate("0.0.0", STDLIB_VERSION).unwrap_err();
        assert!(matches!(
            graphcal_err,
            LockValidationError::GraphcalVersionMismatch { .. }
        ));

        let stdlib_err = lock
            .validate(GRAPHCAL_VERSION, "stdlib-mismatch")
            .unwrap_err();
        assert!(matches!(
            stdlib_err,
            LockValidationError::StdlibVersionMismatch { .. }
        ));
    }

    #[test]
    fn lockfile_rejects_missing_edge_targets() {
        let mut root_deps = BTreeMap::new();
        root_deps.insert(dep("orbital"), id("pkg-missing"));
        let lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            root_deps,
        )]);

        let err = lock.validate(GRAPHCAL_VERSION, STDLIB_VERSION).unwrap_err();
        assert!(matches!(
            err,
            LockValidationError::MissingDependencyTarget { target, .. }
                if target == id("pkg-missing")
        ));
    }

    #[test]
    fn lockfile_rejects_dependency_cycles() {
        let mut root_deps = BTreeMap::new();
        root_deps.insert(dep("orbital"), id("pkg-orbital"));
        let mut orbital_deps = BTreeMap::new();
        orbital_deps.insert(dep("mission_dep"), id("pkg-mission"));
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_deps),
            package(
                "pkg-orbital",
                "orbital",
                git_source("https://github.com/acme/orbital.git", 'a'),
                orbital_deps,
            ),
        ]);

        let err = lock.validate(GRAPHCAL_VERSION, STDLIB_VERSION).unwrap_err();
        assert!(matches!(err, LockValidationError::DependencyCycle { .. }));
    }

    #[test]
    fn lockfile_rejects_duplicate_canonical_package_instances() {
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), BTreeMap::new()),
            package(
                "pkg-units-a",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
            package(
                "pkg-units-b",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ]);

        let err = lock.validate(GRAPHCAL_VERSION, STDLIB_VERSION).unwrap_err();
        assert!(matches!(
            err,
            LockValidationError::DuplicateCanonicalPackage { .. }
        ));
    }

    #[test]
    fn lockfile_rejects_conflicting_digests_for_one_git_source_identity() {
        let root_dependencies = BTreeMap::from([
            (dep("units_a"), id("pkg-units-a")),
            (dep("units_b"), id("pkg-units-b")),
        ]);
        let first = git_source("https://github.com/acme/units.git", '1');
        let second = git_source_with_digest("https://github.com/acme/units.git", '1', '2');
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_dependencies),
            package("pkg-units-a", "units_a", first, BTreeMap::new()),
            package("pkg-units-b", "units_b", second, BTreeMap::new()),
        ]);

        let error = lock.validate(GRAPHCAL_VERSION, STDLIB_VERSION).unwrap_err();

        assert!(matches!(
            error,
            LockValidationError::ConflictingGitSourceDigest(details)
                if details.first_digest == digest('1') && details.second_digest == digest('2')
        ));
    }

    #[test]
    fn package_graph_resolves_same_dependency_name_contextually() {
        let mut mission_deps = BTreeMap::new();
        mission_deps.insert(dep("orbital"), id("pkg-orbital"));
        mission_deps.insert(dep("thermal"), id("pkg-thermal"));
        let mut orbital_deps = BTreeMap::new();
        orbital_deps.insert(dep("units"), id("pkg-units-v1"));
        let mut thermal_deps = BTreeMap::new();
        thermal_deps.insert(dep("units"), id("pkg-units-v2"));
        let graph = lockfile(vec![
            package("pkg-mission", "mission", path_source(), mission_deps),
            package(
                "pkg-orbital",
                "orbital",
                git_source("https://github.com/acme/orbital.git", 'a'),
                orbital_deps,
            ),
            package(
                "pkg-thermal",
                "thermal",
                git_source("https://github.com/acme/thermal.git", 'b'),
                thermal_deps,
            ),
            package(
                "pkg-units-v1",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
            package(
                "pkg-units-v2",
                "units",
                git_source("https://github.com/acme/units.git", '2'),
                BTreeMap::new(),
            ),
        ])
        .package_graph(GRAPHCAL_VERSION, STDLIB_VERSION)
        .unwrap();

        let orbital_units = graph
            .resolve_module_path(&id("pkg-orbital"), &["units".into(), "si".into()])
            .unwrap();
        let thermal_units = graph
            .resolve_module_path(&id("pkg-thermal"), &["units".into(), "si".into()])
            .unwrap();

        assert_eq!(orbital_units.package, id("pkg-units-v1"));
        assert_eq!(thermal_units.package, id("pkg-units-v2"));
        assert_eq!(orbital_units.module_segments, ["si"]);
    }

    #[test]
    fn package_graph_rejects_implicit_transitive_dependency() {
        let mut mission_deps = BTreeMap::new();
        mission_deps.insert(dep("orbital"), id("pkg-orbital"));
        let mut orbital_deps = BTreeMap::new();
        orbital_deps.insert(dep("units"), id("pkg-units"));
        let graph = lockfile(vec![
            package("pkg-mission", "mission", path_source(), mission_deps),
            package(
                "pkg-orbital",
                "orbital",
                git_source("https://github.com/acme/orbital.git", 'a'),
                orbital_deps,
            ),
            package(
                "pkg-units",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ])
        .package_graph(GRAPHCAL_VERSION, STDLIB_VERSION)
        .unwrap();

        let err = graph
            .resolve_module_path(&id("pkg-mission"), &["units".into(), "si".into()])
            .unwrap_err();

        assert!(matches!(err, PackageResolveError::UnknownDependency { .. }));
    }

    #[test]
    fn lockfile_round_trips_deterministic_toml() {
        let mut root_deps = BTreeMap::new();
        root_deps.insert(dep("units_v2"), id("pkg-units-v2"));
        root_deps.insert(dep("units_v1"), id("pkg-units-v1"));
        let lock = lockfile(vec![
            package(
                "pkg-units-v2",
                "units",
                git_source("https://github.com/acme/units.git", '2'),
                BTreeMap::new(),
            ),
            package("pkg-mission", "mission", path_source(), root_deps),
            package(
                "pkg-units-v1",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ]);

        let toml = lock.to_deterministic_toml();
        let reparsed = parse_lockfile_str(&toml).unwrap();

        assert_eq!(reparsed.to_deterministic_toml(), toml);
        assert_eq!(reparsed.root, lock.root);
        assert!(
            toml.find("pkg-mission").unwrap() < toml.find("pkg-units-v1").unwrap()
                && toml.find("pkg-units-v1").unwrap() < toml.find("pkg-units-v2").unwrap()
        );
        assert!(toml.contains("units_v1 = \"pkg-units-v1\"\nunits_v2 = \"pkg-units-v2\""));
    }

    #[test]
    fn current_lock_schema_rejects_unknown_fields_at_every_fixed_table_level() {
        let mut lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), BTreeMap::new()),
            package(
                "pkg-units",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ]);
        lock.plugins = vec![LockedPlugin::new("plugins/x.wasm", "a".repeat(64)).unwrap()];
        let base = lock.to_deterministic_toml();
        let cases = [
            (
                base.replacen("created_by =", "unexpected = true\ncreated_by =", 1),
                "root",
            ),
            (
                base.replacen(
                    "name = \"mission\"",
                    "name = \"mission\"\nunexpected = true",
                    1,
                ),
                "package[0]",
            ),
            (
                base.replacen(
                    "type = \"path\"",
                    "type = \"path\"\nunexpected = true",
                    1,
                ),
                "package.source",
            ),
            (
                base.replacen(
                    "tree_hashes = { sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\" }",
                    "tree_hashes = { sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\", unexpected = true }",
                    1,
                ),
                "package.source.tree_hashes",
            ),
            (
                base.replacen(
                    "path = \"plugins/x.wasm\"",
                    "path = \"plugins/x.wasm\"\nunexpected = true",
                    1,
                ),
                "plugin[0]",
            ),
        ];

        for (content, expected_table) in cases {
            let error = parse_lockfile_str(&content).unwrap_err();
            assert!(
                matches!(
                    error,
                    LockfileParseError::UnknownField { ref table, ref field }
                        if table == expected_table && field == "unexpected"
                ),
                "wrong rejection for table `{expected_table}`: {error}"
            );
        }
    }

    #[test]
    fn current_lock_schema_rejects_non_edge_values_in_dependency_tables() {
        let root_dependencies = BTreeMap::from([(dep("units"), id("pkg-units"))]);
        let base = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_dependencies),
            package(
                "pkg-units",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ])
        .to_deterministic_toml();
        let malformed = base.replacen(
            "units = \"pkg-units\"",
            "units = { target = \"pkg-units\", unexpected = true }",
            1,
        );

        assert!(matches!(
            parse_lockfile_str(&malformed),
            Err(LockfileParseError::InvalidType { field, .. })
                if field == "package.dependencies.units"
        ));
    }

    #[test]
    fn sha256_digest_requires_and_renders_canonical_lowercase_hex() {
        let lowercase = "0123456789abcdef".repeat(4);
        let digest = Sha256Digest::new(lowercase.clone()).unwrap();
        assert_eq!(digest.to_string(), lowercase);
        assert_eq!(digest.as_bytes().len(), 32);

        for malformed in [
            "f".repeat(63),
            "f".repeat(65),
            "F".repeat(64),
            "g".repeat(64),
        ] {
            assert!(Sha256Digest::new(malformed).is_err());
        }
    }

    #[test]
    fn lockfile_parser_rejects_malformed_source_tree_digests() {
        let base = lockfile(vec![
            package("pkg-mission", "mission", path_source(), BTreeMap::new()),
            package(
                "pkg-units",
                "units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ])
        .to_deterministic_toml();
        let uppercase = base.replacen(&"1".repeat(64), &"A".repeat(64), 1);

        assert!(matches!(
            parse_lockfile_str(&uppercase),
            Err(LockfileParseError::Sha256Digest(_))
        ));
    }

    #[test]
    fn lockfile_toml_escapes_control_characters() {
        let mut lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            BTreeMap::new(),
        )]);
        lock.created_by = "graphcal\nnightly".to_string();

        let toml = lock.to_deterministic_toml();
        assert!(toml.contains("created_by = \"graphcal\\nnightly\""));
        let reparsed = parse_lockfile_str(&toml).unwrap();
        assert_eq!(reparsed.created_by, "graphcal\nnightly");
    }

    #[test]
    fn plugin_pins_roundtrip_sorted_through_deterministic_toml() {
        let sha_a = "a".repeat(64);
        let sha_b = "b".repeat(64);
        let mut lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            BTreeMap::new(),
        )]);
        lock.plugins = vec![
            LockedPlugin::new("plugins/zeta.wasm", sha_a.clone()).unwrap(),
            LockedPlugin::new("plugins/alpha.wasm", sha_b.clone()).unwrap(),
        ];

        let toml = lock.to_deterministic_toml();
        let alpha_at = toml.find("plugins/alpha.wasm").unwrap();
        let zeta_at = toml.find("plugins/zeta.wasm").unwrap();
        assert!(alpha_at < zeta_at, "pins must serialize sorted by path");

        let reparsed = parse_lockfile_str(&toml).unwrap();
        reparsed
            .validate(GRAPHICAL_VERSION, STDLIB_VERSION)
            .unwrap();
        assert_eq!(reparsed.plugins.len(), 2);
        assert_eq!(reparsed.plugins[0].path(), "plugins/alpha.wasm");
        assert_eq!(reparsed.plugins[0].sha256().to_string(), sha_b);
        assert_eq!(reparsed.plugins[1].path(), "plugins/zeta.wasm");
        assert_eq!(reparsed.plugins[1].sha256().to_string(), sha_a);
    }

    #[test]
    fn lockfiles_without_plugin_pins_parse_as_empty() {
        let lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            BTreeMap::new(),
        )]);
        let toml = lock.to_deterministic_toml();
        assert!(!toml.contains("[[plugin]]"));
        assert!(parse_lockfile_str(&toml).unwrap().plugins.is_empty());
    }

    #[test]
    fn plugin_pin_paths_and_digests_are_validated() {
        let sha = "0".repeat(64);
        for bad_path in [
            "/abs/plugin.wasm",
            "../escape.wasm",
            "./dotted.wasm",
            "not-wasm.dll",
            "back\\slash.wasm",
            "",
        ] {
            assert!(
                matches!(
                    LockedPlugin::new(bad_path, sha.clone()),
                    Err(LockedPluginError::InvalidPath { .. })
                ),
                "path `{bad_path}` must be rejected"
            );
        }
        for bad_sha in ["", "abc", &"A".repeat(64), &"g".repeat(64)] {
            assert!(matches!(
                LockedPlugin::new("plugins/x.wasm", bad_sha.to_string()),
                Err(LockedPluginError::InvalidSha256(_))
            ));
        }
    }

    #[test]
    fn duplicate_plugin_pins_are_rejected_by_validation() {
        let sha = "0".repeat(64);
        let mut lock = lockfile(vec![package(
            "pkg-mission",
            "mission",
            path_source(),
            BTreeMap::new(),
        )]);
        lock.plugins = vec![
            LockedPlugin::new("plugins/x.wasm", sha.clone()).unwrap(),
            LockedPlugin::new("plugins/x.wasm", sha).unwrap(),
        ];
        assert!(matches!(
            lock.validate(GRAPHICAL_VERSION, STDLIB_VERSION),
            Err(LockValidationError::DuplicatePluginPath { .. })
        ));
    }

    #[test]
    fn lockfile_manifest_validation_is_total_and_bijective() {
        let (lock, manifests) = matched_lock_and_manifests();
        let validated =
            validate_lock_against_manifests(&lock, &manifests, GRAPHICAL_VERSION, STDLIB_VERSION)
                .unwrap();
        assert_eq!(validated.root(), &id("pkg-mission"));
        assert_eq!(
            validated.manifest(&id("pkg-units")).unwrap().name,
            pkg("units")
        );

        let mut missing_manifest = manifests.clone();
        missing_manifest.remove(&id("pkg-units"));
        assert!(matches!(
            validate_lock_against_manifests(
                &lock,
                &missing_manifest,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::MissingPackageManifest { package })
                if package == id("pkg-units")
        ));

        let mut extra_manifest = manifests.clone();
        extra_manifest.insert(
            id("pkg-extra"),
            PackageManifest {
                name: pkg("extra"),
                source_dir: source_dir("src"),
                dependencies: BTreeMap::new(),
            },
        );
        assert!(matches!(
            validate_lock_against_manifests(
                &lock,
                &extra_manifest,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::ManifestWithoutLockedPackage { package })
                if package == id("pkg-extra")
        ));

        let mut wrong_name = manifests.clone();
        wrong_name.get_mut(&id("pkg-mission")).unwrap().name = pkg("other");
        assert!(matches!(
            validate_lock_against_manifests(
                &lock,
                &wrong_name,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::LockedManifestNameMismatch { package, .. })
                if package == id("pkg-mission")
        ));

        let mut wrong_source_dir = manifests.clone();
        wrong_source_dir
            .get_mut(&id("pkg-units"))
            .unwrap()
            .source_dir = source_dir("unhashed");
        assert!(matches!(
            validate_lock_against_manifests(
                &lock,
                &wrong_source_dir,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::LockedManifestSourceDirectoryMismatch { package, .. })
                if package == id("pkg-units")
        ));

        let mut missing_edge = lock.clone();
        missing_edge.packages[0].dependencies.clear();
        assert!(matches!(
            validate_lock_against_manifests(
                &missing_edge,
                &manifests,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::MissingDependencyEdge { dependency, .. })
                if dependency == dep("units")
        ));

        let mut undeclared_edge = manifests;
        undeclared_edge
            .get_mut(&id("pkg-mission"))
            .unwrap()
            .dependencies
            .clear();
        assert!(matches!(
            validate_lock_against_manifests(
                &lock,
                &undeclared_edge,
                GRAPHICAL_VERSION,
                STDLIB_VERSION
            ),
            Err(LockValidationError::UndeclaredDependencyEdge { dependency, .. })
                if dependency == dep("units")
        ));
    }

    #[test]
    fn lockfile_manifest_validation_rejects_package_name_mismatch() {
        let mut root_deps = BTreeMap::new();
        root_deps.insert(dep("units_alias"), id("pkg-units"));
        let lock = lockfile(vec![
            package("pkg-mission", "mission", path_source(), root_deps),
            package(
                "pkg-units",
                "wrong_units",
                git_source("https://github.com/acme/units.git", '1'),
                BTreeMap::new(),
            ),
        ]);
        let mut manifest_deps = BTreeMap::new();
        manifest_deps.insert(
            dep("units_alias"),
            DependencySpec {
                package: Some(pkg("units")),
                git: GitDependency {
                    url: url("https://github.com/acme/units.git"),
                    rev: hash('1'),
                },
            },
        );
        let manifests = BTreeMap::from([
            (
                id("pkg-mission"),
                PackageManifest {
                    name: pkg("mission"),
                    source_dir: source_dir("src"),
                    dependencies: manifest_deps,
                },
            ),
            (
                id("pkg-units"),
                PackageManifest {
                    name: pkg("wrong_units"),
                    source_dir: source_dir("src"),
                    dependencies: BTreeMap::new(),
                },
            ),
        ]);

        let err =
            validate_lock_against_manifests(&lock, &manifests, GRAPHICAL_VERSION, STDLIB_VERSION)
                .unwrap_err();

        assert!(matches!(
            err,
            LockValidationError::PackageNameMismatch { actual, .. } if actual == pkg("wrong_units")
        ));
    }
}
