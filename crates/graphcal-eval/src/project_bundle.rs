//! Portable, bounded project artifacts for offline evaluation.
//!
//! This is a data/validation boundary, not a filesystem loader. Native shells
//! capture artifacts; browser shells mount the validated immutable snapshot.

use crate::package_sources::EmbeddedPackage;
use graphcal_package::PackageInstanceId;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use base64::Engine as _;
use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Upper bound on a serialized report project, checked before JS strings enter Wasm.
pub const MAX_BUNDLE_JSON_BYTES: usize = 96 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENCODED_ARTIFACT_BYTES: usize = MAX_ARTIFACT_BYTES.div_ceil(3) * 4;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_PATH_BYTES: usize = 1024;

/// Portable relative artifact name. Construction excludes traversal and host paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactPath(String);

impl TryFrom<String> for ArtifactPath {
    type Error = BundleError;

    fn try_from(path: String) -> Result<Self, Self::Error> {
        if path.is_empty()
            || path.len() > MAX_PATH_BYTES
            || path.contains(['\\', ':', '\0'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || Path::new(&path)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(BundleError::UnsafePath);
        }
        Ok(Self(path))
    }
}

impl From<ArtifactPath> for String {
    fn from(path: ArtifactPath) -> Self {
        path.0
    }
}

impl ArtifactPath {
    /// The portable spelling, for serialization and virtual filesystem mounting.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Artifact role is explicit; binary data is encoded only at this wire boundary.
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "content",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ArtifactContent {
    Source(String),
    Manifest(String),
    Lockfile(String),
    Plugin(#[serde(with = "binary")] Vec<u8>),
    /// Non-executable bytes included by the authenticated source-tree algorithm.
    Auxiliary(#[serde(with = "binary")] Vec<u8>),
}

impl ArtifactContent {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Source(text) | Self::Manifest(text) | Self::Lockfile(text) => text.as_bytes(),
            Self::Plugin(bytes) | Self::Auxiliary(bytes) => bytes,
        }
    }

    fn accepts(&self, path: &ArtifactPath) -> bool {
        match self {
            Self::Source(_) => Path::new(path.as_str())
                .extension()
                .is_some_and(|ext| ext == "gcl"),
            Self::Manifest(_) => path.as_str() == "graphcal.toml",
            Self::Lockfile(_) => path.as_str() == "graphcal.lock",
            Self::Plugin(_) => Path::new(path.as_str())
                .extension()
                .is_some_and(|ext| ext == "wasm"),
            Self::Auxiliary(_) => {
                path.as_str() != "graphcal.toml"
                    && !Path::new(path.as_str())
                        .extension()
                        .is_some_and(|ext| ext == "wasm" || ext == "gcl")
            }
        }
    }
}

impl std::fmt::Debug for ArtifactContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither source text nor executable bytes belong in incidental logs.
        formatter
            .debug_struct("ArtifactContent")
            .field("bytes", &self.bytes().len())
            .finish()
    }
}

/// One captured artifact, with a validated name and an explicit category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleArtifact {
    pub path: ArtifactPath,
    #[serde(flatten)]
    pub content: ArtifactContent,
}

/// Wire representation; consumers must validate before using its artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBundle {
    pub entry: ArtifactPath,
    pub files: Vec<BundleArtifact>,
    pub dependencies: Vec<BundlePackage>,
}

/// One complete dependency snapshot, keyed by its opaque locked identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundlePackage {
    #[serde(with = "package_identity")]
    pub id: PackageInstanceId,
    pub files: Vec<BundleArtifact>,
}

/// Isolated mounted authorities; no virtual path is interpreted as a native path.
#[derive(Debug, Clone)]
pub struct MountedProject {
    pub filesystem: InMemoryFileSystem,
    pub dependencies: BTreeMap<PackageInstanceId, EmbeddedPackage>,
}

impl ProjectBundle {
    /// Encode the wire boundary with the same envelope limit used by decoding.
    /// Artifact validation remains the responsibility of `mount`.
    ///
    /// # Errors
    /// Rejects serialization failures or an oversized JSON envelope.
    pub fn to_json(&self) -> Result<String, BundleError> {
        let json = serde_json::to_string(self).map_err(BundleError::Json)?;
        Self::check_json_size(json.len())?;
        Ok(json)
    }

    const fn check_json_size(bytes: usize) -> Result<(), BundleError> {
        if bytes > MAX_BUNDLE_JSON_BYTES {
            Err(BundleError::TotalSize)
        } else {
            Ok(())
        }
    }

