use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PROJECT_ROOT: &str = "/playground";
const MANIFEST_FILE: &str = "graphcal.toml";

/// Maximum number of files in one browser playground request.
pub const MAX_PLAYGROUND_FILES: usize = 16;
/// Maximum UTF-8 byte length of one file's contents.
pub const MAX_PLAYGROUND_FILE_BYTES: usize = 256 * 1024;
/// Maximum aggregate UTF-8 byte length of all file contents.
pub const MAX_PLAYGROUND_CONTENT_BYTES: usize = 1024 * 1024;
/// Maximum UTF-8 byte length of an entry path or file path.
pub const MAX_PLAYGROUND_PATH_BYTES: usize = 1024;
/// Maximum aggregate UTF-8 byte length of the entry path and all file paths.
pub const MAX_PLAYGROUND_PATH_TOTAL_BYTES: usize = 16 * 1024;

/// One browser-supplied project evaluation request.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaygroundRequest {
    /// Relative path of the `.gcl` entry file.
    pub entry: String,
    /// Every source file available to the in-memory project.
    pub files: Vec<PlaygroundFile>,
}

/// One text file in a browser-supplied project.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaygroundFile {
    /// Project-root-relative path.
    pub path: String,
    /// UTF-8 file contents.
    pub content: String,
}

/// Location of a path within a playground request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPathRole {
    /// The request's entry path.
    Entry,
    /// One element of the request's file array.
    File {
        /// Zero-based position in the file array.
        index: usize,
    },
}

impl std::fmt::Display for ProjectPathRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entry => formatter.write_str("entry path"),
            Self::File { index } => write!(formatter, "path of project file at index {index}"),
        }
    }
}

/// Validated aggregate request-size state.
///
/// Private fields prevent callers from constructing a state that has skipped
/// an individual or aggregate limit check.
#[derive(Debug, Clone, Copy)]
pub struct RequestSizeBudget {
    content_bytes: usize,
    path_bytes: usize,
}

impl RequestSizeBudget {
    pub const fn for_entry(entry_path_bytes: usize) -> Result<Self, ProjectValidationError> {
        if entry_path_bytes > MAX_PLAYGROUND_PATH_BYTES {
            return Err(ProjectValidationError::PathTooLong {
                role: ProjectPathRole::Entry,
                maximum: MAX_PLAYGROUND_PATH_BYTES,
            });
        }
        Ok(Self {
            content_bytes: 0,
            path_bytes: entry_path_bytes,
        })
    }

    pub fn with_file(
        self,
        index: usize,
        path_bytes: usize,
        content_bytes: usize,
    ) -> Result<Self, ProjectValidationError> {
        let role = ProjectPathRole::File { index };
        if path_bytes > MAX_PLAYGROUND_PATH_BYTES {
            return Err(ProjectValidationError::PathTooLong {
                role,
                maximum: MAX_PLAYGROUND_PATH_BYTES,
            });
        }
        let path_bytes = self
            .path_bytes
            .checked_add(path_bytes)
            .filter(|total| *total <= MAX_PLAYGROUND_PATH_TOTAL_BYTES);
        let Some(path_bytes) = path_bytes else {
            return Err(ProjectValidationError::ProjectPathsTooLarge {
                maximum: MAX_PLAYGROUND_PATH_TOTAL_BYTES,
            });
        };

        if content_bytes > MAX_PLAYGROUND_FILE_BYTES {
            return Err(ProjectValidationError::FileTooLarge {
                index,
                maximum: MAX_PLAYGROUND_FILE_BYTES,
            });
        }
        let content_bytes = self
            .content_bytes
            .checked_add(content_bytes)
            .filter(|total| *total <= MAX_PLAYGROUND_CONTENT_BYTES);
        let Some(content_bytes) = content_bytes else {
            return Err(ProjectValidationError::ProjectTooLarge {
                maximum: MAX_PLAYGROUND_CONTENT_BYTES,
            });
        };

        Ok(Self {
            content_bytes,
            path_bytes,
        })
    }
}

