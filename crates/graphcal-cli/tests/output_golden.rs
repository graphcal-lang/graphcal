//! Output golden for every checked-in Graphcal source.
//!
//! Each fixture, example, and documentation playground file gets one golden
//! JSON file under `tests/output_golden/` recording:
//!
//! - the `graphcal check` outcome as diagnostic codes, severities, and label
//!   spans (message wording is covered by `graphcal-eval`'s `error_snapshots`);
//! - the exit code and stdout of `graphcal eval --format json --output-view all`,
//!   which pins values, presentation, and assertion results.
//!
//! Floats are compared with a relative tolerance because platform `libm`
//! implementations differ in the last units of precision. Collections with
//! many children (sampled series) are pinned by a summary: size, first
//! children, numeric statistics, and a digest of their non-float structure.
//!
//! The goldens are a safety net for behavior-preserving refactors: any change
//! in them must be intentional. Regenerate them with
//! `GRAPHCAL_UPDATE_GOLDEN=1 cargo test -p graphcal --test output_golden` and
//! review the diff.
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

use graphcal_eval::eval::ProjectCompiler;
use graphcal_eval::loader::{build_rooted_filesystem, load_project};
use miette::{Diagnostic, SourceSpan};
use serde_json::{Value, json};

/// Directories (relative to the workspace root) whose `.gcl` files are pinned.
const SOURCE_DIRECTORIES: &[&str] = &[
    "tests/fixtures",
    "docs/en/assets/playground/examples",
    "web/playground/examples",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate lives two levels below the workspace root")
        .to_path_buf()
}

fn collect_sources(root: &Path) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = SOURCE_DIRECTORIES
        .iter()
        .flat_map(|directory| walkdir::WalkDir::new(root.join(directory)))
        .map(|entry| entry.expect("fixture directory is readable"))
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| path.extension().is_some_and(|extension| extension == "gcl"))
        .collect();
    sources.sort();
    sources
}

/// Workspace-relative, `/`-separated display path for stable snapshots.
fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("source lies under the workspace root")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Replace the machine-specific workspace root inside `text` with `<root>`,
/// using `/` separators after it so snapshots match on every platform.
fn normalize_paths(text: &str, root: &Path) -> String {
    let root = root.display().to_string();
    // Windows canonical paths may carry the verbatim `\\?\` prefix.
    let verbatim_root = format!(r"\\?\{root}");
    let mut normalized = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((start, matched)) = [verbatim_root.as_str(), root.as_str()]
        .into_iter()
        .filter_map(|candidate| rest.find(candidate).map(|index| (index, candidate)))
        .min_by_key(|(index, candidate)| (*index, std::cmp::Reverse(candidate.len())))
    {
        normalized.push_str(&rest[..start]);
        normalized.push_str("<root>");
        let after = &rest[start + matched.len()..];
        let path_end = after
            .find(|character: char| {
                character.is_whitespace() || character == ':' || character == '`'
            })
            .unwrap_or(after.len());
        normalized.push_str(&after[..path_end].replace('\\', "/"));
        rest = &after[path_end..];
    }
    normalized.push_str(rest);
    normalized
}

/// Relative tolerance for float comparison.
const FLOAT_RELATIVE_TOLERANCE: f64 = 1e-9;
/// Absolute floor so that values that are zero up to rounding compare equal.
const FLOAT_ABSOLUTE_TOLERANCE: f64 = 1e-18;

/// Collections (arrays, and objects such as indexed `entries`) with more
/// children than this are pinned by a summary so that sampled series do not
/// produce multi-megabyte goldens.
const MAX_INLINE_CHILDREN: usize = 64;
const SUMMARY_PREFIX_LEN: usize = 8;

/// Environment variable that rewrites the goldens instead of comparing.
const UPDATE_ENV: &str = "GRAPHCAL_UPDATE_GOLDEN";

