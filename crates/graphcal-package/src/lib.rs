//! Pure package-management domain model for Graphcal.
//!
//! This crate intentionally contains no Git, filesystem, cache, or CLI I/O.
//! Callers provide manifest and lockfile text, source metadata, and materialized
//! package manifests; this crate validates and resolves the typed package graph.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, PathBuf};

use thiserror::Error;
use toml_spanner::{Item, Table};

/// Graphcal's first lockfile schema version.
pub const LOCK_VERSION: u64 = 1;
/// Current standard-library identity recorded by the MVP lockfile.
pub const STDLIB_VERSION: &str = "0.0.1-alpha.14";

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

/// Git remote URL after credential validation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitUrl(String);

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
impl_string_newtype!(GitUrl);

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
    pub fn new(value: impl Into<String>) -> Result<Self, DependencyNameError> {
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
    pub fn new(value: impl Into<String>) -> Result<Self, GitRevError> {
        let value = value.into();
        if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(GitRevError { value })
        }
    }
}

impl GitUrl {
    /// Construct a Git URL after rejecting embedded credentials.
    ///
    /// # Errors
    ///
    /// Returns [`GitUrlError`] for empty URLs or URLs that include credentials.
    pub fn new(value: impl Into<String>) -> Result<Self, GitUrlError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(GitUrlError {
                value,
                reason: GitUrlErrorReason::Empty,
            });
        }
        if url_has_userinfo(&value) {
            return Err(GitUrlError {
                value,
                reason: GitUrlErrorReason::Credentials,
            });
        }
        Ok(Self(value))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitUrlErrorReason {
    Empty,
    Credentials,
}

impl fmt::Display for GitUrlErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("must not be empty"),
            Self::Credentials => f.write_str("must not include credentials"),
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

fn url_has_userinfo(value: &str) -> bool {
    let Some(scheme_idx) = value.find("://") else {
        return false;
    };
    let authority_start = scheme_idx + 3;
    let authority = value[authority_start..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.contains('@')
}

/// Parsed `graphcal.toml` package section and direct dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    /// Real package name.
    pub name: PackageName,
    /// Source directory relative to the package root.
    pub source_dir: PathBuf,
    /// Direct dependencies keyed by local source-visible alias.
    pub dependencies: BTreeMap<DependencyName, DependencySpec>,
}

/// One direct dependency declaration in `graphcal.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// Real package name expected at the fetched source. `None` means it must
    /// match the dependency alias.
    pub package: Option<PackageName>,
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
    /// Source directory escaped the package root.
    #[error("invalid source_dir `{dir}`: must be a relative path inside the package root")]
    InvalidSourceDir { dir: String },
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
    let arena = toml_spanner::Arena::new();
    let root = toml_spanner::parse(content, &arena).map_err(|e| ManifestError::TomlParseError {
        message: e.to_string(),
    })?;

    let name = PackageName::new(root["package"]["name"].as_str().ok_or(
        ManifestError::MissingField {
            field: "[package].name",
        },
    )?)?;
    let source_dir_str = root["package"]["source_dir"].as_str().unwrap_or("src");
    let source_dir = parse_source_dir(source_dir_str)?;
    let dependencies = parse_manifest_dependencies(&root["dependencies"], &name)?;

    Ok(PackageManifest {
        name,
        source_dir,
        dependencies,
    })
}

fn parse_source_dir(value: &str) -> Result<PathBuf, ManifestError> {
    let path = PathBuf::from(value);
    let escapes_root = path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir));
    if escapes_root {
        Err(ManifestError::InvalidSourceDir {
            dir: value.to_string(),
        })
    } else {
        Ok(path)
    }
}