pub const fn validate_file_count(count: usize) -> Result<(), ProjectValidationError> {
    if count == 0 {
        return Err(ProjectValidationError::NoFiles);
    }
    if count > MAX_PLAYGROUND_FILES {
        return Err(ProjectValidationError::TooManyFiles {
            actual: count,
            maximum: MAX_PLAYGROUND_FILES,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProjectFileKind {
    GraphcalSource,
    Manifest,
}

/// Validated project-relative path retaining its file category.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectFilePath {
    relative: PathBuf,
    absolute: VirtualAbsolutePath,
    kind: ProjectFileKind,
}

impl ProjectFilePath {
    fn parse(raw: &str) -> Result<Self, ProjectValidationError> {
        if raw.is_empty() {
            return Err(ProjectValidationError::EmptyPath);
        }

        let relative = PathBuf::from(raw);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(ProjectValidationError::UnsafePath {
                path: raw.to_string(),
            });
        }

        let kind = if relative == Path::new(MANIFEST_FILE) {
            ProjectFileKind::Manifest
        } else if relative
            .extension()
            .is_some_and(|extension| extension == "gcl")
        {
            ProjectFileKind::GraphcalSource
        } else {
            return Err(ProjectValidationError::UnsupportedFile {
                path: raw.to_string(),
            });
        };

        let absolute =
            VirtualAbsolutePath::new(Path::new(PROJECT_ROOT).join(&relative)).map_err(|_| {
                ProjectValidationError::UnsafePath {
                    path: raw.to_string(),
                }
            })?;
        Ok(Self {
            relative,
            absolute,
            kind,
        })
    }

    fn display(&self) -> String {
        self.relative.to_string_lossy().into_owned()
    }

    const fn absolute(&self) -> &VirtualAbsolutePath {
        &self.absolute
    }
}

#[derive(Debug, Clone)]
struct VirtualFile {
    path: ProjectFilePath,
    content: String,
}

/// A browser project whose paths, size, entry point, and capabilities have
/// already been validated.
#[derive(Debug, Clone)]
pub struct VirtualProject {
    entry: ProjectFilePath,
    filesystem: InMemoryFileSystem,
}

impl TryFrom<PlaygroundRequest> for VirtualProject {
    type Error = ProjectValidationError;

    fn try_from(request: PlaygroundRequest) -> Result<Self, Self::Error> {
        validate_file_count(request.files.len())?;
        let _validated_sizes = request.files.iter().enumerate().try_fold(
            RequestSizeBudget::for_entry(request.entry.len())?,
            |budget, (index, file)| budget.with_file(index, file.path.len(), file.content.len()),
        )?;

        let entry = ProjectFilePath::parse(&request.entry)?;
        if entry.kind != ProjectFileKind::GraphcalSource {
            return Err(ProjectValidationError::EntryIsNotSource {
                path: request.entry,
            });
        }

        let mut seen = HashSet::with_capacity(request.files.len());
        let mut files = Vec::with_capacity(request.files.len());

        for file in request.files {
            let path = ProjectFilePath::parse(&file.path)?;
            if !seen.insert(path.relative.clone()) {
                return Err(ProjectValidationError::DuplicatePath {
                    path: path.display(),
                });
            }

            files.push(VirtualFile {
                path,
                content: file.content,
            });
        }

        if !seen.contains(&entry.relative) {
            return Err(ProjectValidationError::MissingEntry {
                path: entry.display(),
            });
        }

        validate_manifest_capabilities(&files)?;
        let mut filesystem = InMemoryFileSystem::new();
        for file in files {
            filesystem
                .add_file(file.path.absolute().clone(), file.content)
                .map_err(|_| ProjectValidationError::UnsafePath {
                    path: file.path.display(),
                })?;
        }

        Ok(Self { entry, filesystem })
    }
}

impl VirtualProject {
    pub fn entry_path(&self) -> PathBuf {
        self.entry.absolute().clone().into_path_buf()
    }

    pub fn root_path() -> &'static Path {
        Path::new(PROJECT_ROOT)
    }

    pub fn entry_display(&self) -> String {
        self.entry.display()
    }

    pub fn filesystem(&self) -> InMemoryFileSystem {
        self.filesystem.clone()
    }

    pub fn relative_source_name(source_name: &str) -> Option<String> {
        Path::new(source_name)
            .strip_prefix(Self::root_path())
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }
}

fn validate_manifest_capabilities(files: &[VirtualFile]) -> Result<(), ProjectValidationError> {
    let manifest = files
        .iter()
        .find(|file| file.path.kind == ProjectFileKind::Manifest);
    let Some(manifest) = manifest else {
        return Ok(());
    };

    match graphcal_package::parse_manifest_str(&manifest.content) {
        Ok(parsed) if !parsed.dependencies.is_empty() => {
            Err(ProjectValidationError::PackageDependenciesUnsupported)
        }
        // The project loader owns manifest diagnostics. Only a valid manifest
        // with dependencies is intercepted as an unsupported browser feature.
        Ok(_) | Err(_) => Ok(()),
    }
}

/// Rejection raised before the Graphcal compiler is invoked.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProjectValidationError {
    #[error("the project must contain at least one file")]
    NoFiles,
    #[error("the project contains {actual} files; the browser limit is {maximum}")]
    TooManyFiles { actual: usize, maximum: usize },
    #[error("project file paths must not be empty")]
    EmptyPath,
    #[error("project path `{path}` must be relative and must not contain `.` or `..` components")]
    UnsafePath { path: String },
    #[error(
        "project file `{path}` is unsupported; use `.gcl` files and an optional root `graphcal.toml`"
    )]
    UnsupportedFile { path: String },
    #[error("entry `{path}` must be a `.gcl` source file")]
    EntryIsNotSource { path: String },
    #[error("project contains duplicate path `{path}`")]
    DuplicatePath { path: String },
    #[error("entry file `{path}` is not present in the project")]
    MissingEntry { path: String },
    #[error("the {role} exceeds the browser limit of {maximum} UTF-8 bytes")]
    PathTooLong {
        /// Entry or indexed file-path location, without retaining rejected text.
        role: ProjectPathRole,
        /// Maximum accepted UTF-8 byte length.
        maximum: usize,
    },
    #[error("project paths exceed the browser aggregate limit of {maximum} UTF-8 bytes")]
    ProjectPathsTooLarge {
        /// Maximum aggregate UTF-8 byte length.
        maximum: usize,
    },
    #[error("project file at index {index} exceeds the browser limit of {maximum} bytes per file")]
    FileTooLarge {
        /// Zero-based position in the request's file array.
        index: usize,
        /// Maximum accepted UTF-8 content length.
        maximum: usize,
    },
    #[error("project contents exceed the browser aggregate limit of {maximum} bytes")]
    ProjectTooLarge {
        /// Maximum aggregate UTF-8 content length.
        maximum: usize,
    },
    #[error("package dependencies are not supported in the browser playground")]
    PackageDependenciesUnsupported,
    #[error("WASM and host-function plugins are not supported in the browser playground")]
    PluginsUnsupported,
}