/// Replace every float in `value` with `null`, keeping the rest of the tree.
fn without_floats(value: &Value) -> Value {
    match value {
        Value::Number(number) if number.is_f64() => Value::Null,
        Value::Array(items) => Value::Array(items.iter().map(without_floats).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| (key.clone(), without_floats(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn collect_floats(value: &Value, floats: &mut Vec<f64>) {
    match value {
        Value::Number(number) if number.is_f64() => floats.extend(number.as_f64()),
        Value::Array(items) => items.iter().for_each(|item| collect_floats(item, floats)),
        Value::Object(fields) => fields
            .values()
            .for_each(|item| collect_floats(item, floats)),
        _ => {}
    }
}

/// Summarize a large normalized collection: its size and first children, the
/// SHA-256 of its structure with floats erased, and float statistics that
/// are free of cancellation (so they compare stably under tolerance).
fn summarize_collection(collection: &Value, length: usize, head: &Value) -> Value {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let structure =
        serde_json::to_vec(&without_floats(collection)).expect("golden collection serializes");
    let structure_sha256 =
        sha2::Sha256::digest(&structure)
            .iter()
            .fold(String::new(), |mut hex, byte| {
                write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
                hex
            });
    let mut floats = Vec::new();
    collect_floats(collection, &mut floats);
    let floats_summary = json!({
        "count": floats.len(),
        "sum_abs": floats.iter().map(|value| value.abs()).sum::<f64>(),
        "min": floats.iter().copied().reduce(f64::min),
        "max": floats.iter().copied().reduce(f64::max),
    });
    json!({
        "length": length,
        "head": head,
        "floats": floats_summary,
        "structure_sha256": structure_sha256,
    })
}

/// Make a JSON value machine-independent and compact: normalize embedded
/// paths and summarize large collections.
fn normalize_json(value: Value, root: &Path) -> Value {
    match value {
        Value::String(text) => Value::String(normalize_paths(&text, root)),
        Value::Array(items) => {
            let items: Vec<Value> = items
                .into_iter()
                .map(|item| normalize_json(item, root))
                .collect();
            match items.len() {
                length if length > MAX_INLINE_CHILDREN => {
                    let head = Value::Array(items[..SUMMARY_PREFIX_LEN].to_vec());
                    summarize_collection(&Value::Array(items), length, &head)
                }
                _ => Value::Array(items),
            }
        }
        Value::Object(fields) => {
            let fields: serde_json::Map<String, Value> = fields
                .into_iter()
                .map(|(key, item)| (key, normalize_json(item, root)))
                .collect();
            match fields.len() {
                length if length > MAX_INLINE_CHILDREN => {
                    let head = Value::Object(
                        fields
                            .iter()
                            .take(SUMMARY_PREFIX_LEN)
                            .map(|(key, item)| (key.clone(), item.clone()))
                            .collect(),
                    );
                    summarize_collection(&Value::Object(fields), length, &head)
                }
                _ => Value::Object(fields),
            }
        }
        other @ (Value::Null | Value::Bool(_) | Value::Number(_)) => other,
    }
}

fn floats_close(expected: f64, actual: f64) -> bool {
    (expected - actual).abs()
        <= FLOAT_RELATIVE_TOLERANCE
            .mul_add(expected.abs().max(actual.abs()), FLOAT_ABSOLUTE_TOLERANCE)
}

/// Compare two golden trees, recording a message per differing leaf.
fn compare_json(expected: &Value, actual: &Value, path: &str, mismatches: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Number(left), Value::Number(right)) if left.is_f64() || right.is_f64() => {
            match (left.as_f64(), right.as_f64()) {
                (Some(left), Some(right)) if floats_close(left, right) => {}
                _ => mismatches.push(format!("{path}: expected {left}, got {right}")),
            }
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                compare_json(left, right, &format!("{path}[{index}]"), mismatches);
            }
        }
        (Value::Object(left), Value::Object(right)) if left.keys().eq(right.keys()) => {
            for (key, left) in left {
                compare_json(left, &right[key], &format!("{path}.{key}"), mismatches);
            }
        }
        (left, right) if left == right => {}
        (left, right) => mismatches.push(format!("{path}: expected {left}, got {right}")),
    }
}

fn span_json(diagnostic: &dyn Diagnostic, span: SourceSpan, root: &Path) -> Value {
    let location = diagnostic
        .source_code()
        .and_then(|source| source.read_span(&span, 0, 0).ok())
        .map(|contents| {
            let name = contents.name().map(|name| normalize_paths(name, root));
            json!({
                "source": name,
                "line": contents.line() + 1,
                "column": contents.column() + 1,
            })
        });
    json!({
        "offset": span.offset(),
        "length": span.len(),
        "location": location,
    })
}

/// Structural projection of a diagnostic tree: codes, severities, and spans.
fn diagnostic_json(diagnostic: &dyn Diagnostic, root: &Path) -> Value {
    let labels: Vec<Value> = diagnostic
        .labels()
        .into_iter()
        .flatten()
        .map(|label| {
            let mut value = span_json(diagnostic, *label.inner(), root);
            value["primary"] = json!(label.primary());
            value
        })
        .collect();
    let related: Vec<Value> = diagnostic
        .related()
        .into_iter()
        .flatten()
        .map(|related| diagnostic_json(related, root))
        .collect();
    json!({
        "code": diagnostic.code().map(|code| code.to_string()),
        "severity": diagnostic.severity().map(|severity| format!("{severity:?}")),
        "labels": labels,
        "related": related,
        "source": diagnostic.diagnostic_source().map(|source| diagnostic_json(source, root)),
    })
}

fn check_json(path: &Path, root: &Path) -> Value {
    let fs = match build_rooted_filesystem(path, None) {
        Ok(fs) => fs,
        Err(error) => return diagnostic_json(&error, root),
    };
    let project = match load_project(path, None, &fs) {
        Ok(project) => project,
        Err(error) => return diagnostic_json(&error, root),
    };
    let mut host_fns = graphcal_eval::host_fns::demo_registry();
    graphcal_plugin_host::register_project_plugins(
        &graphcal_plugin_host::PluginHost::new(),
        &project,
        &mut host_fns,
    );
    match ProjectCompiler::new(&project).host_fns(&host_fns).check() {
        Ok(_) => json!("ok"),
        Err(error) => diagnostic_json(&error, root),
    }
}

fn eval_json(path: &Path, root: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_graphcal"))
        .current_dir(root)
        .args(["eval", "--format", "json", "--output-view", "all"])
        .arg(path)
        .env("NO_COLOR", "1")
        .output()
        .expect("graphcal binary runs");
    let stdout = serde_json::from_slice::<Value>(&output.stdout)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&output.stdout)));
    json!({ "exit_code": output.status.code(), "stdout": stdout })
}

