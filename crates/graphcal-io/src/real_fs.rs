//! Real filesystem implementation backed by ambient `std::fs` access or a
//! held capability directory.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use cap_std::{ambient_authority, fs::Dir};
use graphcal_compiler::{cancellation::CancellationToken, outcome::Outcome};
use sha2::{Digest, Sha256};

#[cfg(not(target_arch = "wasm32"))]
use crate::VirtualAbsolutePath;
use crate::{
    BoundedFileHash, ByteLimit, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader, OutsideRootError,
};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct RootCapability {
    requested_path: VirtualAbsolutePath,
    canonical_path: VirtualAbsolutePath,
    directory: Arc<Dir>,
}

/// Filesystem reader backed by the operating system.
///
/// An unrooted reader delegates to ambient `std::fs`. A rooted reader holds an
/// open directory capability and performs canonicalization, opens, metadata
/// queries, and directory listing relative to that handle. `cap-std` uses
/// `openat2` where available and component-by-component handle traversal on
/// other supported platforms, preventing concurrent symlink swaps from escaping
/// the held root.
#[derive(Debug, Clone, Default)]
pub struct RealFileSystem {
    #[cfg(not(target_arch = "wasm32"))]
    root: Option<RootCapability>,
}

impl RealFileSystem {
    /// Construct a sandboxed filesystem reader pinned to `project_root`.
    ///
    /// Ambient authority is used only here to open the root directory. Every
    /// later rooted operation is relative to the resulting held capability.
    ///
    /// # Errors
    ///
    /// Returns an error when `project_root` does not exist, is not a directory,
    /// or cannot be opened as a capability.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn rooted(project_root: &Path) -> Result<Self, io::Error> {
        let canonical = project_root.canonicalize()?;
        let canonical_path = VirtualAbsolutePath::new(canonical.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let requested_path = VirtualAbsolutePath::new(project_root.to_path_buf())
            .unwrap_or_else(|_| canonical_path.clone());
        let directory = Dir::open_ambient_dir(&canonical, ambient_authority())?;
        Ok(Self {
            root: Some(RootCapability {
                requested_path,
                canonical_path,
                directory: Arc::new(directory),
            }),
        })
    }

    /// Rooted ambient filesystems are unavailable in bare Wasm environments.
    ///
    /// # Errors
    ///
    /// Always returns [`io::ErrorKind::Unsupported`].
    #[cfg(target_arch = "wasm32")]
    pub fn rooted(_project_root: &Path) -> Result<Self, io::Error> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "rooted ambient filesystems are unavailable on wasm32",
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn outside_root_error(path: &Path, root: &Path) -> io::Error {
        OutsideRootError::new(path.to_path_buf(), root.to_path_buf()).into_io_error()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn rooted_relative_path(root: &RootCapability, path: &Path) -> Result<PathBuf, io::Error> {
        if let Ok(normalized) = VirtualAbsolutePath::new(path.to_path_buf()) {
            for root_path in [&root.canonical_path, &root.requested_path] {
                if let Ok(relative) = normalized.as_path().strip_prefix(root_path.as_path()) {
                    return Ok(cap_relative_path(relative));
                }
            }
        }

        // This fallback maps an existing non-canonical ambient spelling (for
        // example macOS `/var` versus `/private/var`) into the held root. The
        // resulting operation still uses only the relative capability path, so
        // a race here can change which in-root entry is selected but cannot
        // grant access outside the root.
        let canonical = path.canonicalize()?;
        canonical
            .strip_prefix(root.canonical_path.as_path())
            .map(cap_relative_path)
            .map_err(|_| Self::outside_root_error(&canonical, root.canonical_path.as_path()))
    }

    /// Canonicalize a root-relative path without opening a special file.
    ///
    /// `cap-std` canonicalizes by opening every component, and on platforms
    /// without `O_PATH` (macOS) opening a FIFO for reading blocks until a
    /// writer appears. Regular files and directories are canonicalized
    /// normally; any other entry is resolved through its parent directory
    /// only, so the entry itself is never opened. Such an entry can never be
    /// read (see [`Self::open_regular_with_hook`]), so its canonical spelling
    /// only has to stay inside the root, which the parent's resolution ensures.
    #[cfg(not(target_arch = "wasm32"))]
    fn rooted_canonical_relative(
        root: &RootCapability,
        relative: &Path,
    ) -> Result<PathBuf, io::Error> {
        let canonicalize = |relative: &Path| {
            root.directory
                .canonicalize(relative)
                .map_err(|error| root.classify_error(relative, error))
        };
        let metadata = root
            .directory
            .metadata(relative)
            .map_err(|error| root.classify_error(relative, error))?;
        if metadata.is_file() || metadata.is_dir() {
            return canonicalize(relative);
        }
        match (relative.parent(), relative.file_name()) {
            (Some(parent), Some(name)) => Ok(canonicalize(&cap_relative_path(parent))?.join(name)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot canonicalize a special file without a parent directory",
            )),
        }
    }