/// Browser-facing rejection details for an invalid or unsupported project.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RequestErrorView {
    pub kind: RequestErrorKind,
    pub message: String,
}

impl From<&ProjectValidationError> for RequestErrorView {
    fn from(error: &ProjectValidationError) -> Self {
        Self {
            kind: RequestErrorKind::from(error),
            message: error.to_string(),
        }
    }
}

/// Stable classification for pre-compilation playground rejections.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestErrorKind {
    NoFiles,
    TooManyFiles,
    EmptyPath,
    UnsafePath,
    UnsupportedFile,
    EntryIsNotSource,
    DuplicatePath,
    MissingEntry,
    PathTooLong,
    ProjectPathsTooLarge,
    FileTooLarge,
    ProjectTooLarge,
    PackageDependenciesUnsupported,
    PluginsUnsupported,
}

impl From<&ProjectValidationError> for RequestErrorKind {
    fn from(error: &ProjectValidationError) -> Self {
        match error {
            ProjectValidationError::NoFiles => Self::NoFiles,
            ProjectValidationError::TooManyFiles { .. } => Self::TooManyFiles,
            ProjectValidationError::EmptyPath => Self::EmptyPath,
            ProjectValidationError::UnsafePath { .. } => Self::UnsafePath,
            ProjectValidationError::UnsupportedFile { .. } => Self::UnsupportedFile,
            ProjectValidationError::EntryIsNotSource { .. } => Self::EntryIsNotSource,
            ProjectValidationError::DuplicatePath { .. } => Self::DuplicatePath,
            ProjectValidationError::MissingEntry { .. } => Self::MissingEntry,
            ProjectValidationError::PathTooLong { .. } => Self::PathTooLong,
            ProjectValidationError::ProjectPathsTooLarge { .. } => Self::ProjectPathsTooLarge,
            ProjectValidationError::FileTooLarge { .. } => Self::FileTooLarge,
            ProjectValidationError::ProjectTooLarge { .. } => Self::ProjectTooLarge,
            ProjectValidationError::PackageDependenciesUnsupported => {
                Self::PackageDependenciesUnsupported
            }
            ProjectValidationError::PluginsUnsupported => Self::PluginsUnsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(entry: &str, files: &[(&str, &str)]) -> PlaygroundRequest {
        PlaygroundRequest {
            entry: entry.to_string(),
            files: files
                .iter()
                .map(|(path, content)| PlaygroundFile {
                    path: (*path).to_string(),
                    content: (*content).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn validates_single_file_project() {
        let project = VirtualProject::try_from(request(
            "main.gcl",
            &[("main.gcl", "node x: Dimensionless = 1.0;")],
        ))
        .unwrap();
        assert_eq!(project.entry_path(), PathBuf::from("/playground/main.gcl"));
    }

    #[test]
    fn rejects_invalid_project_shapes() {
        for (request, expected) in [
            (request("main.gcl", &[]), ProjectValidationError::NoFiles),
            (
                request("graphcal.toml", &[("graphcal.toml", "")]),
                ProjectValidationError::EntryIsNotSource {
                    path: "graphcal.toml".to_string(),
                },
            ),
            (
                request("main.gcl", &[("other.gcl", "")]),
                ProjectValidationError::MissingEntry {
                    path: "main.gcl".to_string(),
                },
            ),
            (
                request("main.gcl", &[("main.gcl", ""), ("notes.txt", "")]),
                ProjectValidationError::UnsupportedFile {
                    path: "notes.txt".to_string(),
                },
            ),
            (
                request("main.gcl", &[("main.gcl", ""), ("", "")]),
                ProjectValidationError::EmptyPath,
            ),
        ] {
            assert_eq!(VirtualProject::try_from(request).unwrap_err(), expected);
        }
    }

    #[test]
    fn rejects_parent_traversal() {
        let error = VirtualProject::try_from(request(
            "../main.gcl",
            &[("../main.gcl", "node x: Dimensionless = 1.0;")],
        ))
        .unwrap_err();
        assert!(matches!(error, ProjectValidationError::UnsafePath { .. }));
    }

    #[test]
    fn enforces_file_count_limit() {
        let files = (0..=MAX_PLAYGROUND_FILES)
            .map(|index| PlaygroundFile {
                path: format!("{index}.gcl"),
                content: String::new(),
            })
            .collect();
        let error = VirtualProject::try_from(PlaygroundRequest {
            entry: "0.gcl".to_string(),
            files,
        })
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::TooManyFiles {
                actual: MAX_PLAYGROUND_FILES + 1,
                maximum: MAX_PLAYGROUND_FILES,
            }
        );
    }

    #[test]
    fn enforces_path_and_aggregate_metadata_limits_before_path_parsing() {
        let exact_path = format!("{}.gcl", "x".repeat(MAX_PLAYGROUND_PATH_BYTES - 4));
        assert!(
            VirtualProject::try_from(request(&exact_path, &[(&exact_path, "")])).is_ok(),
            "a path exactly at the byte limit should be accepted"
        );

        let oversized_path = format!("{}.gcl", "x".repeat(MAX_PLAYGROUND_PATH_BYTES - 3));
        let entry_error = VirtualProject::try_from(request(
            &oversized_path,
            &[("main.gcl", "node x: Dimensionless = 1.0;")],
        ))
        .unwrap_err();
        assert_eq!(
            entry_error,
            ProjectValidationError::PathTooLong {
                role: ProjectPathRole::Entry,
                maximum: MAX_PLAYGROUND_PATH_BYTES,
            }
        );

        let file_error = VirtualProject::try_from(request(
            "main.gcl",
            &[("main.gcl", ""), (&oversized_path, "")],
        ))
        .unwrap_err();
        assert_eq!(
            file_error,
            ProjectValidationError::PathTooLong {
                role: ProjectPathRole::File { index: 1 },
                maximum: MAX_PLAYGROUND_PATH_BYTES,
            }
        );

        let files: Vec<PlaygroundFile> = (0..MAX_PLAYGROUND_FILES)
            .map(|index| PlaygroundFile {
                path: format!(
                    "{index:02}{}.gcl",
                    "x".repeat(MAX_PLAYGROUND_PATH_BYTES - 6)
                ),
                content: String::new(),
            })
            .collect();
        let aggregate_error = VirtualProject::try_from(PlaygroundRequest {
            entry: files[0].path.clone(),
            files,
        })
        .unwrap_err();
        assert_eq!(
            aggregate_error,
            ProjectValidationError::ProjectPathsTooLarge {
                maximum: MAX_PLAYGROUND_PATH_TOTAL_BYTES,
            }
        );
    }

    #[test]
    fn enforces_file_size_limit() {
        let error = VirtualProject::try_from(PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: "x".repeat(MAX_PLAYGROUND_FILE_BYTES + 1),
            }],
        })
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::FileTooLarge {
                index: 0,
                maximum: MAX_PLAYGROUND_FILE_BYTES,
            }
        );
    }

    #[test]
    fn enforces_total_project_size_limit() {
        let files = (0..5)
            .map(|index| PlaygroundFile {
                path: format!("{index}.gcl"),
                content: "x".repeat(if index < 4 {
                    MAX_PLAYGROUND_FILE_BYTES
                } else {
                    1
                }),
            })
            .collect();
        let error = VirtualProject::try_from(PlaygroundRequest {
            entry: "0.gcl".to_string(),
            files,
        })
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::ProjectTooLarge {
                maximum: MAX_PLAYGROUND_CONTENT_BYTES,
            }
        );
    }