fn golden(path: &Path, root: &Path) -> Value {
    let record = json!({
        "check": check_json(path, root),
        "eval": eval_json(path, root),
    });
    normalize_json(record, root)
}

fn golden_file_name(name: &str) -> String {
    format!("{}.json", name.trim_end_matches(".gcl").replace('/', "__"))
}

#[test]
fn output_golden_for_every_checked_in_source() {
    let root = workspace_root();
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output_golden");
    let sources = collect_sources(&root);
    assert!(
        !sources.is_empty(),
        "no Graphcal sources found under {root:?}"
    );

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    let chunk_size = sources.len().div_ceil(workers);
    let records: Vec<(String, Value)> = std::thread::scope(|scope| {
        #[expect(
            clippy::needless_collect,
            reason = "spawn every worker before joining any of them"
        )]
        let handles: Vec<_> = sources
            .chunks(chunk_size)
            .map(|chunk| {
                let root = &root;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|path| (relative_display(root, path), golden(path, root)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("golden worker does not panic"))
            .collect()
    });

    let expected_files: std::collections::BTreeSet<String> = records
        .iter()
        .map(|(name, _)| golden_file_name(name))
        .collect();
    let existing_files: std::collections::BTreeSet<String> = match std::fs::read_dir(&golden_dir) {
        Ok(entries) => entries
            .map(|entry| entry.expect("golden directory entry is readable"))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        // A missing directory means no goldens yet; every source reports missing.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::collections::BTreeSet::new()
        }
        Err(error) => panic!("cannot read {}: {error}", golden_dir.display()),
    };

    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::create_dir_all(&golden_dir).expect("golden directory is creatable");
        for stale in existing_files.difference(&expected_files) {
            std::fs::remove_file(golden_dir.join(stale)).expect("stale golden is removable");
        }
        for (name, record) in &records {
            let text = serde_json::to_string_pretty(record).expect("golden record serializes");
            std::fs::write(golden_dir.join(golden_file_name(name)), text + "\n")
                .expect("golden file is writable");
        }
        return;
    }

    let mut failures: Vec<String> = existing_files
        .difference(&expected_files)
        .map(|stale| format!("{stale}: golden has no source"))
        .collect();
    for (name, record) in &records {
        let file = golden_file_name(name);
        let Ok(text) = std::fs::read_to_string(golden_dir.join(&file)) else {
            failures.push(format!("{file}: golden is missing"));
            continue;
        };
        let expected: Value = serde_json::from_str(&text).expect("golden file is valid JSON");
        let mut mismatches = Vec::new();
        compare_json(&expected, record, "$", &mut mismatches);
        if !mismatches.is_empty() {
            mismatches.truncate(20);
            failures.push(format!("{name}:\n  {}", mismatches.join("\n  ")));
        }
    }
    assert!(
        failures.is_empty(),
        "output golden mismatch ({} file(s)); rerun with {UPDATE_ENV}=1 to accept intended changes:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
