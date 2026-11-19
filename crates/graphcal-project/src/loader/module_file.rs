//! The module identity that a package source file's name gives it.
//!
//! In a real package every directory and `.gcl` file stem below the source
//! directory is a module path segment: `<source_dir>/nasa/rocket.gcl` is the
//! module `nasa.rocket`. [`ModuleFilePath::parse`] is the only place where
//! on-disk names become module path segments, so every package file the
//! loader accepts has a name that an `import` can spell.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use graphcal_compiler::syntax::module_path_key::ModulePathKey;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::token::{SourceIdentifier, SourceIdentifierError};

use crate::load_error::LoadError;

/// Extension of every Graphcal source file.
const SOURCE_EXTENSION: &str = "gcl";

/// A `.gcl` file path, relative to its package's source directory, whose
/// every component is a module path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModuleFilePath {
    segments: NonEmpty<NameAtom>,
}

/// Why a path below a source directory names no module file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ModuleFilePathError {
    /// The path is empty, absolute, or contains `.` or `..` components.
    #[error("the path must be a plain relative path below the source directory")]
    NotRelative,
    /// A component is not valid UTF-8.
    #[error("the path contains a component that is not valid UTF-8")]
    NonUtf8,
    /// The file does not have the `.gcl` extension.
    #[error("the file does not have the `.gcl` extension")]
    NotGcl,
    /// A directory name or file stem is not a module path segment.
    #[error("`{name}` {reason}")]
    InvalidName {
        /// The offending directory name or file stem.
        name: String,
        reason: ModuleNameError,
    },
}

/// Why a directory name or file stem is not a module path segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum ModuleNameError {
    /// Not one source identifier (`my-helpers`, `2d`, ...).
    #[error("is not a Graphcal identifier")]
    NotIdentifier,
    /// A hard keyword (`match`, `node`, ...).
    #[error("is a reserved Graphcal keyword")]
    Keyword,
    /// An identifier that is not lower snake case (`MyModule`).
    #[error("is not lower_snake_case")]
    NotLowerSnakeCase,
}

impl ModuleFilePath {
    /// Validate `relative`, a file path below a package source directory.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleFilePathError`] when the path is not relative, is not
    /// a `.gcl` file, or has a directory name or file stem that is not a
    /// lower-snake-case, non-keyword Graphcal identifier.
    pub(super) fn parse(relative: &Path) -> Result<Self, ModuleFilePathError> {
        let mut components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => name.to_str().ok_or(ModuleFilePathError::NonUtf8),
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => Err(ModuleFilePathError::NotRelative),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let file = components.pop().ok_or(ModuleFilePathError::NotRelative)?;
        let stem = Path::new(file)
            .extension()
            .filter(|extension| *extension == SOURCE_EXTENSION)
            .and_then(|_| file.strip_suffix(".gcl"))
            .ok_or(ModuleFilePathError::NotGcl)?;
        components.push(stem);
        let segments = components
            .into_iter()
            .map(|name| {
                module_segment(name).map_err(|reason| ModuleFilePathError::InvalidName {
                    name: name.to_owned(),
                    reason,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        NonEmpty::try_from_vec(segments)
            .map(|segments| Self { segments })
            .map_err(|_| ModuleFilePathError::NotRelative)
    }

    /// The module path that names this file within its package.
    pub(super) fn module_path(&self) -> ModulePathKey {
        ModulePathKey::new(self.segments.clone())
    }
}

/// Why a package file cannot be loaded under the path that reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ModuleFileFailure {
    /// The file's path below the source directory names no module.
    Invalid {
        path: PathBuf,
        error: ModuleFilePathError,
    },
    /// A component of `path` is spelled differently from its directory entry.
    SpellingMismatch {
        path: PathBuf,
        on_disk: Option<OsString>,
    },
    /// A directory on the path could not be listed. `reason` is host text.
    Unreadable { path: PathBuf, reason: String },
    /// The file, reached through a symlink, lies outside the source
    /// directory, so no module path below it names the file.
    OutsideSourceDir { path: PathBuf, source_dir: PathBuf },
}

impl ModuleFileFailure {
    /// The diagnostic for this failure.
    pub(super) fn to_load_error(&self) -> LoadError {
        match self {
            Self::Invalid {
                path,
                error: error @ ModuleFilePathError::InvalidName { .. },
            } => LoadError::InvalidModuleFileName {
                path: path.clone(),
                reason: error.to_string(),
            },
            Self::Invalid { path, error } => LoadError::InvalidSourcePath {
                path: path.clone(),
                reason: error.to_string(),
            },
            Self::SpellingMismatch { path, on_disk } => LoadError::ModuleFileSpellingMismatch {
                path: path.clone(),
                on_disk: on_disk
                    .as_ref()
                    .map(|name| name.to_string_lossy().into_owned()),
            },
            Self::Unreadable { path, reason } => LoadError::SourceReadFailed {
                path: path.clone(),
                reason: reason.clone(),
            },
            Self::OutsideSourceDir { path, source_dir } => LoadError::ModuleOutsideSourceDir {
                path: path.clone(),
                source_dir: source_dir.clone(),
            },
        }
    }
}

/// Validate one directory name or file stem as a module path segment.
fn module_segment(name: &str) -> Result<NameAtom, ModuleNameError> {
    let identifier = SourceIdentifier::parse(name).map_err(|error| match error {
        SourceIdentifierError::InvalidCharacters => ModuleNameError::NotIdentifier,
        SourceIdentifierError::ReservedKeyword => ModuleNameError::Keyword,
    })?;
    let lower_snake_case = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if lower_snake_case {
        Ok(identifier.into_atom())
    } else {
        Err(ModuleNameError::NotLowerSnakeCase)
    }
}

#[cfg(test)]
mod tests;
