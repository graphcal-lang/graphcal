//! Property-based tests: declaration order must not affect evaluation results.
//!
//! Graphcal's reactive evaluation model builds a dependency DAG and
//! topologically sorts it, so the source order of top-level declarations
//! should never influence the computed values.
//!
//! These tests randomly shuffle declarations and verify that the evaluation
//! results remain identical.
//!
//! Note the boundary of the claim: *top-level declaration order* is
//! semantically inert, but order *within* a declaration is not. Label order
//! inside an `index` declaration defines the axis's index order and is
//! load-bearing for order-sensitive consumers such as `scan`
//! (`docs/language/indexes.md`), and assertions report in declaration order
//! (`docs/language/assertions.md`). The shuffler only permutes whole
//! declarations, so it never disturbs either.
//!
//! See: <https://github.com/graphcal-lang/graphcal/issues/247>
#![cfg(test)]

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use graphcal_compiler::syntax::parser::Parser;
use graphcal_eval::eval::{
    EvalResult, compile_and_eval, compile_and_eval_from_project, compile_and_eval_named,
};
use graphcal_eval::loader::load_project;
use graphcal_io::RealFileSystem;
use proptest::prelude::*;
use rand::SeedableRng;
use rand::seq::SliceRandom;

// ============================================================================
// Shuffling
// ============================================================================

/// Split `source` into the text slices of its top-level declarations.
///
/// Each `Declaration` carries a `span` (byte offset + length) that covers the
/// full text from the first attribute (or the leading keyword, when there are
/// none) to the closing semicolon/brace. Text *between* declarations — blank
/// lines, standalone comments, and `///` doc blocks, none of which carry
/// semantics — is dropped, since reassembly only concatenates these slices.
fn declaration_slices(source: &str) -> Vec<&str> {
    let mut parser = Parser::new(source);
    let file = parser.parse_file().expect("fixture must parse");

    file.declarations
        .iter()
        .map(|decl| {
            let start = decl.span.offset();
            let mut end = start + decl.span.len();
            // Some declaration spans (e.g., `range`) exclude the trailing
            // semicolon. Extend the slice to include it when present.
            if end < source.len() && source.as_bytes()[end] == b';' {
                end += 1;
            }
            &source[start..end]
        })
        .collect()
}

/// Join declaration slices back into a parseable source file.
fn reassemble(slices: &[&str]) -> String {
    slices.join("\n\n")
}

/// How a source file should be permuted before re-evaluation.
#[derive(Debug, Clone, Copy)]
enum Permutation {
    /// Reassemble the declarations in their original order. This exercises the
    /// slice/rejoin machinery on its own, so a failure here is a harness bug
    /// (a dropped semicolon, a wrongly measured span) rather than an
    /// order-dependence bug in the language.
    Identity,
    /// Shuffle the declarations with a seeded RNG.
    Shuffled(u64),
}

impl Permutation {
    fn apply(self, source: &str) -> String {
        let mut slices = declaration_slices(source);
        if let Self::Shuffled(seed) = self {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            slices.shuffle(&mut rng);
        }
        reassemble(&slices)
    }

    fn label(self) -> String {
        match self {
            Self::Identity => "identity reassembly".to_owned(),
            Self::Shuffled(seed) => format!("shuffle seed={seed}"),
        }
    }
}

/// Parse the source, shuffle the top-level declarations using a seeded RNG,
/// and reassemble the source text.
fn shuffle_source(source: &str, seed: u64) -> String {
    Permutation::Shuffled(seed).apply(source)
}

// ============================================================================
// Result comparison
// ============================================================================

