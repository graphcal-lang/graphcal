//! Durable same-directory file replacement.
//!
//! Formatting and lockfile generation compute bytes before entering this
//! imperative shell. The shell writes those bytes to a temporary file beside
//! the destination, synchronizes them, and only then replaces the destination.

use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use graphcal_compiler::outcome::without_cancellation;
use thiserror::Error;

use crate::{ByteLimit, FileSystemReadError, FileSystemReader, RealFileSystem};

/// Atomically create a regular file with `contents` if the destination remains
/// absent.
///
/// The temporary file is synchronized and then published with a no-clobber
/// rename, so an entry created concurrently is never overwritten. On Unix the
/// new file gets mode `0o666` masked by the umask, and the parent directory
/// is synchronized afterwards so the rename is durable.
///
/// # Errors
///
/// Returns [`AtomicWriteError::DestinationExists`] if any destination entry is
/// present, plus errors from temporary-file preparation or durability
/// synchronization.
pub fn create_file_atomically(destination: &Path, contents: &[u8]) -> Result<(), AtomicWriteError> {
    write_file_atomically_with(
        destination,
        DestinationExpectation::Missing,
        contents,
        |_| Ok(()),
    )
}

/// Atomically replace an existing regular file only if its bytes still equal
/// `expected_contents`.
///
/// This prevents a formatter from knowingly overwriting edits made after it
/// read the source. The comparison is repeated after the replacement is
/// synchronized, immediately before the rename. This is best effort: portable
/// filesystems offer no compare-and-rename, so an edit saved between that last
/// comparison and the rename is still replaced. Permissions are preserved,
/// while ownership, ACLs, extended attributes, and hard-link identity are not.
/// Symbolic links and non-regular destinations are rejected rather than
/// followed.
///
/// # Errors
///
/// Returns [`AtomicWriteError::DestinationChanged`] if the current bytes differ
/// and [`AtomicWriteError::DestinationMissing`] if the file disappeared, plus
/// other temporary-file, replacement, and durability failures.
pub fn replace_file_atomically_if_unchanged(
    destination: &Path,
    expected_contents: &[u8],
    replacement: &[u8],
) -> Result<(), AtomicWriteError> {
    write_file_atomically_with(
        destination,
        DestinationExpectation::Unchanged(expected_contents),
        replacement,
        |_| Ok(()),
    )
}

#[derive(Debug, Clone, Copy)]
enum DestinationExpectation<'a> {
    Missing,
    Unchanged(&'a [u8]),
}

fn write_file_atomically_with(
    destination: &Path,
    expectation: DestinationExpectation<'_>,
    replacement: &[u8],
    before_replace: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), AtomicWriteError> {
    let directory = destination_directory(destination)?;
    // Fail fast before writing anything; the authoritative checks happen at
    // publication below.
    let permissions = validate_destination_expectation(destination, expectation)?;

    let mut builder = tempfile::Builder::new();
    builder.prefix(".graphcal-write-");
    if let Some(permissions) = new_file_permissions(expectation) {
        builder.permissions(permissions);
    }
    let mut temporary =
        builder
            .tempfile_in(directory)
            .map_err(|source| AtomicWriteError::CreateTemporary {
                directory: directory.to_path_buf(),
                source,
            })?;
    temporary
        .write_all(replacement)
        .and_then(|()| temporary.flush())
        .map_err(|source| AtomicWriteError::WriteTemporary {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|source| AtomicWriteError::SetPermissions {
                path: temporary.path().to_path_buf(),
                source,
            })?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| AtomicWriteError::SyncTemporary {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    before_replace(temporary.path()).map_err(|source| AtomicWriteError::PrepareReplacement {
        destination: destination.to_path_buf(),
        source,
    })?;

    let temporary_path = temporary.path().to_path_buf();
    let replace_error = |source| AtomicWriteError::Replace {
        temporary: temporary_path.clone(),
        destination: destination.to_path_buf(),
        source,
    };
    match expectation {
        // Creation is exact: the no-clobber rename fails atomically if any
        // entry appeared at the destination, however late.
        DestinationExpectation::Missing => {
            temporary
                .persist_noclobber(destination)
                .map_err(|error| match error.error.kind() {
                    io::ErrorKind::AlreadyExists => AtomicWriteError::DestinationExists {
                        destination: destination.to_path_buf(),
                    },
                    _ => replace_error(error.error),
                })?;
        }
        // Replacement is best effort: the expectation is re-checked after the
        // slow `sync_all`, immediately before the rename, which leaves only
        // the window between this comparison and the rename itself.
        DestinationExpectation::Unchanged(_) => {
            validate_destination_expectation(destination, expectation)?;
            temporary
                .persist(destination)
                .map_err(|error| replace_error(error.error))?;
        }
    }
    sync_directory(directory)?;
    Ok(())
}

/// Mode of a newly created file: like any ordinary file creation,
/// `0o666` masked by the process umask (the temporary-file default `0o600`
/// would make e.g. a lockfile unreadable to other users of a checkout).
/// Replacements instead copy the existing destination's permissions.
#[cfg(unix)]
fn new_file_permissions(expectation: DestinationExpectation<'_>) -> Option<Permissions> {
    use std::os::unix::fs::PermissionsExt;
    match expectation {
        DestinationExpectation::Missing => Some(Permissions::from_mode(0o666)),
        DestinationExpectation::Unchanged(_) => None,
    }
}

/// Non-Unix platforms have no umask-relative mode to request.
#[cfg(not(unix))]
const fn new_file_permissions(_expectation: DestinationExpectation<'_>) -> Option<Permissions> {
    None
}

fn destination_directory(destination: &Path) -> Result<&Path, AtomicWriteError> {
    match destination.parent() {
        Some(directory) if directory.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(directory) => Ok(directory),
        None => Err(AtomicWriteError::MissingParent {
            destination: destination.to_path_buf(),
        }),
    }
}

fn validate_destination_expectation(
    destination: &Path,
    expectation: DestinationExpectation<'_>,
) -> Result<Option<Permissions>, AtomicWriteError> {
    match expectation {
        DestinationExpectation::Missing => ensure_destination_missing(destination).map(|()| None),
        DestinationExpectation::Unchanged(expected_contents) => {
            validate_unchanged_destination(destination, expected_contents).map(Some)
        }
    }
}

fn ensure_destination_missing(destination: &Path) -> Result<(), AtomicWriteError> {
    match fs::symlink_metadata(destination) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AtomicWriteError::InspectDestination {
            destination: destination.to_path_buf(),
            source,
        }),
        Ok(_) => Err(AtomicWriteError::DestinationExists {
            destination: destination.to_path_buf(),
        }),
    }
}

