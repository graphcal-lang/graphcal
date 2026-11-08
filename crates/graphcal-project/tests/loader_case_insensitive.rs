//! PRJ-10 (#2255): on a case-insensitive filesystem one file must not load
//! under two identities. A reader that folds ASCII case and keeps the
//! requested spelling (as `cap-std` does on macOS) makes the check
//! reproducible on every CI host.
#![cfg(test)]

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::outcome::Outcome;
use graphcal_io::{
    ByteLimit, EntryLimit, FileSystemEntryKind, FileSystemReadError, FileSystemReader,
    InMemoryFileSystem, VirtualAbsolutePath,
};
use graphcal_project::compile_error::CompileError;
use graphcal_project::load_error::LoadError;
use graphcal_project::loader::load_project;

struct CaseInsensitive(InMemoryFileSystem);

impl CaseInsensitive {
    fn new(files: &[(&str, &str)]) -> Self {
        let mut fs = InMemoryFileSystem::new();
        for (path, contents) in files {
            fs.add_file(
                VirtualAbsolutePath::new(*path).unwrap(),
                (*contents).to_owned(),
            )
            .unwrap();
        }
        Self(fs)
    }

    /// The stored spelling of `path`, matching each component ignoring case.
    fn stored(&self, path: &Path) -> io::Result<PathBuf> {
        let mut stored = PathBuf::new();
        for component in path.components() {
            let Component::Normal(name) = component else {
                stored.push(component.as_os_str());
                continue;
            };
            let entry = self
                .0
                .read_directory_bounded(&stored, EntryLimit::new(1_000), &unbounded())
                .ok()
                .and_then(|names| {
                    names.into_iter().find(|entry| {
                        entry
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&name.to_string_lossy())
                    })
                })
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            stored.push(entry);
        }
        Ok(stored)
    }
}

fn unbounded() -> CancellationToken {
    CancellationToken::unbounded()
}

impl FileSystemReader for CaseInsensitive {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        let stored = self.stored(path).map_err(FileSystemReadError::Io)?;
        self.0.read_bytes_bounded(&stored, limit, cancellation)
    }

    /// Like `cap-std` on macOS: the requested spelling of an existing path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.stored(path)?;
        self.0.canonicalize(path).or_else(|_| {
            VirtualAbsolutePath::new(path)
                .map(VirtualAbsolutePath::into_path_buf)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        })
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        self.0.entry_kind(&self.stored(path)?)
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OsString>, Outcome<FileSystemReadError>> {
        let stored = self.stored(path).map_err(FileSystemReadError::Io)?;
        self.0.read_directory_bounded(&stored, limit, cancellation)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.stored(path)
            .is_ok_and(|stored| self.0.is_file(&stored))
    }

    fn exists(&self, path: &Path) -> bool {
        self.stored(path).is_ok()
    }
}

const MANIFEST: (&str, &str) = ("/p/graphcal.toml", "[package]\nname = \"pkg\"\n");
const LIB: (&str, &str) = (
    "/p/src/pkg/lib.gcl",
    "pub type Thing { Thing(v: Dimensionless) }\npub const node k: Dimensionless = 1.0;",
);

fn assert_spelling_mismatch(error: &CompileError, requested: &str, stored: &str) {
    assert!(
        matches!(
            error,
            CompileError::Load(LoadError::ModuleFileSpellingMismatch { path, on_disk })
                if path == Path::new(requested) && on_disk.as_deref() == Some(stored)
        ),
        "{error:?}"
    );
}

#[test]
fn import_in_another_case_is_a_spelling_mismatch() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        LIB,
        (
            "/p/src/pkg/main.gcl",
            "import pkg.lib::{type Thing, Thing};\nimport pkg.Lib::{type Thing as Thing2};\nnode a: Thing = Thing(v: 1.0);\nnode b: Thing2 = @a;",
        ),
    ]);
    let error = load_project(Path::new("/p/src/pkg/main.gcl"), None, &fs)
        .expect_err("`pkg.Lib` must not load `lib.gcl` a second time");
    assert_spelling_mismatch(&error, "/p/src/pkg/Lib.gcl", "lib.gcl");
}

#[test]
fn directory_in_another_case_is_a_spelling_mismatch() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        (
            "/p/src/pkg/nested/lib.gcl",
            "pub const node k: Dimensionless = 1.0;",
        ),
        (
            "/p/src/pkg/main.gcl",
            "import pkg.NESTED.lib::{k};\nnode a: Dimensionless = @k;",
        ),
    ]);
    let error = load_project(Path::new("/p/src/pkg/main.gcl"), None, &fs)
        .expect_err("`pkg.NESTED` must not name `nested/`");
    assert_spelling_mismatch(&error, "/p/src/pkg/NESTED", "nested");
}

#[test]
fn entry_in_another_case_is_a_spelling_mismatch() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        LIB,
        (
            "/p/src/pkg/main.gcl",
            "import pkg.lib::{k};\nnode a: Dimensionless = @k;",
        ),
    ]);
    let error = load_project(Path::new("/p/src/pkg/Main.gcl"), None, &fs)
        .expect_err("the entry must be spelled as on disk");
    assert_spelling_mismatch(&error, "/p/src/pkg/Main.gcl", "main.gcl");
}

#[test]
fn exact_spellings_still_load() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        LIB,
        (
            "/p/src/pkg/main.gcl",
            "import pkg.lib::{k};\nnode a: Dimensionless = @k;",
        ),
    ]);
    load_project(Path::new("/p/src/pkg/main.gcl"), None, &fs).unwrap();
}

/// A wrong-case candidate is skipped like a missing file, as on a
/// case-sensitive filesystem: `pkg.lib.Inner` names `dag Inner` in
/// `lib.gcl`, not the sibling file `lib/inner.gcl`.
#[test]
fn wrong_case_candidate_falls_back_as_on_a_case_sensitive_filesystem() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        (
            "/p/src/pkg/lib.gcl",
            "dag Inner {\n    pub const node k: Dimensionless = 1.0;\n}",
        ),
        (
            "/p/src/pkg/lib/inner.gcl",
            "pub const node k: Dimensionless = 2.0;",
        ),
        (
            "/p/src/pkg/main.gcl",
            "import pkg.lib.Inner::{k};\nnode a: Dimensionless = @k;",
        ),
    ]);
    let project = load_project(Path::new("/p/src/pkg/main.gcl"), None, &fs).unwrap();
    assert!(
        project
            .files()
            .iter()
            .all(|file| !file.path().ends_with("lib/inner.gcl")),
        "the wrong-case sibling file must not be loaded"
    );
}

/// The namespace directory spelled in another case is the package entry
/// spelled wrongly, not a standalone script.
#[test]
fn namespace_directory_in_another_case_is_a_spelling_mismatch() {
    let fs = CaseInsensitive::new(&[
        MANIFEST,
        LIB,
        (
            "/p/src/pkg/main.gcl",
            "import pkg.lib::{k};\nnode a: Dimensionless = @k;",
        ),
    ]);
    let error = load_project(Path::new("/p/src/Pkg/main.gcl"), None, &fs)
        .expect_err("the namespace directory must be spelled as on disk");
    assert_spelling_mismatch(&error, "/p/src/Pkg", "pkg");
}