/// Compare two `EvalResult`s for semantic equivalence.
///
/// Every collection on `EvalResult` is documented as being *in source order*,
/// so a permuted file legitimately produces the same entries in a different
/// sequence. Each collection is therefore keyed by declaration name and
/// compared entry-by-entry; only the set of names and the value behind each
/// name are load-bearing.
///
/// Returns a human-readable description of every difference found, or `Ok(())`
/// when the two results agree.
fn compare_results(original: &EvalResult, permuted: &EvalResult) -> Result<(), String> {
    let mut diffs = String::new();

    // `all` subsumes consts/params/nodes and additionally carries each
    // declaration's `DeclType`, so a value silently changing category is
    // caught too.
    compare_keyed(
        &mut diffs,
        "declaration",
        original
            .all
            .iter()
            .map(|(name, value, decl_type)| (name.to_string(), format!("{decl_type:?} {value:?}"))),
        permuted
            .all
            .iter()
            .map(|(name, value, decl_type)| (name.to_string(), format!("{decl_type:?} {value:?}"))),
    );

    // Per-bucket counts, so a declaration moving between buckets is reported
    // even if `all` somehow agrees.
    for (bucket, orig_len, perm_len) in [
        ("const", original.consts.len(), permuted.consts.len()),
        ("param", original.params.len(), permuted.params.len()),
        ("node", original.nodes.len(), permuted.nodes.len()),
    ] {
        if orig_len != perm_len {
            let _ = writeln!(diffs, "  {bucket} count: {orig_len} vs {perm_len}");
        }
    }

    // Assertion outcomes. Spans are excluded: they necessarily move when the
    // declarations move, and they carry no evaluation semantics.
    compare_keyed(
        &mut diffs,
        "assertion",
        original
            .assertions
            .iter()
            .map(|(name, result, _span)| (name.to_string(), format!("{result:?}"))),
        permuted
            .assertions
            .iter()
            .map(|(name, result, _span)| (name.to_string(), format!("{result:?}"))),
    );

    // Presentation specs. These have no `PartialEq`, and none of them embed a
    // span, so their `Debug` rendering is a faithful structural comparison.
    // Stringifying here is a test-boundary concern, not a core one.
    compare_keyed(
        &mut diffs,
        "plot",
        original
            .plots
            .iter()
            .map(|plot| (plot.name.to_string(), format!("{plot:?}"))),
        permuted
            .plots
            .iter()
            .map(|plot| (plot.name.to_string(), format!("{plot:?}"))),
    );
    compare_keyed(
        &mut diffs,
        "figure",
        original
            .figures
            .iter()
            .map(|figure| (figure.name.to_string(), format!("{figure:?}"))),
        permuted
            .figures
            .iter()
            .map(|figure| (figure.name.to_string(), format!("{figure:?}"))),
    );
    compare_keyed(
        &mut diffs,
        "layer",
        original
            .layers
            .iter()
            .map(|layer| (layer.name.to_string(), format!("{layer:?}"))),
        permuted
            .layers
            .iter()
            .map(|layer| (layer.name.to_string(), format!("{layer:?}"))),
    );
    compare_keyed(
        &mut diffs,
        "plot error",
        original
            .plot_errors
            .iter()
            .map(|error| (error.name.to_string(), format!("{error:?}"))),
        permuted
            .plot_errors
            .iter()
            .map(|error| (error.name.to_string(), format!("{error:?}"))),
    );

    if diffs.is_empty() { Ok(()) } else { Err(diffs) }
}