fn inspect_existing_destination(destination: &Path) -> Result<Permissions, AtomicWriteError> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(metadata.permissions()),
        Ok(_) => Err(AtomicWriteError::UnsupportedDestination {
            destination: destination.to_path_buf(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Err(AtomicWriteError::DestinationMissing {
                destination: destination.to_path_buf(),
            })
        }
        Err(source) => Err(AtomicWriteError::InspectDestination {
            destination: destination.to_path_buf(),
            source,
        }),
    }
}

fn validate_unchanged_destination(
    destination: &Path,
    expected_contents: &[u8],
) -> Result<Permissions, AtomicWriteError> {
    inspect_existing_destination(destination)?;
    let unchanged = destination_contents_equal(destination, expected_contents)?;
    let permissions = inspect_existing_destination(destination)?;
    if unchanged {
        Ok(permissions)
    } else {
        Err(AtomicWriteError::DestinationChanged {
            destination: destination.to_path_buf(),
        })
    }
}

/// Compare through the bounded regular-file read, which validates the opened
/// handle, so a destination swapped for a FIFO cannot block the comparison
/// and a grown file is never read past one byte beyond the expectation.
fn destination_contents_equal(
    destination: &Path,
    expected_contents: &[u8],
) -> Result<bool, AtomicWriteError> {
    let limit = ByteLimit::new(u64::try_from(expected_contents.len()).unwrap_or(u64::MAX));
    match without_cancellation(|cancellation| {
        RealFileSystem::default().read_bytes_bounded(destination, limit, cancellation)
    }) {
        Ok(current) => Ok(current == expected_contents),
        Err(FileSystemReadError::ByteLimitExceeded { .. }) => Ok(false),
        Err(FileSystemReadError::Io(source)) => Err(AtomicWriteError::ReadDestination {
            destination: destination.to_path_buf(),
            source,
        }),
        Err(error @ FileSystemReadError::EntryLimitExceeded { .. }) => {
            Err(AtomicWriteError::ReadDestination {
                destination: destination.to_path_buf(),
                source: io::Error::other(error),
            })
        }
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), AtomicWriteError> {
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|source| AtomicWriteError::SyncDirectory {
            directory: directory.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), AtomicWriteError> {
    // Stable `std` has no portable way to open a directory for synchronization.
    Ok(())
}

/// Failure to prepare, commit, or synchronize an atomic file replacement.
#[derive(Debug, Error)]
pub enum AtomicWriteError {
    /// The destination spelling had no directory component.
    #[error("destination `{destination}` has no parent directory")]
    MissingParent {
        /// Intended destination.
        destination: PathBuf,
    },
    /// Existing destination metadata could not be read.
    #[error("could not inspect destination `{destination}`: {source}")]
    InspectDestination {
        /// Intended destination.
        destination: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// Atomic creation requires an absent destination.
    #[error("destination `{destination}` appeared before atomic creation completed")]
    DestinationExists {
        /// Intended destination.
        destination: PathBuf,
    },
    /// Conditional replacement requires an existing destination.
    #[error("destination `{destination}` disappeared before replacement")]
    DestinationMissing {
        /// Intended destination.
        destination: PathBuf,
    },
    /// The destination was a symlink, directory, or special file.
    #[error("destination `{destination}` is not a regular file; refusing to replace it")]
    UnsupportedDestination {
        /// Intended destination.
        destination: PathBuf,
    },
    /// The destination changed after the caller read it.
    #[error("destination `{destination}` changed while replacement was being prepared")]
    DestinationChanged {
        /// Intended destination.
        destination: PathBuf,
    },
    /// Current destination bytes could not be re-read for comparison.
    #[error("could not re-read destination `{destination}`: {source}")]
    ReadDestination {
        /// Intended destination.
        destination: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// Same-directory temporary-file creation failed.
    #[error("could not create a temporary file in `{directory}`: {source}")]
    CreateTemporary {
        /// Destination directory.
        directory: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// Writing or flushing the temporary file failed.
    #[error("could not write temporary file `{path}`: {source}")]
    WriteTemporary {
        /// Temporary path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// Applying preserved destination permissions failed.
    #[error("could not set permissions on temporary file `{path}`: {source}")]
    SetPermissions {
        /// Temporary path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// Synchronizing the complete temporary file failed.
    #[error("could not synchronize temporary file `{path}`: {source}")]
    SyncTemporary {
        /// Temporary path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// A pre-replacement safety step failed.
    #[error("could not prepare replacement of `{destination}`: {source}")]
    PrepareReplacement {
        /// Intended destination.
        destination: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// Committing the same-directory rename failed.
    #[error("could not replace `{destination}` from `{temporary}`: {source}")]
    Replace {
        /// Fully written temporary path.
        temporary: PathBuf,
        /// Intended destination.
        destination: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// The replacement succeeded, but synchronizing its directory failed.
    #[error("replaced the file but could not synchronize directory `{directory}`: {source}")]
    SyncDirectory {
        /// Destination directory.
        directory: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_pre_replace_step_preserves_the_original_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("model.gcl");
        fs::write(&destination, b"original").unwrap();

        let error = write_file_atomically_with(
            &destination,
            DestinationExpectation::Unchanged(b"original"),
            b"replacement",
            |_| Err(io::Error::other("injected failure before rename")),
        )
        .unwrap_err();

        assert!(matches!(error, AtomicWriteError::PrepareReplacement { .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_creation_never_overwrites_an_entry_created_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("graphcal.lock");

        let error = write_file_atomically_with(
            &destination,
            DestinationExpectation::Missing,
            b"ours",
            |_| fs::write(&destination, b"theirs"),
        )
        .unwrap_err();

        assert!(
            matches!(error, AtomicWriteError::DestinationExists { .. }),
            "{error:?}"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"theirs");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn conditional_replacement_rechecks_after_synchronizing() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("model.gcl");
        fs::write(&destination, b"original").unwrap();

        let error = write_file_atomically_with(
            &destination,
            DestinationExpectation::Unchanged(b"original"),
            b"formatted",
            |_| fs::write(&destination, b"edit saved concurrently"),
        )
        .unwrap_err();

        assert!(
            matches!(error, AtomicWriteError::DestinationChanged { .. }),
            "{error:?}"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"edit saved concurrently");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn conditional_replacement_rejects_a_grown_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("model.gcl");
        fs::write(&destination, b"original plus more").unwrap();

        let error = replace_file_atomically_if_unchanged(&destination, b"original", b"formatted")
            .unwrap_err();

        assert!(matches!(error, AtomicWriteError::DestinationChanged { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_creation_uses_the_umask_masked_default_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        // An ordinary creation requests 0o666 and is masked by the umask.
        let ordinary = directory.path().join("ordinary");
        fs::File::create(&ordinary).unwrap();
        let destination = directory.path().join("graphcal.lock");

        create_file_atomically(&destination, b"lock").unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&destination), mode(&ordinary));
    }

    #[test]
    fn atomic_creation_refuses_to_overwrite_an_existing_entry() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("graphcal.lock");
        fs::write(&destination, b"existing").unwrap();

        let error = create_file_atomically(&destination, b"replacement").unwrap_err();

        assert!(matches!(error, AtomicWriteError::DestinationExists { .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"existing");
    }

    #[test]
    fn conditional_replacement_refuses_to_overwrite_newer_contents() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("model.gcl");
        fs::write(&destination, b"newer edit").unwrap();

        let error = replace_file_atomically_if_unchanged(
            &destination,
            b"stale snapshot",
            b"formatted stale snapshot",
        )
        .unwrap_err();

        assert!(matches!(error, AtomicWriteError::DestinationChanged { .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"newer edit");
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("model.gcl");
        fs::write(&destination, b"old").unwrap();
        fs::set_permissions(&destination, Permissions::from_mode(0o640)).unwrap();

        replace_file_atomically_if_unchanged(&destination, b"old", b"new").unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_destinations_are_not_followed_or_replaced() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.gcl");
        let destination = directory.path().join("link.gcl");
        fs::write(&target, b"original").unwrap();
        symlink(&target, &destination).unwrap();

        let error = replace_file_atomically_if_unchanged(&destination, b"original", b"replacement")
            .unwrap_err();

        assert!(matches!(
            error,
            AtomicWriteError::UnsupportedDestination { .. }
        ));
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