    #[test]
    fn accepts_exact_size_limits() {
        let files = (0..4)
            .map(|index| PlaygroundFile {
                path: format!("{index}.gcl"),
                content: "x".repeat(MAX_PLAYGROUND_FILE_BYTES),
            })
            .collect();
        assert!(
            VirtualProject::try_from(PlaygroundRequest {
                entry: "0.gcl".to_string(),
                files,
            })
            .is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_paths() {
        let error =
            VirtualProject::try_from(request("main.gcl", &[("main.gcl", ""), ("main.gcl", "")]))
                .unwrap_err();
        assert!(matches!(
            error,
            ProjectValidationError::DuplicatePath { .. }
        ));
    }

    #[test]
    fn rejects_file_and_directory_path_conflicts() {
        let error = VirtualProject::try_from(request(
            "main.gcl",
            &[
                ("main.gcl", ""),
                ("module.gcl", ""),
                ("module.gcl/child.gcl", ""),
            ],
        ))
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::UnsafePath {
                path: "module.gcl/child.gcl".to_string(),
            }
        );
    }

    #[test]
    fn rejects_manifest_dependencies() {
        let error = VirtualProject::try_from(request(
            "src/demo/main.gcl",
            &[
                (
                    "graphcal.toml",
                    "[package]\nname = \"demo\"\n\n[dependencies]\nunits = { git = \"https://example.com/units\", rev = \"1111111111111111111111111111111111111111\" }\n",
                ),
                ("src/demo/main.gcl", "node x: Dimensionless = 1.0;"),
            ],
        ))
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::PackageDependenciesUnsupported
        );
    }
}