    fn read_file_bounded(
        mut file: impl Read,
        declared_len: u64,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        if declared_len > limit.get() {
            return Err(FileSystemReadError::ByteLimitExceeded { limit }.into());
        }
        let capacity = usize::try_from(declared_len)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "file length cannot be represented on this platform",
                )
            })
            .map_err(FileSystemReadError::Io)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("could not reserve bounded file buffer: {error}"),
                )
            })
            .map_err(FileSystemReadError::Io)?;
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            cancellation.checkpoint()?;
            let read = file.read(&mut chunk).map_err(FileSystemReadError::Io)?;
            if read == 0 {
                return Ok(bytes);
            }
            let next_len = (bytes.len() as u64).saturating_add(read as u64);
            if next_len > limit.get() {
                return Err(FileSystemReadError::ByteLimitExceeded { limit }.into());
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
    }

    fn hash_file_bounded(
        mut file: impl Read,
        declared_len: u64,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<BoundedFileHash, Outcome<FileSystemReadError>> {
        if declared_len > limit.get() {
            return Err(FileSystemReadError::ByteLimitExceeded { limit }.into());
        }
        let mut hasher = Sha256::new();
        let mut bytes = 0_u64;
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            cancellation.checkpoint()?;
            let read = file.read(&mut chunk).map_err(FileSystemReadError::Io)?;
            if read == 0 {
                return Ok(BoundedFileHash::new(hasher.finalize().into(), bytes));
            }
            bytes = bytes
                .checked_add(read as u64)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::FileTooLarge, "file byte count overflow")
                })
                .map_err(FileSystemReadError::Io)?;
            if bytes > limit.get() {
                return Err(FileSystemReadError::ByteLimitExceeded { limit }.into());
            }
            hasher.update(&chunk[..read]);
        }
    }

    /// Check before opening to avoid touching known devices, then check the
    /// handle too. On Unix a substituted FIFO cannot block the open itself.
    fn open_regular_with_hook(
        &self,
        path: &Path,
        before_open: impl FnOnce(),
    ) -> Result<File, io::Error> {
        let reject = || io::Error::new(io::ErrorKind::InvalidInput, "cannot read non-regular file");
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            if !root
                .directory
                .metadata(&relative)
                .map_err(|error| root.classify_error(&relative, error))?
                .is_file()
            {
                return Err(reject());
            }
            before_open();
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
            }
            let file = root
                .directory
                .open_with(&relative, &options)
                .map_err(|error| root.classify_error(&relative, error))?
                .into_std();
            return if file.metadata()?.is_file() {
                Ok(file)
            } else {
                Err(reject())
            };
        }
        if !std::fs::metadata(path)?.is_file() {
            return Err(reject());
        }
        before_open();
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
        }
        let file = options.open(path)?;
        if file.metadata()?.is_file() {
            Ok(file)
        } else {
            Err(reject())
        }
    }

    fn read_bytes_bounded_with_hook(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
        before_open: impl FnOnce(),
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        cancellation.checkpoint()?;
        let file = self
            .open_regular_with_hook(path, before_open)
            .map_err(FileSystemReadError::Io)?;
        let declared_len = file.metadata().map_err(FileSystemReadError::Io)?.len();
        Self::read_file_bounded(file, declared_len, limit, cancellation)
    }

    fn read_directory_bounded_with_hook(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &CancellationToken,
        before_open: impl FnOnce(),
    ) -> Result<Vec<OsString>, Outcome<FileSystemReadError>> {
        cancellation.checkpoint()?;
        let mut names = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative =
                Self::rooted_relative_path(root, path).map_err(FileSystemReadError::Io)?;
            before_open();
            for entry in root
                .directory
                .read_dir(&relative)
                .map_err(|error| root.classify_error(&relative, error))
                .map_err(FileSystemReadError::Io)?
            {
                cancellation.checkpoint()?;
                if names.len() as u64 >= limit.get() {
                    return Err(FileSystemReadError::EntryLimitExceeded { limit }.into());
                }
                names.push(
                    entry
                        .map_err(|error| root.classify_error(&relative, error))
                        .map_err(FileSystemReadError::Io)?
                        .file_name(),
                );
            }
            return Ok(names);
        }

        before_open();
        for entry in std::fs::read_dir(path).map_err(FileSystemReadError::Io)? {
            cancellation.checkpoint()?;
            if names.len() as u64 >= limit.get() {
                return Err(FileSystemReadError::EntryLimitExceeded { limit }.into());
            }
            names.push(entry.map_err(FileSystemReadError::Io)?.file_name());
        }
        Ok(names)
    }
}