fn parse_manifest_dependencies(
    item: &toml_spanner::MaybeItem<'_>,
    package_name: &PackageName,
) -> Result<BTreeMap<DependencyName, DependencySpec>, ManifestError> {
    let Some(item) = item.item() else {
        return Ok(BTreeMap::new());
    };
    let Some(table) = item.as_table() else {
        return Err(ManifestError::InvalidType {
            field: "[dependencies]".to_string(),
            expected: "a table",
        });
    };

    table
        .entries()
        .iter()
        .map(|(key, value)| {
            let dep_name = DependencyName::new(key.name)?;
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

fn parse_dependency_spec(
    dependency: &str,
    item: &Item<'_>,
) -> Result<DependencySpec, ManifestError> {
    let Some(table) = item.as_table() else {
        return Err(ManifestError::InvalidType {
            field: format!("[dependencies].{dependency}"),
            expected: "an inline table",
        });
    };

    for (key, _) in table.entries() {
        match key.name {
            "git" | "rev" | "package" => {}
            other => {
                return Err(ManifestError::UnsupportedDependencyField {
                    dependency: dependency.to_string(),
                    field: other.to_string(),
                });
            }
        }
    }

    let git = table
        .get("git")
        .and_then(Item::as_str)
        .ok_or_else(|| ManifestError::MissingField {
            field: "[dependencies].<name>.git",
        })
        .and_then(|url| GitUrl::new(url).map_err(ManifestError::GitUrl))?;
    let rev = table
        .get("rev")
        .and_then(Item::as_str)
        .ok_or_else(|| ManifestError::MissingField {
            field: "[dependencies].<name>.rev",
        })
        .and_then(|rev| GitCommitHash::new(rev).map_err(ManifestError::GitRev))?;
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
        self.reject_duplicate_canonical_instances()?;
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
            push_kv_string(
                &mut out,
                "source_dir",
                package.source_dir.to_string_lossy().as_ref(),
            );
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
                    push_kv_string(&mut out, "url", url.as_str());
                    push_kv_string(&mut out, "requested_rev", requested_rev.as_str());
                    push_kv_string(&mut out, "commit", commit.as_str());
                    push_kv_inline_table(
                        &mut out,
                        "tree_hashes",
                        &[("sha256", tree_hashes.sha256.as_str())],
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
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalPackageInstance {
    name: PackageName,
    source_dir: PathBuf,
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
    pub source_dir: PathBuf,
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
            Self::Git {
                url,
                commit,
                tree_hashes,
                ..
            } => CanonicalPackageSource::Git {
                url: url.clone(),
                commit: commit.clone(),
                sha256: tree_hashes.sha256.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonicalPackageSource {
    Path {
        path: String,
    },
    Git {
        url: GitUrl,
        commit: GitCommitHash,
        sha256: String,
    },
}

/// Hashes for a locked Git source tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTreeHashes {
    /// SHA-256 digest of the normalized source tree.
    pub sha256: String,
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
    /// Two ids describe the same canonical package instance.
    #[error("package ids `{first}` and `{second}` describe the same canonical package instance")]
    DuplicateCanonicalPackage {
        first: PackageInstanceId,
        second: PackageInstanceId,
    },
    /// A lock edge was not declared by the importing manifest.
    #[error("package `{package}` dependency `{dependency}` is not declared in its manifest")]
    UndeclaredDependencyEdge {
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

/// Validate a lockfile against each materialized package manifest.
///
/// # Errors
///
/// Returns [`LockValidationError`] if the lock introduces undeclared edges or
/// edges whose target source/name does not satisfy the importing manifest.
pub fn validate_lock_against_manifests(
    lockfile: &Lockfile,
    manifests: &BTreeMap<PackageInstanceId, PackageManifest>,
    active_graphcal_version: &str,
    active_stdlib_version: &str,
) -> Result<(), LockValidationError> {
    let graph = lockfile.package_graph(active_graphcal_version, active_stdlib_version)?;
    for package in lockfile.packages.iter() {
        let Some(manifest) = manifests.get(&package.id) else {
            continue;
        };
        for (dependency, target_id) in &package.dependencies {
            let spec = manifest.dependencies.get(dependency).ok_or_else(|| {
                LockValidationError::UndeclaredDependencyEdge {
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
    Ok(())
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
    pub fn root(&self) -> &PackageInstanceId {
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
    pub relation: PackageResolutionRelation,
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
    /// Source directory escaped the package root.
    #[error(
        "invalid source_dir `{dir}` in graphcal.lock: must be a relative path inside the package root"
    )]
    InvalidSourceDir { dir: String },
    /// Package source type was not recognized.
    #[error("unsupported package source type `{source_type}` in graphcal.lock")]
    UnsupportedSourceType { source_type: String },
}

/// Parse a lockfile from TOML text.
///
/// # Errors
///
/// Returns [`LockfileParseError`] for TOML, type, and field validation errors.
pub fn parse_lockfile_str(content: &str) -> Result<Lockfile, LockfileParseError> {
    let arena = toml_spanner::Arena::new();
    let root =
        toml_spanner::parse(content, &arena).map_err(|e| LockfileParseError::TomlParseError {
            message: e.to_string(),
        })?;

    let lock_version = required_u64(root["lock_version"].item(), "lock_version")?;
    let created_by = required_string(root["created_by"].item(), "created_by")?.to_string();
    let graphcal_version =
        required_string(root["graphcal_version"].item(), "graphcal_version")?.to_string();
    let stdlib_version =
        required_string(root["stdlib_version"].item(), "stdlib_version")?.to_string();
    let root_id = PackageInstanceId::new(required_string(root["root"].item(), "root")?)?;
    let package_array = root["package"]
        .as_array()
        .ok_or(LockfileParseError::MissingField { field: "package" })?;
    let packages = package_array
        .iter()
        .enumerate()
        .map(|(index, package)| parse_locked_package(index, package))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Lockfile {
        lock_version,
        created_by,
        graphcal_version,
        stdlib_version,
        root: root_id,
        packages,
    })
}

fn parse_locked_package(
    index: usize,
    item: &Item<'_>,
) -> Result<LockedPackage, LockfileParseError> {
    let Some(table) = item.as_table() else {
        return Err(LockfileParseError::InvalidType {
            field: format!("package[{index}]"),
            expected: "a table",
        });
    };
    let id = PackageInstanceId::new(required_string(table.get("id"), "package.id")?)?;
    let name = PackageName::new(required_string(table.get("name"), "package.name")?)?;
    let source_dir = parse_lock_source_dir(required_string(
        table.get("source_dir"),
        "package.source_dir",
    )?)?;
    let source_table = table
        .get("source")
        .and_then(Item::as_table)
        .ok_or_else(|| LockfileParseError::MissingField {
            field: "package.source",
        })?;
    let source = parse_package_source(source_table)?;
    let dependencies = table
        .get("dependencies")
        .map(parse_package_dependencies)
        .transpose()?
        .unwrap_or_default();

    Ok(LockedPackage {
        id,
        name,
        source_dir,
        source,
        dependencies,
    })
}

fn parse_lock_source_dir(value: &str) -> Result<PathBuf, LockfileParseError> {
    parse_source_dir(value).map_err(|_| LockfileParseError::InvalidSourceDir {
        dir: value.to_string(),
    })
}

fn parse_package_source(table: &Table<'_>) -> Result<PackageSource, LockfileParseError> {
    let source_type = required_string(table.get("type"), "package.source.type")?;
    match source_type {
        "path" => Ok(PackageSource::Path {
            path: required_string(table.get("path"), "package.source.path")?.to_string(),
        }),
        "git" => {
            let url = GitUrl::new(required_string(table.get("url"), "package.source.url")?)?;
            let requested_rev = GitCommitHash::new(required_string(
                table.get("requested_rev"),
                "package.source.requested_rev",
            )?)?;
            let commit = GitCommitHash::new(required_string(
                table.get("commit"),
                "package.source.commit",
            )?)?;
            let hashes = table.get("tree_hashes").and_then(Item::as_table).ok_or(
                LockfileParseError::MissingField {
                    field: "package.source.tree_hashes",
                },
            )?;
            let sha256 =
                required_string(hashes.get("sha256"), "package.source.tree_hashes.sha256")?
                    .to_string();
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
    item: &Item<'_>,
) -> Result<BTreeMap<DependencyName, PackageInstanceId>, LockfileParseError> {
    let Some(table) = item.as_table() else {
        return Err(LockfileParseError::InvalidType {
            field: "package.dependencies".to_string(),
            expected: "a table",
        });
    };
    table
        .entries()
        .iter()
        .map(|(key, value)| {
            let name = DependencyName::new(key.name)?;
            let target = PackageInstanceId::new(value.as_str().ok_or_else(|| {
                LockfileParseError::InvalidType {
                    field: format!("package.dependencies.{}", key.name),
                    expected: "a string",
                }
            })?)?;
            Ok((name, target))
        })
        .collect()
}

fn required_string<'a>(
    item: Option<&'a Item<'a>>,
    field: &'static str,
) -> Result<&'a str, LockfileParseError> {
    let item = item.ok_or(LockfileParseError::MissingField { field })?;
    item.as_str()
        .ok_or_else(|| LockfileParseError::InvalidType {
            field: field.to_string(),
            expected: "a string",
        })
}

fn required_u64(item: Option<&Item<'_>>, field: &'static str) -> Result<u64, LockfileParseError> {
    let item = item.ok_or(LockfileParseError::MissingField { field })?;
    item.as_u64()
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
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAPHCAL_VERSION: &str = "0.0.1-alpha.14";

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

    fn git_source(url_text: &str, rev_char: char) -> PackageSource {
        PackageSource::Git {
            url: url(url_text),
            requested_rev: hash(rev_char),
            commit: hash(rev_char),
            tree_hashes: SourceTreeHashes {
                sha256: format!("sha256-{rev_char}"),
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
            source_dir: PathBuf::from("src"),
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
            packages,
        }
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
    fn manifest_rejects_credential_url() {
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
        let manifests = BTreeMap::from([(
            id("pkg-mission"),
            PackageManifest {
                name: pkg("mission"),
                source_dir: PathBuf::from("src"),
                dependencies: manifest_deps,
            },
        )]);

        let err =
            validate_lock_against_manifests(&lock, &manifests, GRAPHICAL_VERSION, STDLIB_VERSION)
                .unwrap_err();

        assert!(matches!(
            err,
            LockValidationError::PackageNameMismatch { actual, .. } if actual == pkg("wrong_units")
        ));
    }
}
