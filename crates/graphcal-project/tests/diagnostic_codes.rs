//! Workspace-wide guard for the stable `graphcal::` diagnostic code namespace.
//!
//! Diagnostic codes are assigned in several crates and in two syntaxes:
//! `DiagnosticKind::code()` match arms (`Self::X => "graphcal::P001"`) and
//! miette attributes (`code(graphcal::M033)`). This test discovers every such
//! assignment in non-test sources itself, so a new error enum is covered
//! without registering it anywhere.
#![allow(
    clippy::expect_used,
    reason = "an unreadable workspace source tree should fail the guard immediately"
)]

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// One place in the sources that assigns a stable code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CodeSite {
    file: PathBuf,
    line: usize,
}

impl fmt::Display for CodeSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

fn workspace_crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives under `crates/`")
        .to_path_buf()
}

/// Every non-test `.rs` file under each crate's `src/`.
fn non_test_sources(crates_dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("readable source dir") {
            let path = entry.expect("readable dir entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let is_test_module =
                name == "tests" || name == "tests.rs" || name.ends_with("_tests.rs");
            let is_rust = path.extension().is_some_and(|ext| ext == "rs");
            match (path.is_dir(), is_rust && !is_test_module) {
                (true, _) if !is_test_module => walk(&path, out),
                (false, true) => out.push(path),
                _ => {}
            }
        }
    }
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(crates_dir).expect("readable crates dir") {
        let src = entry.expect("readable dir entry").path().join("src");
        if src.is_dir() {
            walk(&src, &mut sources);
        }
    }
    sources.sort();
    sources
}

/// The code (`P001`) assigned at byte `start`, if the text there is
/// `graphcal::<letter><3 digits>` closed by `"` or `)`.
fn code_at(text: &str, start: usize) -> Option<&str> {
    let code = text.get(start..start + 4)?;
    let mut chars = code.chars();
    let well_formed = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_digit())
        && matches!(text.as_bytes().get(start + 4), Some(b'"' | b')'));
    well_formed.then_some(code)
}

/// Code assignments in `text`, ignoring everything from the first inline
/// `#[cfg(test)]` module on (which also skips frozen legacy copies kept
/// for regression tests).
fn code_assignments(text: &str) -> Vec<(usize, &str)> {
    const MARKERS: [&str; 3] = [
        "=> \"graphcal::",               // `DiagnosticKind::code()` arm
        "code(graphcal::",               // miette attribute
        ": &'static str = \"graphcal::", // associated `CODE` constant
    ];
    let text = text
        .find("#[cfg(test)]\nmod ")
        .map_or(text, |end| &text[..end]);
    let mut found: Vec<(usize, &str)> = MARKERS
        .iter()
        .flat_map(|marker| {
            text.match_indices(marker).filter_map(|(at, _)| {
                let line = text[..at].matches('\n').count() + 1;
                code_at(text, at + marker.len()).map(|code| (line, code))
            })
        })
        .collect();
    found.sort_unstable();
    found
}

fn workspace_code_sites() -> BTreeMap<String, Vec<CodeSite>> {
    let crates_dir = workspace_crates_dir();
    let mut sites: BTreeMap<String, Vec<CodeSite>> = BTreeMap::new();
    for path in non_test_sources(&crates_dir) {
        let text = std::fs::read_to_string(&path).expect("readable source file");
        let file = path
            .strip_prefix(&crates_dir)
            .expect("source under crates dir")
            .to_path_buf();
        for (line, code) in code_assignments(&text) {
            sites.entry(code.to_owned()).or_default().push(CodeSite {
                file: file.clone(),
                line,
            });
        }
    }
    sites
}

#[test]
fn every_diagnostic_code_is_assigned_at_exactly_one_site() {
    let sites = workspace_code_sites();
    assert!(sites.len() > 200, "incomplete catalog: {sites:?}");
    for family in ["graphcal-compiler", "graphcal-project"] {
        assert!(
            sites
                .values()
                .flatten()
                .any(|site| site.file.starts_with(family)),
            "no codes discovered in `{family}`"
        );
    }

    let shared: Vec<String> = sites
        .iter()
        .filter(|(_, sites)| sites.len() > 1)
        .map(|(code, sites)| {
            let sites: Vec<String> = sites.iter().map(ToString::to_string).collect();
            format!("{code}: {}", sites.join(", "))
        })
        .collect();
    assert!(
        shared.is_empty(),
        "diagnostic codes assigned at more than one site:\n{}",
        shared.join("\n")
    );
}

#[test]
fn each_source_file_assigns_codes_from_a_single_prefix() {
    let mut prefixes_by_file: BTreeMap<PathBuf, BTreeMap<char, CodeSite>> = BTreeMap::new();
    for (code, sites) in workspace_code_sites() {
        let prefix = code.chars().next().expect("non-empty code");
        for site in sites {
            prefixes_by_file
                .entry(site.file.clone())
                .or_default()
                .entry(prefix)
                .or_insert(site);
        }
    }
    for (file, prefixes) in prefixes_by_file {
        assert!(
            prefixes.len() == 1,
            "`{}` mixes code prefixes: {prefixes:?}",
            file.display()
        );
    }
}

#[test]
fn code_assignments_recognize_every_syntax_and_skip_test_modules() {
    let source = "\
            Self::A { .. } | Self::B => \"graphcal::P001\",
    #[diagnostic(code(graphcal::M033), help(\"see graphcal::M034\"))]
    #[diagnostic(code(graphcal::cli::O001))]
    pub const CODE: &'static str = \"graphcal::X001\";
#[cfg(test)]
mod tests {
    #[diagnostic(code(graphcal::P002))]
}
";
    assert_eq!(
        code_assignments(source),
        vec![(1, "P001"), (2, "M033"), (4, "X001")]
    );
}