    /// Validate and mount this snapshot without granting any host filesystem access.
    ///
    /// # Errors
    /// Rejects oversized, ambiguous, incorrectly categorized, or incomplete bundles.
    pub fn mount(&self, root: &Path) -> Result<MountedProject, BundleError> {
        if self.files.is_empty() || self.dependencies.len() > 1024 {
            return Err(BundleError::FileCount);
        }
        self.files
            .iter()
            .chain(self.dependencies.iter().flat_map(|package| &package.files))
            .try_fold((0usize, 0usize), |(count, total), artifact| {
                if count >= MAX_FILES {
                    return Err(BundleError::FileCount);
                }
                let length = artifact.content.bytes().len();
                if length > MAX_ARTIFACT_BYTES {
                    return Err(BundleError::ArtifactSize);
                }
                let total = total
                    .checked_add(length)
                    .filter(|size| *size <= MAX_TOTAL_BYTES)
                    .ok_or(BundleError::TotalSize)?;
                Ok((count.checked_add(1).ok_or(BundleError::FileCount)?, total))
            })?;
        let filesystem = mount_files(&self.files, root)?;
        if !self.files.iter().any(|artifact| {
            artifact.path == self.entry && matches!(artifact.content, ArtifactContent::Source(_))
        }) {
            return Err(BundleError::MissingEntry);
        }
        let mut dependencies = BTreeMap::new();
        for (index, package) in self.dependencies.iter().enumerate() {
            if package.id.as_str().len() > MAX_PATH_BYTES || dependencies.contains_key(&package.id)
            {
                return Err(BundleError::PackageIdentity);
            }
            // Numeric mount locations are wire/filesystem boundary details, not
            // encoded package identities. The typed map preserves lock identity.
            let package_root = root.join(".graphcal-packages").join(index.to_string());
            let filesystem = mount_files(&package.files, &package_root)?;
            dependencies.insert(
                package.id.clone(),
                EmbeddedPackage {
                    root: package_root,
                    filesystem,
                },
            );
        }
        Ok(MountedProject {
            filesystem,
            dependencies,
        })
    }

    /// Decode a bounded wire payload; mounting performs cross-artifact validation.
    ///
    /// # Errors
    /// Returns a structured size or malformed-payload error.
    pub fn from_json(json: &str) -> Result<Self, BundleError> {
        Self::check_json_size(json.len())?;
        serde_json::from_str(json).map_err(BundleError::Json)
    }
}

/// Invalid portable-project input. Messages never dump source or binary payloads.
#[derive(Debug, Error)]
pub enum BundleError {
    #[error("bundle contains duplicate or oversized package identities")]
    PackageIdentity,
    #[error("authenticated source or manifest is not UTF-8")]
    InvalidText,
    #[error("bundle paths must be bounded, portable relative paths without traversal")]
    UnsafePath,
    #[error("bundle must contain between 1 and 4096 artifacts")]
    FileCount,
    #[error("bundle artifact exceeds 16 MiB")]
    ArtifactSize,
    #[error("bundle exceeds its aggregate size limit")]
    TotalSize,
    #[error("bundle artifact category does not match its path")]
    ArtifactKind,
    #[error("bundle contains duplicate or conflicting paths")]
    DuplicatePath,
    #[error("bundle entry must identify a bundled Graphcal source")]
    MissingEntry,
    #[error("invalid bundle JSON: {0}")]
    Json(serde_json::Error),
}

fn mount_files(files: &[BundleArtifact], root: &Path) -> Result<InMemoryFileSystem, BundleError> {
    if files.is_empty() {
        return Err(BundleError::FileCount);
    }
    let mut seen = BTreeSet::new();
    let mut fs = InMemoryFileSystem::new();
    for artifact in files {
        if !artifact.content.accepts(&artifact.path) {
            return Err(BundleError::ArtifactKind);
        }
        if !seen.insert(&artifact.path) {
            return Err(BundleError::DuplicatePath);
        }
        let path = VirtualAbsolutePath::new(root.join(artifact.path.as_str()))
            .map_err(|_| BundleError::UnsafePath)?;
        fs.add_binary_file(path, artifact.content.bytes().to_vec())
            .map_err(|_| BundleError::DuplicatePath)?;
    }
    Ok(fs)
}