/// Rewrite anonymous include scopes to a position-free spelling.
///
/// A selective include introduces no source-visible module alias, so lowering
/// gives it an opaque owner-local identity (`IncludeInstanceId`) rendered as
/// `<include@{byte offset}>`. The offset is explicitly documented as an
/// implementation detail that is "never parsed to recover the offset", but it
/// does surface in `EvalResult`'s debug view, and it necessarily moves when a
/// declaration moves. Comparing those raw spellings across two different
/// source texts would compare opaque identities, not semantics, so both the
/// name and the rendered value are normalized before comparison.
fn normalize_anonymous_includes(text: &str) -> String {
    const PREFIX: &str = "<include@";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(PREFIX) {
        match rest[start..].find('>') {
            Some(offset) => {
                out.push_str(&rest[..start]);
                out.push_str("<include>");
                rest = &rest[start + offset + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Compare two name-keyed collections, appending one line per difference.
///
/// Names are grouped rather than assumed unique: normalizing anonymous include
/// scopes can make two sibling instances share a key, and two interchangeable
/// instances swapping positions is not a semantic change. A differing *value*
/// under a key is still reported.
fn compare_keyed(
    diffs: &mut String,
    kind: &str,
    original: impl Iterator<Item = (String, String)>,
    permuted: impl Iterator<Item = (String, String)>,
) {
    let original = group_by_name(original);
    let permuted = group_by_name(permuted);

    for (name, original_values) in &original {
        match permuted.get(name) {
            None => {
                let _ = writeln!(diffs, "  {kind} `{name}`: missing after permutation");
            }
            Some(permuted_values) if permuted_values != original_values => {
                match (original_values.as_slice(), permuted_values.as_slice()) {
                    ([one], [other]) => {
                        let _ = writeln!(diffs, "  {kind} `{name}`: {one} vs {other}");
                    }
                    _ => {
                        let _ = writeln!(
                            diffs,
                            "  {kind} `{name}`: {original_values:?} vs {permuted_values:?}"
                        );
                    }
                }
            }
            Some(_) => {}
        }
    }
    for name in permuted.keys() {
        if !original.contains_key(name) {
            let _ = writeln!(diffs, "  {kind} `{name}`: unexpected after permutation");
        }
    }
}

/// Group `(name, rendered)` pairs by normalized name, sorting each group so
/// that two permutations of the same entries compare equal.
fn group_by_name(entries: impl Iterator<Item = (String, String)>) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, rendered) in entries {
        grouped
            .entry(normalize_anonymous_includes(&name))
            .or_default()
            .push(normalize_anonymous_includes(&rendered));
    }
    for values in grouped.values_mut() {
        values.sort();
    }
    grouped
}

/// Panicking wrapper around [`compare_results`] for the single-fixture
/// property tests.
fn assert_results_equal(original: &EvalResult, shuffled: &EvalResult) {
    if let Err(diffs) = compare_results(original, shuffled) {
        panic!("shuffled source produced different results:\n{diffs}");
    }
}

// ============================================================================
// Fixture discovery
// ============================================================================

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// How a fixture's sources are laid out on disk, which decides how it is
/// loaded and how the permuted copy is built.
#[derive(Debug, Clone)]
enum FixtureLayout {
    /// A self-contained `.gcl` file with no project manifest above it. It is
    /// permuted purely in memory.
    SingleFile,
    /// A `graphcal.toml`-rooted project. The whole project directory is copied
    /// to a scratch directory and *every* `.gcl` file in it is permuted, so
    /// cross-file import and include order is exercised too.
    Project { root: PathBuf },
}

/// A fixture entry point under `tests/fixtures/valid/`.
#[derive(Debug, Clone)]
struct Fixture {
    /// Path relative to `tests/fixtures/`, used as a stable identifier in
    /// failure messages and in [`SKIPPED`].
    id: String,
    /// Absolute path to the entry-point `.gcl` file.
    entry: PathBuf,
    layout: FixtureLayout,
}

/// Collect entry points the same way the CLI fixture sweep does
/// (`crates/graphcal-cli/tests/cli.rs`): within a directory, a `main.gcl`
/// shadows its siblings; otherwise every `.gcl` file is an entry point.
fn collect_entry_points(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut local_gcls: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut has_main = false;
    for entry in std::fs::read_dir(dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "gcl") {
            if path.file_name().is_some_and(|name| name == "main.gcl") {
                has_main = true;
            }
            local_gcls.push(path);
        } else {
            // Skip non-`.gcl` files such as `graphcal.toml` and `input_*.json`.
        }
    }
    if has_main {
        out.extend(
            local_gcls
                .into_iter()
                .filter(|path| path.file_name().is_some_and(|name| name == "main.gcl")),
        );
    } else {
        local_gcls.sort();
        out.extend(local_gcls);
    }
    subdirs.sort();
    for subdir in subdirs {
        collect_entry_points(&subdir, out);
    }
}

/// The nearest ancestor directory holding a `graphcal.toml`, mirroring the
/// loader's own project-root discovery.
fn project_root_of(entry: &Path, stop_at: &Path) -> Option<PathBuf> {
    let mut dir = entry.parent()?;
    loop {
        if dir.join("graphcal.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        if dir == stop_at {
            return None;
        }
        dir = dir.parent()?;
    }
}

fn valid_fixtures() -> Vec<Fixture> {
    let root = fixtures_root();
    let valid = root.join("valid");
    let mut entries = Vec::new();
    collect_entry_points(&valid, &mut entries);
    entries.sort();

    entries
        .into_iter()
        .map(|entry| {
            let id = entry
                .strip_prefix(&root)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            let layout = project_root_of(&entry, &valid)
                .map_or(FixtureLayout::SingleFile, |root| FixtureLayout::Project {
                    root,
                });
            Fixture { id, entry, layout }
        })
        .collect()
}

// ============================================================================
// Fixture evaluation
// ============================================================================

/// Recursively copy `src` into `dst`.
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create scratch dir");
    for entry in std::fs::read_dir(src).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

/// Every `.gcl` file under `dir`, recursively.
fn gcl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read project dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            gcl_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "gcl") {
            out.push(path);
        } else {
            // Manifests and data files are copied verbatim, never permuted.
        }
    }
}