const fn classify_entry_kind(
    is_file: bool,
    is_directory: bool,
    is_symlink: bool,
) -> FileSystemEntryKind {
    if is_file {
        FileSystemEntryKind::File
    } else if is_directory {
        FileSystemEntryKind::Directory
    } else if is_symlink {
        FileSystemEntryKind::Symlink
    } else {
        FileSystemEntryKind::Other
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RootCapability {
    /// Tell a sandbox escape apart from an ordinary permission failure.
    ///
    /// `cap-std` reports a path that resolves outside the held root (for
    /// example through an escaping symlink) as
    /// [`io::ErrorKind::PermissionDenied`], the same kind as a real access
    /// failure, and carries no typed marker. The ambient resolution below only
    /// classifies that refusal; nothing is opened or read through it. An
    /// escape becomes an [`OutsideRootError`], and every other failure keeps
    /// its own kind, so a refused path is never mistaken for a missing one.
    fn classify_error(&self, relative: &Path, error: io::Error) -> io::Error {
        if error.kind() != io::ErrorKind::PermissionDenied {
            return error;
        }
        let requested = self.canonical_path.as_path().join(relative);
        match requested.canonicalize() {
            Ok(resolved) if !resolved.starts_with(self.canonical_path.as_path()) => {
                RealFileSystem::outside_root_error(&requested, self.canonical_path.as_path())
            }
            _ => error,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cap_relative_path(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    }
}

impl FileSystemReader for RealFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        self.read_bytes_bounded_with_hook(path, limit, cancellation, || {})
    }

    fn hash_file_sha256_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<BoundedFileHash, Outcome<FileSystemReadError>> {
        cancellation.checkpoint()?;
        if self.entry_kind(path).map_err(FileSystemReadError::Io)? != FileSystemEntryKind::File {
            return Err(FileSystemReadError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot hash non-regular file `{}`", path.display()),
            ))
            .into());
        }
        let file = self
            .open_regular_with_hook(path, || {})
            .map_err(FileSystemReadError::Io)?;
        let metadata = file.metadata().map_err(FileSystemReadError::Io)?;
        Self::hash_file_bounded(file, metadata.len(), limit, cancellation)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            return Self::rooted_canonical_relative(root, &relative)
                .map(|relative| root.canonical_path.as_path().join(relative));
        }
        path.canonicalize()
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            let file_type = root
                .directory
                .symlink_metadata(&relative)
                .map_err(|error| root.classify_error(&relative, error))?
                .file_type();
            return Ok(classify_entry_kind(
                file_type.is_file(),
                file_type.is_dir(),
                file_type.is_symlink(),
            ));
        }

        let file_type = std::fs::symlink_metadata(path)?.file_type();
        Ok(classify_entry_kind(
            file_type.is_file(),
            file_type.is_dir(),
            file_type.is_symlink(),
        ))
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OsString>, Outcome<FileSystemReadError>> {
        self.read_directory_bounded_with_hook(path, limit, cancellation, || {})
    }

    fn is_file(&self, path: &Path) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            return Self::rooted_relative_path(root, path)
                .is_ok_and(|relative| root.directory.is_file(relative));
        }
        path.is_file()
    }

    fn exists(&self, path: &Path) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            return Self::rooted_relative_path(root, path)
                .is_ok_and(|relative| root.directory.try_exists(relative).unwrap_or(false));
        }
        path.exists()
    }
}