impl BundlePackage {
    /// Preserve every byte contributing to the verified dependency digest,
    /// including non-Graphcal data files inside the source directory.
    ///
    /// # Errors
    /// Rejects non-portable paths or invalid textual source/manifest encoding.
    pub fn from_snapshot(
        id: PackageInstanceId,
        snapshot: &graphcal_io::SourceTreeSnapshot,
    ) -> Result<Self, BundleError> {
        let files = snapshot
            .files()
            .map(|(relative, bytes)| {
                let text =
                    || String::from_utf8(bytes.to_vec()).map_err(|_| BundleError::InvalidText);
                let content = if relative == "graphcal.toml" {
                    ArtifactContent::Manifest(text()?)
                } else {
                    match Path::new(relative)
                        .extension()
                        .and_then(|extension| extension.to_str())
                    {
                        Some("gcl") => ArtifactContent::Source(text()?),
                        Some("wasm") => ArtifactContent::Plugin(bytes.to_vec()),
                        _ => ArtifactContent::Auxiliary(bytes.to_vec()),
                    }
                };
                Ok(BundleArtifact {
                    path: relative
                        .components()
                        .map(|part| part.as_os_str().to_str().ok_or(BundleError::UnsafePath))
                        .collect::<Result<Vec<_>, _>>()?
                        .join("/")
                        .try_into()?,
                    content,
                })
            })
            .collect::<Result<_, BundleError>>()?;
        Ok(Self { id, files })
    }
}

mod package_identity {
    use super::{Deserialize, MAX_PATH_BYTES, PackageInstanceId};
    pub fn serialize<S: serde::Serializer>(
        id: &PackageInstanceId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(id.as_str())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PackageInstanceId, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() > MAX_PATH_BYTES {
            return Err(serde::de::Error::custom(
                "package identity exceeds bundle limit",
            ));
        }
        PackageInstanceId::new(value).map_err(serde::de::Error::custom)
    }
}

mod binary {
    use super::*;

    pub fn serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() > MAX_ENCODED_ARTIFACT_BYTES {
            return Err(serde::de::Error::custom(
                "plugin exceeds encoded artifact limit",
            ));
        }
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| serde::de::Error::custom("invalid plugin base64"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_size_boundaries_are_shared_by_producers_and_consumers() {
        assert!(ProjectBundle::check_json_size(MAX_BUNDLE_JSON_BYTES - 1).is_ok());
        assert!(ProjectBundle::check_json_size(MAX_BUNDLE_JSON_BYTES).is_ok());
        assert!(matches!(
            ProjectBundle::check_json_size(MAX_BUNDLE_JSON_BYTES + 1),
            Err(BundleError::TotalSize)
        ));
        assert!(matches!(
            ProjectBundle::check_json_size(usize::MAX),
            Err(BundleError::TotalSize)
        ));
    }

    #[test]
    fn paths_are_portable_and_do_not_escape() {
        for path in [
            "",
            "/main.gcl",
            "../main.gcl",
            "x/../main.gcl",
            "C:main.gcl",
            "x\\main.gcl",
            "x//main.gcl",
            "./main.gcl",
        ] {
            assert!(ArtifactPath::try_from(path.to_string()).is_err(), "{path}");
        }
    }

    #[test]
    fn malformed_categories_entries_and_resource_limits_fail_closed() {
        let source = || BundleArtifact {
            path: "main.gcl".to_string().try_into().unwrap(),
            content: ArtifactContent::Source(String::new()),
        };
        let mut bundle = ProjectBundle {
            dependencies: vec![],
            entry: "main.gcl".to_string().try_into().unwrap(),
            files: vec![],
        };
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::FileCount)
        ));
        bundle.files = vec![source(); MAX_FILES + 1];
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::FileCount)
        ));
        bundle.files = vec![source()];
        bundle.files[0].content = ArtifactContent::Plugin(vec![]);
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::ArtifactKind)
        ));
        bundle.files[0] = source();
        bundle.entry = "missing.gcl".to_string().try_into().unwrap();
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::MissingEntry)
        ));
        bundle.files[0].content = ArtifactContent::Source("x".repeat(MAX_ARTIFACT_BYTES + 1));
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::ArtifactSize)
        ));
        assert!(
            ProjectBundle::from_json(r#"{"entry":"main.gcl","files":[],"unexpected":true}"#)
                .is_err()
        );
    }

    #[test]
    fn wire_round_trip_preserves_binary_and_rejects_conflicts() {
        let mut bundle = ProjectBundle {
            dependencies: vec![],
            entry: ArtifactPath::try_from("main.gcl".to_string()).unwrap(),
            files: vec![
                BundleArtifact {
                    path: ArtifactPath::try_from("main.gcl".to_string()).unwrap(),
                    content: ArtifactContent::Source("node x: Dimensionless = 1.0;".to_string()),
                },
                BundleArtifact {
                    path: ArtifactPath::try_from("plugins/test.wasm".to_string()).unwrap(),
                    content: ArtifactContent::Plugin(vec![0, 255, 128, 1]),
                },
            ],
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let decoded = ProjectBundle::from_json(&json).unwrap();
        assert_eq!(decoded.files[1].content.bytes(), [0, 255, 128, 1]);
        assert!(decoded.mount(Path::new("/report")).is_ok());
        bundle.files.push(bundle.files[0].clone());
        assert!(matches!(
            bundle.mount(Path::new("/report")),
            Err(BundleError::DuplicatePath)
        ));
    }
}