/// A fixture prepared for repeated permutation.
enum PreparedFixture {
    /// Source text, permuted in memory and evaluated under a stable name so
    /// that diagnostics paths never differ between runs.
    Single { name: String, source: String },
    /// A scratch copy of the project. Each permutation rewrites the `.gcl`
    /// files in place, so every run loads from identical paths and only the
    /// declaration order varies.
    Project {
        _scratch: tempfile::TempDir,
        entry: PathBuf,
        sources: Vec<(PathBuf, String)>,
    },
}

impl PreparedFixture {
    fn prepare(fixture: &Fixture) -> Self {
        match &fixture.layout {
            FixtureLayout::SingleFile => Self::Single {
                name: fixture.entry.to_string_lossy().into_owned(),
                source: std::fs::read_to_string(&fixture.entry).expect("read fixture"),
            },
            FixtureLayout::Project { root } => {
                let scratch = tempfile::tempdir().expect("create scratch dir");
                let dest = scratch.path().join("project");
                copy_dir(root, &dest);

                let entry = dest.join(
                    fixture
                        .entry
                        .strip_prefix(root)
                        .expect("entry point lives under its project root"),
                );
                let mut paths = Vec::new();
                gcl_files(&dest, &mut paths);
                paths.sort();
                let sources = paths
                    .into_iter()
                    .map(|path| {
                        let source = std::fs::read_to_string(&path).expect("read project file");
                        (path, source)
                    })
                    .collect();

                Self::Project {
                    _scratch: scratch,
                    entry,
                    sources,
                }
            }
        }
    }

    /// The source text of every `.gcl` file this fixture is built from.
    fn sources(&self) -> Vec<&str> {
        match self {
            Self::Single { source, .. } => vec![source.as_str()],
            Self::Project { sources, .. } => {
                sources.iter().map(|(_, source)| source.as_str()).collect()
            }
        }
    }

    /// Apply `permutation` to every source file, returning the new texts in
    /// the same order as [`Self::sources`].
    fn permute(&self, permutation: Permutation) -> Vec<String> {
        self.sources()
            .into_iter()
            .map(|source| permutation.apply(source))
            .collect()
    }

    /// Evaluate the fixture from its unmodified sources.
    fn eval_original(&self) -> Result<EvalResult, String> {
        let texts: Vec<String> = self
            .sources()
            .into_iter()
            .map(std::borrow::ToOwned::to_owned)
            .collect();
        self.eval(&texts)
    }

