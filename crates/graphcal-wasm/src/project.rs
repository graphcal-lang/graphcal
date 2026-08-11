use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PROJECT_ROOT: &str = "/playground";
const MANIFEST_FILE: &str = "graphcal.toml";
const MAX_FILES: usize = 16;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_PROJECT_BYTES: usize = 1024 * 1024;

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
        if request.files.is_empty() {
            return Err(ProjectValidationError::NoFiles);
        }
        if request.files.len() > MAX_FILES {
            return Err(ProjectValidationError::TooManyFiles {
                actual: request.files.len(),
                maximum: MAX_FILES,
            });
        }

        let entry = ProjectFilePath::parse(&request.entry)?;
        if entry.kind != ProjectFileKind::GraphcalSource {
            return Err(ProjectValidationError::EntryIsNotSource {
                path: request.entry,
            });
        }

        let mut seen = HashSet::with_capacity(request.files.len());
        let mut total_bytes = 0usize;
        let mut files = Vec::with_capacity(request.files.len());

        for file in request.files {
            let path = ProjectFilePath::parse(&file.path)?;
            if !seen.insert(path.relative.clone()) {
                return Err(ProjectValidationError::DuplicatePath {
                    path: path.display(),
                });
            }

            let file_bytes = file.content.len();
            if file_bytes > MAX_FILE_BYTES {
                return Err(ProjectValidationError::FileTooLarge {
                    path: path.display(),
                    actual: file_bytes,
                    maximum: MAX_FILE_BYTES,
                });
            }
            total_bytes = total_bytes.checked_add(file_bytes).ok_or(
                ProjectValidationError::ProjectTooLarge {
                    actual: usize::MAX,
                    maximum: MAX_PROJECT_BYTES,
                },
            )?;
            if total_bytes > MAX_PROJECT_BYTES {
                return Err(ProjectValidationError::ProjectTooLarge {
                    actual: total_bytes,
                    maximum: MAX_PROJECT_BYTES,
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
    #[error("file `{path}` is {actual} bytes; the browser limit is {maximum} bytes per file")]
    FileTooLarge {
        path: String,
        actual: usize,
        maximum: usize,
    },
    #[error("project is {actual} bytes; the browser limit is {maximum} bytes")]
    ProjectTooLarge { actual: usize, maximum: usize },
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
        let files = (0..=MAX_FILES)
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
                actual: MAX_FILES + 1,
                maximum: MAX_FILES,
            }
        );
    }

    #[test]
    fn enforces_file_size_limit() {
        let error = VirtualProject::try_from(PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: "x".repeat(MAX_FILE_BYTES + 1),
            }],
        })
        .unwrap_err();
        assert_eq!(
            error,
            ProjectValidationError::FileTooLarge {
                path: "main.gcl".to_string(),
                actual: MAX_FILE_BYTES + 1,
                maximum: MAX_FILE_BYTES,
            }
        );
    }

    #[test]
    fn enforces_total_project_size_limit() {
        let files = (0..5)
            .map(|index| PlaygroundFile {
                path: format!("{index}.gcl"),
                content: "x".repeat(if index < 4 { MAX_FILE_BYTES } else { 1 }),
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
                actual: MAX_PROJECT_BYTES + 1,
                maximum: MAX_PROJECT_BYTES,
            }
        );
    }

    #[test]
    fn accepts_exact_size_limits() {
        let files = (0..4)
            .map(|index| PlaygroundFile {
                path: format!("{index}.gcl"),
                content: "x".repeat(MAX_FILE_BYTES),
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
