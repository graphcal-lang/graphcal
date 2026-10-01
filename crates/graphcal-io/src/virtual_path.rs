//! Normalized absolute paths of the in-memory virtual filesystem.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Normalized absolute path in an in-memory virtual filesystem.
///
/// Construction removes `.` components and resolves `..` components while
/// rejecting any parent traversal that would escape the virtual root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualAbsolutePath(PathBuf);

impl VirtualAbsolutePath {
    /// Validate and normalize an absolute virtual path.
    ///
    /// # Errors
    ///
    /// Returns [`VirtualPathError::NotAbsolute`] for a relative path and
    /// [`VirtualPathError::EscapesRoot`] when a `..` component crosses the
    /// virtual root.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, VirtualPathError> {
        let path = path.into();
        if !path.has_root() {
            return Err(VirtualPathError::NotAbsolute { path });
        }

        let mut normalized = PathBuf::new();
        let mut normal_components = 0usize;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    normalized.push(component.as_os_str());
                }
                Component::CurDir => {}
                Component::Normal(name) => {
                    normalized.push(name);
                    normal_components += 1;
                }
                Component::ParentDir if normal_components > 0 => {
                    normalized.pop();
                    normal_components -= 1;
                }
                Component::ParentDir => return Err(VirtualPathError::EscapesRoot { path }),
            }
        }
        Ok(Self(normalized))
    }

    /// Borrow the normalized host representation used at filesystem boundaries.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Consume the validated path and return its normalized representation.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for VirtualAbsolutePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// Invalid virtual absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VirtualPathError {
    /// Virtual paths must start at a root.
    #[error("virtual filesystem path `{}` must be absolute", path.display())]
    NotAbsolute {
        /// Rejected spelling.
        path: PathBuf,
    },
    /// Parent traversal attempted to cross the virtual root.
    #[error("virtual filesystem path `{}` escapes the virtual root", path.display())]
    EscapesRoot {
        /// Rejected spelling.
        path: PathBuf,
    },
}