    /// Evaluate the fixture from `texts`, which must correspond one-to-one
    /// with [`Self::sources`].
    fn eval(&self, texts: &[String]) -> Result<EvalResult, String> {
        match self {
            Self::Single { name, .. } => {
                let [text] = texts else {
                    panic!("a single-file fixture has exactly one source");
                };
                compile_and_eval_named(text, name).map_err(|error| format!("{error:?}"))
            }
            Self::Project { entry, sources, .. } => {
                assert_eq!(
                    sources.len(),
                    texts.len(),
                    "one text per project source file"
                );
                for ((path, _), text) in sources.iter().zip(texts) {
                    std::fs::write(path, text).expect("write permuted project file");
                }
                let fs = RealFileSystem::default();
                let project =
                    load_project(entry, None, &fs).map_err(|error| format!("{error:?}"))?;
                compile_and_eval_from_project(&project, &HashMap::new())
                    .map_err(|error| format!("{error:?}"))
            }
        }
    }
}

// ============================================================================
// Fixture sweep
// ============================================================================

/// Number of seeded shuffles applied to each fixture. Deterministic rather
/// than proptest-random so a CI failure always reproduces locally from the
/// seed printed in the message.
const SHUFFLE_SEEDS: u64 = 12;

/// Fixtures the sweep cannot evaluate, with the reason. Every entry is
/// verified to still be unevaluatable, so the list cannot silently go stale.
///
/// Format: `(fixture id relative to tests/fixtures, reason)`.
const SKIPPED: &[(&str, &str)] = &[];