#[cfg(test)]
mod tests {
    fn io_kind(outcome: Outcome<FileSystemReadError>) -> Option<io::ErrorKind> {
        match outcome {
            Outcome::Failed(error) => error.io_kind(),
            Outcome::Cancelled => None,
        }
    }

    use super::*;
    use std::fs;

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    fn assert_outside_root(outcome: &Outcome<FileSystemReadError>) {
        assert!(
            matches!(outcome, Outcome::Failed(error) if error.outside_root().is_some()),
            "{outcome:?}"
        );
    }

    #[test]
    fn unrooted_reads_any_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.gcl");
        fs::write(&file, "param x: Dimensionless = 1.0;").unwrap();

        let fs_reader = RealFileSystem::default();
        let content = fs_reader
            .read_to_string_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded())
            .unwrap();
        assert!(content.contains("param x"));
    }

    #[test]
    fn rooted_allows_paths_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("a.gcl");
        fs::write(&file, "hello").unwrap();

        let fs_reader = RealFileSystem::rooted(&root).unwrap();
        assert_eq!(
            fs_reader
                .read_to_string_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded())
                .unwrap(),
            "hello"
        );
        assert!(fs_reader.is_file(&file));
        assert!(fs_reader.exists(&file));
        assert_eq!(fs_reader.canonicalize(&file).unwrap(), file);
    }

    #[cfg(unix)]
    #[test]
    fn special_file_reads_have_a_subprocess_watchdog() {
        use std::process::Command;
        const CHILD: &str = "GRAPHCAL_SPECIAL_FILE_READ_CHILD";
        if std::env::var_os(CHILD).is_some() {
            for rooted in [false, true] {
                for swap in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let root = dir.path().canonicalize().unwrap();
                    let path = root.join("graphcal.toml");
                    let fifo = || {
                        assert!(
                            Command::new("mkfifo")
                                .arg(&path)
                                .status()
                                .unwrap()
                                .success()
                        );
                    };
                    if swap {
                        fs::write(&path, "regular").unwrap();
                    } else {
                        fifo();
                    }
                    let reader = if rooted {
                        RealFileSystem::rooted(&root).unwrap()
                    } else {
                        RealFileSystem::default()
                    };
                    assert!(
                        reader
                            .read_bytes_bounded_with_hook(
                                &path,
                                TEST_LIMIT,
                                &CancellationToken::unbounded(),
                                || {
                                    if swap {
                                        fs::remove_file(&path).unwrap();
                                        fifo();
                                    }
                                }
                            )
                            .is_err()
                    );
                    assert!(
                        reader
                            .hash_file_sha256_bounded(
                                &path,
                                TEST_LIMIT,
                                &CancellationToken::unbounded()
                            )
                            .is_err()
                    );
                    let socket = root.join("socket");
                    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
                    assert!(
                        reader
                            .read_bytes_bounded(
                                &socket,
                                TEST_LIMIT,
                                &CancellationToken::unbounded()
                            )
                            .is_err()
                    );
                }
            }
            assert!(
                RealFileSystem::default()
                    .read_bytes_bounded(
                        Path::new("/dev/null"),
                        TEST_LIMIT,
                        &CancellationToken::unbounded()
                    )
                    .is_err()
            );
            return;
        }
        assert_child_test_finishes(
            "real_fs::tests::special_file_reads_have_a_subprocess_watchdog",
            CHILD,
        );
    }

    /// Re-run one test in a child process with `child_env` set and fail if it
    /// does not finish successfully before a watchdog deadline, so a blocking
    /// open fails the test instead of hanging the suite.
    #[cfg(unix)]
    fn assert_child_test_finishes(test: &str, child_env: &str) {
        use std::process::Command;
        use std::time::{Duration, Instant};
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(child_env, "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success());
                    break;
                }
                None if Instant::now() >= deadline => {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{test} exceeded watchdog");
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn special_file_canonicalization_does_not_open_the_entry() {
        use std::process::Command;
        const CHILD: &str = "GRAPHCAL_SPECIAL_FILE_CANONICALIZE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            // Opening a FIFO for reading blocks until a writer appears, and
            // `cap-std` canonicalizes by opening every component.
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let fifo = root.join("model.gcl");
            assert!(
                Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
            for reader in [
                RealFileSystem::rooted(&root).unwrap(),
                RealFileSystem::default(),
            ] {
                assert_eq!(reader.canonicalize(&fifo).unwrap(), fifo);
            }
            return;
        }
        assert_child_test_finishes(
            "real_fs::tests::special_file_canonicalization_does_not_open_the_entry",
            CHILD,
        );
    }

    #[test]
    fn bounded_read_rejects_sparse_file_before_allocating_declared_length() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.bin");
        let handle = File::create(&file).unwrap();
        handle.set_len(1025).unwrap();

        let error = RealFileSystem::default()
            .read_bytes_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded())
            .unwrap_err();
        assert!(matches!(
            error,
            Outcome::Failed(FileSystemReadError::ByteLimitExceeded { .. })
        ));
    }

    #[test]
    fn bounded_hash_streams_regular_file_and_matches_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plugin.wasm");
        fs::write(&file, b"streamed plugin bytes").unwrap();

        let hash = RealFileSystem::rooted(dir.path())
            .unwrap()
            .hash_file_sha256_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded())
            .unwrap();

        assert_eq!(hash.bytes(), 21);
        let expected: [u8; 32] = Sha256::digest(b"streamed plugin bytes").into();
        assert_eq!(hash.sha256(), expected);
    }

    #[test]
    fn bounded_hash_rejects_sparse_file_from_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.wasm");
        let handle = File::create(&file).unwrap();
        handle.set_len(TEST_LIMIT.get() + 1).unwrap();

        assert!(matches!(
            RealFileSystem::rooted(dir.path())
                .unwrap()
                .hash_file_sha256_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded()),
            Err(Outcome::Failed(
                FileSystemReadError::ByteLimitExceeded { .. }
            ))
        ));
    }

    #[test]
    fn bounded_hash_observes_cancellation_before_streaming_completes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plugin.wasm");
        fs::write(&file, vec![7_u8; 32 * 1024]).unwrap();
        let cancellation = CancellationToken::cancel_after_successful_checkpoints(2);

        assert!(matches!(
            RealFileSystem::rooted(dir.path())
                .unwrap()
                .hash_file_sha256_bounded(&file, ByteLimit::new(64 * 1024), &cancellation),
            Err(Outcome::Cancelled)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_hash_rejects_symlinks_and_special_files_before_open() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.wasm");
        let linked = dir.path().join("linked.wasm");
        let socket = dir.path().join("socket.wasm");
        fs::write(&target, b"secret").unwrap();
        symlink(&target, &linked).unwrap();
        let _listener = UnixListener::bind(&socket).unwrap();
        let reader = RealFileSystem::rooted(dir.path()).unwrap();

        for rejected in [linked, socket] {
            assert_eq!(
                io_kind(
                    reader
                        .hash_file_sha256_bounded(
                            &rejected,
                            TEST_LIMIT,
                            &CancellationToken::unbounded()
                        )
                        .unwrap_err()
                ),
                Some(io::ErrorKind::InvalidInput)
            );
        }
    }

    #[test]
    fn rooted_rejects_paths_outside_root() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let secret = external.join("secret.gcl");
        fs::write(&secret, "secret content").unwrap();

        let fs_reader = RealFileSystem::rooted(&project).unwrap();
        let error = fs_reader
            .read_to_string_bounded(&secret, TEST_LIMIT, &CancellationToken::unbounded())
            .unwrap_err();
        assert_outside_root(&error);
        assert!(OutsideRootError::find(&fs_reader.canonicalize(&secret).unwrap_err()).is_some());
        assert!(!fs_reader.is_file(&secret));
        assert!(!fs_reader.exists(&secret));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let target = external.join("escaped.gcl");
        fs::write(&target, "escaped").unwrap();

        let link = project.join("link.gcl");
        symlink(&target, &link).unwrap();

        let fs_reader = RealFileSystem::rooted(&project).unwrap();
        let error = fs_reader
            .read_to_string_bounded(&link, TEST_LIMIT, &CancellationToken::unbounded())
            .unwrap_err();
        assert_outside_root(&error);
        // PRJ-9: canonicalizing an escaping symlink is a sandbox refusal, not
        // a missing entry, so callers cannot fall back as if it were absent.
        let error = fs_reader.canonicalize(&link).unwrap_err();
        assert!(OutsideRootError::find(&error).is_some(), "{error:?}");
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
        // A missing in-root entry is still reported as not found.
        assert_eq!(
            fs_reader
                .canonicalize(&project.join("missing.gcl"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            fs_reader.entry_kind(&link).unwrap(),
            FileSystemEntryKind::Symlink
        );
        assert!(!fs_reader.exists(&link));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_read_rejects_final_file_swap_between_mapping_and_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("secret.gcl");
        fs::create_dir_all(&project).unwrap();
        fs::write(&external, "outside secret").unwrap();
        let victim = project.join("victim.gcl");
        fs::write(&victim, "inside").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();

        let error = reader
            .read_bytes_bounded_with_hook(
                &victim,
                TEST_LIMIT,
                &CancellationToken::unbounded(),
                || {
                    fs::remove_file(&victim).unwrap();
                    symlink(&external, &victim).unwrap();
                },
            )
            .unwrap_err();

        assert!(matches!(error, Outcome::Failed(FileSystemReadError::Io(_))));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_read_rejects_parent_swap_between_mapping_and_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        let directory = project.join("directory");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(directory.join("model.gcl"), "inside").unwrap();
        fs::write(external.join("model.gcl"), "outside secret").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();
        let requested = directory.join("model.gcl");
        let parked = project.join("parked");

        let error = reader
            .read_bytes_bounded_with_hook(
                &requested,
                TEST_LIMIT,
                &CancellationToken::unbounded(),
                || {
                    fs::rename(&directory, &parked).unwrap();
                    symlink(&external, &directory).unwrap();
                },
            )
            .unwrap_err();

        assert!(matches!(error, Outcome::Failed(FileSystemReadError::Io(_))));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_directory_listing_rejects_parent_swap_before_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        let directory = project.join("listing");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(directory.join("inside.gcl"), "inside").unwrap();
        fs::write(external.join("secret.gcl"), "outside secret").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();
        let parked = project.join("parked");

        let error = reader
            .read_directory_bounded_with_hook(
                &directory,
                EntryLimit::new(10),
                &CancellationToken::unbounded(),
                || {
                    fs::rename(&directory, &parked).unwrap();
                    symlink(&external, &directory).unwrap();
                },
            )
            .unwrap_err();

        assert!(matches!(error, Outcome::Failed(FileSystemReadError::Io(_))));
    }

    #[test]
    fn unrooted_path_outside_any_root_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("loose.gcl");
        fs::write(&file, "loose").unwrap();

        let fs_reader = RealFileSystem::default();
        assert!(fs_reader.exists(&file));
        assert!(fs_reader.is_file(&file));
        assert_eq!(
            fs_reader
                .read_to_string_bounded(&file, TEST_LIMIT, &CancellationToken::unbounded())
                .unwrap(),
            "loose"
        );
    }
}