#[test]
fn all_valid_fixtures_are_declaration_order_independent() {
    let fixtures = valid_fixtures();
    assert!(
        fixtures.len() >= 100,
        "found only {} fixture entry points — discovery is probably broken",
        fixtures.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut stale_skips: Vec<String> = Vec::new();
    let mut never_permuted: Vec<String> = Vec::new();
    let mut checked = 0_usize;

    for fixture in &fixtures {
        let skip = SKIPPED.iter().find(|(id, _)| *id == fixture.id);
        let prepared = PreparedFixture::prepare(fixture);

        let baseline = match prepared.eval_original() {
            Ok(result) => result,
            Err(error) => {
                if skip.is_none() {
                    failures.push(format!(
                        "{}: fixture does not evaluate unmodified: {error}",
                        fixture.id
                    ));
                }
                continue;
            }
        };

        if let Some((_, reason)) = skip {
            stale_skips.push(format!(
                "{}: now evaluates cleanly, remove from SKIPPED ({reason})",
                fixture.id
            ));
            continue;
        }

        checked += 1;

        // Reassembling in the original order isolates harness bugs (a dropped
        // semicolon, a wrongly measured span) from genuine order-dependence, and
        // is the baseline the shuffles are measured against for reordering.
        let identity = prepared.permute(Permutation::Identity);
        let mut reordered = false;

        let permutations = std::iter::once((Permutation::Identity, identity.clone())).chain(
            (0..SHUFFLE_SEEDS).map(|seed| {
                let permutation = Permutation::Shuffled(seed);
                (permutation, prepared.permute(permutation))
            }),
        );

        for (permutation, texts) in permutations {
            if matches!(permutation, Permutation::Shuffled(_)) && texts != identity {
                reordered = true;
            }
            match prepared.eval(&texts) {
                Err(error) => failures.push(format!(
                    "{} [{}]: failed to evaluate after permutation: {error}",
                    fixture.id,
                    permutation.label()
                )),
                Ok(permuted) => {
                    if let Err(diffs) = compare_results(&baseline, &permuted) {
                        failures.push(format!(
                            "{} [{}]: results differ:\n{diffs}",
                            fixture.id,
                            permutation.label()
                        ));
                    }
                }
            }
        }

        // A fixture whose sources all hold a single declaration cannot be
        // reordered; anything else must actually have been permuted, or this
        // sweep would be passing without testing anything.
        let permutable = prepared
            .sources()
            .into_iter()
            .any(|source| declaration_slices(source).len() >= 2);
        if permutable && !reordered {
            never_permuted.push(fixture.id.clone());
        }
    }

    assert!(
        stale_skips.is_empty(),
        "{} stale SKIPPED entr(y/ies):\n{}",
        stale_skips.len(),
        stale_skips.join("\n")
    );
    assert!(
        never_permuted.is_empty(),
        "{} fixture(s) were never actually reordered by any of the {SHUFFLE_SEEDS} seeds — \
         the sweep would pass vacuously for them:\n{}",
        never_permuted.len(),
        never_permuted.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} declaration-order violation(s) across {checked} fixture(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ============================================================================
// Harness self-checks
// ============================================================================

/// Guard against a vacuous comparator. If [`compare_results`] ever stopped
/// looking at values, every test in this file would pass for the wrong reason.
#[test]
fn comparison_detects_a_real_difference() {
    let source = r"
        param a: Dimensionless = 1.0;
        node b: Dimensionless = @a * 2.0;
        assert b_small = @b < 10.0;
    ";
    let baseline = compile_and_eval(source).expect("baseline must evaluate");
    // `b` becomes 10.0 instead of 2.0, which also flips `b_small`.
    let mutated =
        compile_and_eval(&source.replace("1.0;", "5.0;")).expect("mutated source must evaluate");

    let diffs = compare_results(&baseline, &mutated)
        .expect_err("a changed value must be reported as a difference");
    assert!(
        diffs.contains("declaration `b`"),
        "changed node value must be reported, got:\n{diffs}"
    );
    assert!(
        diffs.contains("assertion `b_small`"),
        "flipped assertion must be reported, got:\n{diffs}"
    );
}

/// Only the opaque offset is erased; the surrounding name is left intact so a
/// value moving between two *named* scopes is still a reported difference.
#[test]
fn anonymous_include_normalization_is_narrow() {
    assert_eq!(
        normalize_anonymous_includes("left.<include@63>.base_val"),
        "left.<include>.base_val"
    );
    assert_eq!(
        normalize_anonymous_includes("right.<include@65>.base_val"),
        "right.<include>.base_val"
    );
    assert_eq!(
        normalize_anonymous_includes("a.<include@1>.b.<include@22>.c"),
        "a.<include>.b.<include>.c"
    );
    assert_eq!(normalize_anonymous_includes("plain.name"), "plain.name");
}

// ============================================================================
// Targeted forward-reference tests
// ============================================================================

/// Derived dimension declared before the dimension it references.
#[test]
fn forward_ref_derived_dimension() {
    let source = r"
        dim CustomAcceleration = Speed / Time;
        dim Speed = Length / Time;
        const node g0: CustomAcceleration = 9.80665 m/s^2;
    ";
    compile_and_eval(source).expect("forward-ref derived dimension must compile and evaluate");
}

/// Unit declared before the unit it references in its definition.
#[test]
fn forward_ref_unit() {
    let source = r"
        base dim CustomLength;
        const unit km_custom: CustomLength = 1000 m_base;
        base unit m_base: CustomLength;
        const node dist: CustomLength = 5.0 km_custom;
    ";
    compile_and_eval(source).expect("forward-ref unit must compile and evaluate");
}

/// Chain of derived dimensions: A depends on B depends on C, declared in reverse.
#[test]
fn forward_ref_derived_dimension_chain() {
    let source = r"
        pub dim Jerk = CustomAcceleration / Time;
        pub dim CustomAcceleration = Speed / Time;
        pub dim Speed = Length / Time;
        param j: Jerk = 1.0 m/s^3;
    ";
    compile_and_eval(source).expect("chained forward-ref dimensions must compile and evaluate");
}

/// Range index declared before the unit it uses in its start/end/step.
#[test]
fn forward_ref_range_index_unit() {
    let source = r"
        index Distances = range(0.0 custom_m, 100.0 custom_m, step: 10.0 custom_m);
        const unit custom_m: Length = 1.0 m;
        node num_points: Int = count(for d: Distances { 1.0 });
    ";
    compile_and_eval(source).expect("range index with forward-ref unit must compile and evaluate");
}

// ============================================================================
// Property-based tests
// ============================================================================
//
// The sweep above covers every fixture at a fixed set of seeds; these cover a
// handful of representative fixtures across a much wider seed space.

proptest! {
    #[test]
    fn rocket_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }

    #[test]
    fn indexed_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/valid/indexed.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }

    #[test]
    fn range_index_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/valid/range_index.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }

    #[test]
    fn mixed_index_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/valid/mixed_index.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }
}
