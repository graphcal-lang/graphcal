//! Allow use of unwrap in tests
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn fixtures_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tests");
    p.push("fixtures");
    p
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR").trim_end_matches("crates/graphcal-cli"),
        name
    )
}

fn write_temp_file(root: &Path, rel: &str, source: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, source).unwrap();
    path
}

fn commit_git_repo(root: &Path, message: &str) -> String {
    let repo = gix::open(root).unwrap();
    let tree_id = write_tree_object(&repo, root);
    let parents = repo
        .head_id()
        .map(|id| vec![id.detach()])
        .unwrap_or_default();
    let signature =
        gix::actor::SignatureRef::from_bytes(b"Graphcal Test <graphcal@example.invalid> 0 +0000")
            .unwrap();
    repo.commit_as(signature, signature, "HEAD", message, tree_id, parents)
        .unwrap()
        .detach()
        .to_hex()
        .to_string()
}

fn write_tree_object(repo: &gix::Repository, root: &Path) -> gix::hash::ObjectId {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let filename = entry.file_name();
        if filename == ".git" {
            continue;
        }
        let file_type = entry.file_type().unwrap();
        let (mode, oid) = if file_type.is_dir() {
            (
                gix::objs::tree::EntryKind::Tree.into(),
                write_tree_object(repo, &path),
            )
        } else if file_type.is_file() {
            (
                gix::objs::tree::EntryKind::Blob.into(),
                repo.write_blob(std::fs::read(path).unwrap())
                    .unwrap()
                    .detach(),
            )
        } else {
            panic!("unsupported test fixture entry `{}`", path.display());
        };
        entries.push(gix::objs::tree::Entry {
            mode,
            filename: filename.to_string_lossy().into_owned().into(),
            oid,
        });
    }
    entries.sort();
    repo.write_object(gix::objs::Tree { entries })
        .unwrap()
        .detach()
}

fn init_git_package(
    root: &Path,
    package: &str,
    manifest_tail: &str,
    module_source: &str,
) -> String {
    std::fs::create_dir_all(root.join(format!("src/{package}"))).unwrap();
    std::fs::write(
        root.join("graphcal.toml"),
        format!("[package]\nname = \"{package}\"\n{manifest_tail}"),
    )
    .unwrap();
    std::fs::write(root.join(format!("src/{package}/lib.gcl")), module_source).unwrap();
    gix::init(root).unwrap();
    commit_git_repo(root, "initial")
}

fn create_git_package(root: &Path, package: &str, module_source: &str) -> String {
    init_git_package(root, package, "", module_source)
}

fn update_git_package_module(
    root: &Path,
    package: &str,
    module_source: &str,
    message: &str,
) -> String {
    std::fs::write(root.join(format!("src/{package}/lib.gcl")), module_source).unwrap();
    commit_git_repo(root, message)
}

fn write_package_project(root: &Path, manifest_dependencies: &str, main_source: &str) -> PathBuf {
    std::fs::create_dir_all(root.join("src/mission")).unwrap();
    let main = root.join("src/mission/main.gcl");
    std::fs::write(&main, main_source).unwrap();
    std::fs::write(
        root.join("graphcal.toml"),
        format!(
            "[package]\n\
             name = \"mission\"\n\
             source_dir = \"src\"\n\
             {manifest_dependencies}"
        ),
    )
    .unwrap();
    main
}

fn find_cached_file(root: &Path, suffix: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.ends_with(suffix) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_cached_file(&path, suffix)
        {
            return Some(found);
        }
    }
    None
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "end-to-end CLI scenario keeps setup, command, and assertions together"
)]
fn deps_lock_writes_deterministic_git_lockfile() {
    let dir = tempfile::tempdir().unwrap();
    let dep_repo = dir.path().join("units-repo");
    let dep_rev = create_git_package(
        &dep_repo,
        "units",
        "pub const node one: Dimensionless = 1.0;\n",
    );
    let project = dir.path().join("mission");
    std::fs::create_dir_all(project.join("src/mission")).unwrap();
    let main = project.join("src/mission/main.gcl");
    std::fs::write(
        &main,
        "import units_v1.lib.{ one };\n\
         node two: Dimensionless = @one + 1.0;\n",
    )
    .unwrap();
    std::fs::write(
        project.join("graphcal.toml"),
        format!(
            "[package]\n\
             name = \"mission\"\n\
             source_dir = \"src\"\n\n\
             [dependencies]\n\
             units_v1 = {{ package = \"units\", git = \"file://{}\", rev = \"{dep_rev}\" }}\n",
            dep_repo.display()
        ),
    )
    .unwrap();
    let cache = dir.path().join("cache");

    let output = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wrote"), "stdout: {stdout}");

    let lock = std::fs::read_to_string(project.join("graphcal.lock")).unwrap();
    assert!(lock.contains("lock_version = 1"), "lock:\n{lock}");
    assert!(lock.contains("root = \"pkg-mission\""), "lock:\n{lock}");
    assert!(
        lock.contains("units_v1 = \"pkg-units-"),
        "dependency edge missing:\n{lock}"
    );
    assert!(
        lock.contains(&format!("requested_rev = \"{dep_rev}\"")),
        "requested_rev missing:\n{lock}"
    );
    assert!(
        lock.contains(&format!("commit = \"{dep_rev}\"")),
        "commit missing:\n{lock}"
    );
    assert!(lock.contains("tree_hashes = { sha256 = \""));
    assert!(
        !lock.contains(cache.to_str().unwrap()),
        "lock must not serialize cache paths:\n{lock}"
    );

    let check = graphcal_bin()
        .args([
            "check",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let eval = graphcal_bin()
        .args([
            "eval",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        eval.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&eval.stderr)
    );
    let stdout = String::from_utf8(eval.stdout).unwrap();
    assert!(stdout.contains("two = 2"), "stdout: {stdout}");

    let second = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout = String::from_utf8(second.stdout).unwrap();
    assert!(stdout.contains("up to date"), "stdout: {stdout}");
}

#[test]
fn deps_lock_rejects_floating_git_refs() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("mission");
    std::fs::create_dir_all(project.join("src/mission")).unwrap();
    std::fs::write(
        project.join("src/mission/main.gcl"),
        "param dry: kg = 1.0;\n",
    )
    .unwrap();
    std::fs::write(
        project.join("graphcal.toml"),
        "[package]\n\
         name = \"mission\"\n\n\
         [dependencies]\n\
         units = { git = \"https://github.com/acme/units.git\", branch = \"main\" }\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", dir.path().join("cache"))
        .output()
        .expect("failed to run graphcal");
    assert!(!output.status.success(), "floating refs must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported dependency field `branch`"),
        "stderr: {stderr}"
    );
    assert!(
        !project.join("graphcal.lock").exists(),
        "failed lock command must not write graphcal.lock"
    );
}

#[test]
fn package_consumers_fail_without_lockfile() {
    let dir = tempfile::tempdir().unwrap();
    let dep_repo = dir.path().join("units-repo");
    let dep_rev = create_git_package(
        &dep_repo,
        "units",
        "pub const node one: Dimensionless = 1.0;\n",
    );
    let project = dir.path().join("mission");
    let main = write_package_project(
        &project,
        &format!(
            "\n[dependencies]\n\
             units = {{ git = \"file://{}\", rev = \"{dep_rev}\" }}\n",
            dep_repo.display()
        ),
        "import units.lib.{ one };\n\
         node two: Dimensionless = @one + 1.0;\n",
    );
    let cache = dir.path().join("cache");

    let output = graphcal_bin()
        .args([
            "check",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(!output.status.success(), "missing lockfile must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("graphcal deps lock"), "stderr: {stderr}");
    assert!(!project.join("graphcal.lock").exists());
    assert!(!cache.exists(), "consumer command must not create cache");
}

#[test]
fn package_consumers_reject_cached_source_hash_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let dep_repo = dir.path().join("units-repo");
    let dep_rev = create_git_package(
        &dep_repo,
        "units",
        "pub const node one: Dimensionless = 1.0;\n",
    );
    let project = dir.path().join("mission");
    let main = write_package_project(
        &project,
        &format!(
            "\n[dependencies]\n\
             units = {{ git = \"file://{}\", rev = \"{dep_rev}\" }}\n",
            dep_repo.display()
        ),
        "import units.lib.{ one };\n\
         node two: Dimensionless = @one + 1.0;\n",
    );
    let cache = dir.path().join("cache");
    let lock = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        lock.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&lock.stderr)
    );

    let cached_lib = find_cached_file(&cache, Path::new("src/units/lib.gcl")).unwrap();
    std::fs::write(cached_lib, "pub const node one: Dimensionless = 99.0;\n").unwrap();

    let output = graphcal_bin()
        .args([
            "check",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(!output.status.success(), "hash mismatch must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("hash mismatch"), "stderr: {stderr}");
    assert!(stderr.contains("graphcal deps lock"), "stderr: {stderr}");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "contextual dependency graph scenario is clearer as one integration test"
)]
fn deps_lock_supports_contextual_transitive_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let units_repo = dir.path().join("units-repo");
    let units_rev1 = create_git_package(
        &units_repo,
        "units",
        "pub const node one: Dimensionless = 1.0;\n",
    );
    let units_rev2 = update_git_package_module(
        &units_repo,
        "units",
        "pub const node one: Dimensionless = 2.0;\n",
        "second",
    );
    let orbital_repo = dir.path().join("orbital-repo");
    let orbital_rev = init_git_package(
        &orbital_repo,
        "orbital",
        &format!(
            "\n[dependencies]\n\
             units = {{ git = \"file://{}\", rev = \"{units_rev1}\" }}\n",
            units_repo.display()
        ),
        "import units.lib.{ one as units_one };\n\
         pub const node one: Dimensionless = @units_one;\n",
    );
    let thermal_repo = dir.path().join("thermal-repo");
    let thermal_rev = init_git_package(
        &thermal_repo,
        "thermal",
        &format!(
            "\n[dependencies]\n\
             units = {{ git = \"file://{}\", rev = \"{units_rev2}\" }}\n",
            units_repo.display()
        ),
        "import units.lib.{ one as units_one };\n\
         pub const node one: Dimensionless = @units_one;\n",
    );
    let project = dir.path().join("mission");
    let main = write_package_project(
        &project,
        &format!(
            "\n[dependencies]\n\
             orbital = {{ git = \"file://{}\", rev = \"{orbital_rev}\" }}\n\
             thermal = {{ git = \"file://{}\", rev = \"{thermal_rev}\" }}\n",
            orbital_repo.display(),
            thermal_repo.display()
        ),
        "import orbital.lib.{ one as orbital_one };\n\
         import thermal.lib.{ one as thermal_one };\n\
         node sum: Dimensionless = @orbital_one + @thermal_one;\n",
    );
    let cache = dir.path().join("cache");

    let lock = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        lock.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let lock_text = std::fs::read_to_string(project.join("graphcal.lock")).unwrap();
    assert!(
        lock_text.contains("units = \"pkg-units-"),
        "transitive units edge missing:\n{lock_text}"
    );

    let eval = graphcal_bin()
        .args([
            "eval",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        eval.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&eval.stderr)
    );
    let stdout = String::from_utf8(eval.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("sum") && line.contains("= 3")),
        "stdout: {stdout}"
    );

    let bad_main = project.join("src/mission/bad.gcl");
    std::fs::write(
        &bad_main,
        "import units.lib.{ one };\n\
         node x: Dimensionless = @one;\n",
    )
    .unwrap();
    let bad = graphcal_bin()
        .args([
            "check",
            bad_main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        !bad.status.success(),
        "implicit transitive import must fail"
    );
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("unknown dependency `units`"),
        "stderr: {stderr}"
    );
}

#[test]
fn package_instance_identities_distinguish_aliases_and_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let units_repo = dir.path().join("units-repo");
    let units_rev1 = create_git_package(
        &units_repo,
        "units",
        "pub base dim Money;\n\
         pub base unit USD: Money;\n\
         pub const node price: Money = 1.0 USD;\n",
    );
    let units_rev2 = update_git_package_module(
        &units_repo,
        "units",
        "pub base dim Money;\n\
         pub base unit USD: Money;\n\
         pub const node price: Money = 1.0 USD;\n\
         // same API, different locked package instance\n",
        "second",
    );
    let project = dir.path().join("mission");
    let main = write_package_project(
        &project,
        &format!(
            "\n[dependencies]\n\
             units_a = {{ package = \"units\", git = \"file://{}\", rev = \"{units_rev1}\" }}\n\
             units_b = {{ package = \"units\", git = \"file://{}\", rev = \"{units_rev1}\" }}\n\
             units_v2 = {{ package = \"units\", git = \"file://{}\", rev = \"{units_rev2}\" }}\n",
            units_repo.display(),
            units_repo.display(),
            units_repo.display()
        ),
        "import units_a.lib.{ Money as MoneyA, price as price_a };\n\
         import units_b.lib.{ price as price_b };\n\
         import units_v2.lib.{ Money as MoneyV2, price as price_v2 };\n\
         node shared_sum: MoneyA = @price_a + @price_b;\n\
         node keep_v2: MoneyV2 = @price_v2;\n",
    );
    let cache = dir.path().join("cache");
    let lock = graphcal_bin()
        .args(["deps", "lock", "--root", project.to_str().unwrap()])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        lock.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let check = graphcal_bin()
        .args([
            "check",
            main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let bad_main = project.join("src/mission/bad_money.gcl");
    std::fs::write(
        &bad_main,
        "import units_a.lib.{ Money as MoneyA, price as price_a };\n\
         import units_v2.lib.{ price as price_v2 };\n\
         node bad_sum: MoneyA = @price_a + @price_v2;\n",
    )
    .unwrap();
    let bad = graphcal_bin()
        .args([
            "check",
            bad_main.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
        ])
        .env("GRAPHCAL_CACHE_DIR", &cache)
        .output()
        .expect("failed to run graphcal");
    assert!(
        !bad.status.success(),
        "different package instances must keep package-defined dimensions distinct"
    );
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("dimension") || stderr.contains("Dimension"),
        "stderr: {stderr}"
    );
}

#[test]
fn eval_rocket_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Source order: dry_mass, fuel_mass, isp, g0, v_exhaust, mass_ratio, delta_v
    assert_eq!(lines.len(), 7);
    assert!(lines[0].contains("dry_mass"));
    assert!(lines[1].contains("fuel_mass"));
    assert!(lines[2].contains("isp"));
    assert!(lines[3].contains("g0"));
    assert!(lines[4].contains("v_exhaust"));
    assert!(lines[5].contains("mass_ratio"));
    assert!(lines[6].contains("delta_v"));

    // Check values
    assert!(lines[0].contains("1200"));
    assert!(lines[3].contains("9.80665"));
    assert!(lines[4].contains("3138.128"));
}

#[test]
fn eval_rocket_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    assert!(json["const"]["g0"]["si_value"].as_f64().is_some());
    assert!(
        (json["param"]["dry_mass"]["si_value"].as_f64().unwrap() - 1200.0).abs() < f64::EPSILON
    );
    assert!(json["node"]["v_exhaust"]["si_value"].as_f64().is_some());
}

#[test]
fn eval_nonexistent_file_fails() {
    let output = graphcal_bin()
        .args(["eval", "nonexistent.gcl"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("file not found"),
        "expected 'file not found' in stderr: {stderr}"
    );
}
#[test]
fn eval_indexed_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Indexed values flatten: delta_v[Departure], delta_v[Correction], delta_v[Insertion], etc.
    // Check key lines exist
    assert!(
        lines.iter().any(|l| l.contains("delta_v[Departure]")),
        "missing delta_v[Departure]: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("total_dv")),
        "missing total_dv: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("cumulative_dv[Insertion]")),
        "missing cumulative_dv[Insertion]: {lines:?}"
    );
}

#[test]
fn eval_indexed_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/indexed.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // delta_v is an indexed param
    let dv = &json["param"]["delta_v"];
    assert_eq!(dv["index"].as_str(), Some("Maneuver"));
    assert!(dv["entries"]["Departure"]["si_value"].as_f64().is_some());

    // total_dv is a scalar node
    assert!(json["node"]["total_dv"]["si_value"].as_f64().is_some());
}

#[test]
fn eval_same_leaf_imported_indexes_display_as_boundary_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series_a: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n\
         node series_b: Dimensionless[b.Phase] = for p: b.Phase { 2.0 };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("series_a[Burn]"), "stdout: {stdout}");
    assert!(stdout.contains("series_b[Coast]"), "stdout: {stdout}");

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(json["node"]["series_a"]["index"].as_str(), Some("Phase"));
    assert_eq!(json["node"]["series_b"]["index"].as_str(), Some("Phase"));
}

#[test]
fn eval_multiple_includes_qualified_output() {
    // #813: multiple instantiations of the same dag must stay distinct in
    // both text and JSON output, keyed by their include alias path.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
dag checked {
    param v: Dimensionless;
    pub node out: Dimensionless = @v * 2.0;
    assert v_positive = @v > 0.0;
}

include checked(v: 1.0) as good;
include checked(v: -1.0) as bad;
node sum2: Dimensionless = @good.out + @bad.out;
",
    );

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let combined = format!("{stdout}{stderr}");
    for needle in [
        "good.v ",
        "good.out",
        "bad.v ",
        "bad.out",
        "good.v_positive",
        "bad.v_positive",
    ] {
        assert!(
            combined.contains(needle),
            "missing `{needle}` in output:\n{combined}"
        );
    }
    assert!(
        combined.contains("good.v_positive  PASS"),
        "good instance should pass:\n{combined}"
    );
    assert!(
        combined.contains("bad.v_positive   FAIL"),
        "bad instance should fail:\n{combined}"
    );

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("failed to run graphcal");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // JSON keeps one entry per instance — nothing is silently dropped.
    assert_eq!(json["param"]["good.v"]["si_value"].as_f64(), Some(1.0));
    assert_eq!(json["param"]["bad.v"]["si_value"].as_f64(), Some(-1.0));
    assert_eq!(json["node"]["good.out"]["si_value"].as_f64(), Some(2.0));
    assert_eq!(json["node"]["bad.out"]["si_value"].as_f64(), Some(-2.0));
    assert_eq!(json["node"]["sum2"]["si_value"].as_f64(), Some(0.0));
    assert_eq!(
        json["assert"]["good.v_positive"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["bad.v_positive"]["status"].as_str(),
        Some("fail")
    );
}

#[test]
fn eval_multiple_includes_expected_fail_attribution() {
    // #813: with #[expected_fail] on the dag's assert, the per-instance
    // results stay attributable — one PASS (failure occurred as expected)
    // and one "unexpected pass" FAIL.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
dag checked {
    param v: Dimensionless;
    pub node out: Dimensionless = @v * 2.0;
    #[expected_fail]
    assert is_neg = @v < 0.0;
}

include checked(v: 1.0) as pos;
include checked(v: -1.0) as neg;
node use_both: Dimensionless = @pos.out + @neg.out;
",
    );

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("pos.is_neg  PASS"),
        "pos instance fails as expected → PASS:\n{combined}"
    );
    assert!(
        combined.contains("neg.is_neg  FAIL  (assertion passed but was marked #[expected_fail])"),
        "neg instance passes unexpectedly → FAIL:\n{combined}"
    );
}

#[test]
fn check_rejects_duplicate_expected_fail_variant() {
    // Duplicate expected_fail keys are ambiguous and must be rejected at check time.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Mode.A, Mode.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "duplicate expected_fail keys should be rejected"
    );
}

#[test]
fn check_rejects_foreign_expected_fail_variant() {
    // Expected-fail keys must use the assertion's semantic index, not a foreign
    // index with the same variant leaves.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Other.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "expected_fail keys must belong to the assertion index"
    );
}

#[test]
fn check_rejects_duplicate_expected_fail_tuple() {
    // Duplicate multi-index expected_fail tuple keys must be rejected at check time.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase {
    match p {
        Phase.Hot => 2.0,
        Phase.Cold => 0.0,
    }
};
#[expected_fail((Mode.A, Phase.Hot), (Mode.A, Phase.Hot))]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "duplicate expected_fail tuple keys should be rejected"
    );
}

#[test]
fn check_rejects_partial_expected_fail_tuple() {
    // Multi-index assertions require full tuple keys.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 2.0 };
#[expected_fail(Mode.A)]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "multi-index expected_fail should require full tuple keys"
    );
}

#[test]
fn check_rejects_wrong_axis_expected_fail_tuple() {
    // Multi-index tuple keys must match the assertion's axis order exactly.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 2.0 };
#[expected_fail((Phase.Hot, Mode.A))]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "expected_fail tuple keys should match assertion axis order"
    );
}

#[test]
fn check_rejects_variant_arg_on_scalar_expected_fail() {
    // Scalar assertions cannot accept per-variant expected_fail metadata.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
param lhs: Dimensionless = 1.0;
param rhs: Dimensionless = 2.0;
#[expected_fail(Mode.A)]
assert order = @lhs > @rhs;
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "scalar expected_fail should not accept per-variant keys"
    );
}

#[test]
fn check_rejects_foreign_expected_fail_when_all_pass() {
    // Foreign expected_fail keys are invalid even if the assertion currently passes.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 3.0, Mode.B: 3.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 2.0 };
#[expected_fail(Other.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "foreign expected_fail keys should be rejected before evaluation"
    );
}

#[test]
fn check_rejects_private_include_output() {
    // Include brace selection must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ hidden };\nnode y: Dimensionless = @hidden;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include should not expose private outputs across modules"
    );
}

#[test]
fn check_rejects_private_include_output_renamed() {
    // Include brace renaming must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ hidden as leaked };\nnode y: Dimensionless = @leaked;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include output renaming should not bypass visibility"
    );
}

#[test]
fn check_rejects_private_include_output_via_alias() {
    // Include module aliases must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0) as h;\nnode y: Dimensionless = @h.hidden;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include aliases should not expose private outputs"
    );
}

#[test]
fn check_rejects_private_dag_include() {
    // A private DAG must not be instantiated from another module by full path.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "dag helper {\n  param x: Dimensionless;\n  pub node shown: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ shown };\nnode y: Dimensionless = @shown;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "private DAGs should not be includable across modules"
    );
}

#[test]
fn eval_invalid_syntax_fails() {
    // Create a temp file with invalid syntax
    let dir = std::env::temp_dir().join("graphcal_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.gcl");
    std::fs::write(&path, "this is not valid graphcal").unwrap();

    let output = graphcal_bin()
        .args(["eval", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    std::fs::remove_dir_all(&dir).ok();
}

// --- --set flag tests ---

#[test]
fn eval_with_set_flag() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=450.0 s"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // isp should show 450, not the default 320
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450 in output: {stdout}"
    );
    // delta_v should be higher than default (3778)
    let dv_line = stdout.lines().find(|l| l.contains("delta_v")).unwrap();
    assert!(
        dv_line.contains("5313"),
        "expected delta_v ~5313 with isp=450: {dv_line}"
    );
}

#[test]
fn eval_with_multiple_set() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "isp=450.0 s",
            "--set",
            "dry_mass=1500.0 kg",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500: {stdout}"
    );
}

#[test]
fn eval_set_invalid_param() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "nonexistent=100",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

#[test]
fn eval_user_defined_dimensions() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/user_dimensions.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("storage"));
    assert!(lines[0].contains("kB"));
    assert!(lines[1].contains("rate"));
    assert!(lines[1].contains("bit/s"));
    assert!(lines[2].contains("transfer_time"));
    assert!(lines[2].contains("40000"));
    assert!(lines[2].contains(" s"));
}

#[test]
fn eval_set_node_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "delta_v=100.0 m/s",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("node"),
        "expected error mentioning 'node': {stderr}"
    );
}

#[test]
fn eval_set_bad_value() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=???"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error"), "expected parse error: {stderr}");
}

// --- Multi-file import tests ---

#[test]
fn eval_missing_import_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/multi/missing_module/src/missing_module/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
}

// --- Tagged union tests ---

#[test]
fn eval_tagged_union_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/tagged_union.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Union type value shows fields directly: maneuver.thrust, maneuver.duration
    assert!(
        lines.iter().any(|l| l.contains("maneuver.thrust")),
        "expected maneuver.thrust in output: {stdout}"
    );
    assert!(
        lines.iter().any(|l| l.contains("maneuver.duration")),
        "expected maneuver.duration in output: {stdout}"
    );

    // Single-variant (struct sugar) shows flat fields: transfer.dv1
    assert!(
        lines.iter().any(|l| l.contains("transfer.dv1")),
        "expected transfer.dv1 in output: {stdout}"
    );
    assert!(
        lines.iter().any(|l| l.contains("transfer.dv2")),
        "expected transfer.dv2 in output: {stdout}"
    );

    // Bare variant displays as label
    assert!(
        lines
            .iter()
            .any(|l| l.contains("current_status") && l.contains("Nominal")),
        "expected current_status = Nominal in output: {stdout}"
    );
}

#[test]
fn eval_tagged_union_json_output() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/tagged_union.gcl"),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // Union type value shows concrete type name
    let maneuver = &json["node"]["maneuver"];
    assert_eq!(maneuver["type"].as_str(), Some("LowThrust"));
    assert!(maneuver["fields"]["thrust"]["si_value"].as_f64().is_some());

    // Record type (struct sugar)
    let transfer = &json["node"]["transfer"];
    assert_eq!(transfer["type"].as_str(), Some("TransferResult"));

    // Unit type value shows concrete type name
    let status = &json["node"]["current_status"];
    assert_eq!(status["type"].as_str(), Some("Nominal"));
}

#[test]
fn eval_import_name_not_found() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/multi/bad_name_import/src/bad_name_import/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

#[test]
fn check_rejects_type_only_import_for_constructor() {
    let output = graphcal_bin()
        .args([
            "check",
            &fixture("invalid/inline_dag_type_import_without_constructor.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "type-only import must not bring the constructor into scope"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("TransferResult"),
        "expected error mentioning TransferResult: {stderr}"
    );
}

// --- --input JSON file tests ---

#[test]
fn eval_with_input_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            &fixture("valid/input_rocket.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // dry_mass should show 1500 (from JSON), not default 1200
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500 in output: {stdout}"
    );
    // isp should show 450 (from JSON), not default 320
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_set_precedence() {
    // --set should override the same param from --input
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            &fixture("valid/input_rocket.json"),
            "--set",
            "isp=500.0 s",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // isp should show 500 (from --set), not 450 (from JSON)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("500")),
        "expected isp=500 in output: {stdout}"
    );
    // dry_mass should still come from JSON
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_indexed() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/indexed.gcl"),
            "--input",
            &fixture("valid/input_indexed.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // delta_v[Departure] should show 3000 (3.0 km/s in SI)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("Departure") && l.contains('3')),
        "expected Departure delta_v ~3 km/s in output: {stdout}"
    );
}

#[test]
fn eval_input_json_tagged_union() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/tagged_union_param.gcl"),
            "--input",
            &fixture("valid/input_tagged_union.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // maneuver should now be Impulsive (from JSON), not LowThrust (default)
    assert!(
        stdout.lines().any(|l| l.contains("maneuver.delta_v")),
        "expected maneuver.delta_v in output: {stdout}"
    );
    // fuel_proxy should be 0 N (Impulsive branch returns 0)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("fuel_proxy") && l.contains('0')),
        "expected fuel_proxy=0 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_multiple_structured_overrides() {
    // Regression for #764: several indexed/tagged-union overrides in one JSON
    // file. Span-keyed TIR semantic metadata gave every synthetic override the
    // same (or per-entry colliding) spans, so one override's map keys and
    // constructor callees could resolve against another override's metadata.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi_override_params.gcl"),
            "--input",
            &fixture("valid/input_multi_override.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // total_dv = (3.0 + 2.0) km/s from delta_v + (0.5 + 0.25) km/s from boost
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("total_dv") && l.contains("5750")),
        "expected total_dv=5750 m/s in output: {stdout}"
    );
    // maneuver overridden to Impulsive -> fuel_proxy takes the 0 N branch
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("fuel_proxy") && l.contains('0')),
        "expected fuel_proxy=0 in output: {stdout}"
    );
    // guidance overridden to Closed(gain: 2.0)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("gain_proxy") && l.contains('2')),
        "expected gain_proxy=2 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_unknown_param() {
    let dir = std::env::temp_dir().join("graphcal_test_input");
    std::fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("bad_param.json");
    std::fs::write(&json_path, r#"{"nonexistent": "100.0 kg"}"#).unwrap();

    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            json_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");

    // Should fail because "nonexistent" is not a param in rocket.gcl
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn eval_input_json_invalid_json() {
    let dir = std::env::temp_dir().join("graphcal_test_input_bad");
    std::fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("bad.json");
    std::fs::write(&json_path, "not valid json {{{").unwrap();

    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            json_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("error"),
        "expected JSON parse error: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
// --- Assertion tests ---

#[test]
fn eval_assertions_pass() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/assertions.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Assertions:"),
        "expected Assertions section: {stdout}"
    );
    assert!(
        stdout.contains("velocity_in_range") && stdout.contains("PASS"),
        "expected velocity_in_range PASS: {stdout}"
    );
    assert!(
        stdout.contains("mass_approx") && stdout.contains("PASS"),
        "expected mass_approx PASS: {stdout}"
    );
    assert!(
        stdout.contains("velocity_approx") && stdout.contains("PASS"),
        "expected velocity_approx PASS: {stdout}"
    );
}

#[test]
fn eval_assertions_pass_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/assertions.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(
        json["assert"]["velocity_in_range"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["mass_approx"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["velocity_approx"]["status"].as_str(),
        Some("pass")
    );
}

#[test]
fn eval_assertions_fail_exit_code() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_fail.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("x_greater") && stderr.contains("FAIL"),
        "expected x_greater FAIL in stderr: {stderr}"
    );
}

#[test]
fn eval_assertions_tolerance_fail() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/assertions_tolerance_fail.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for tolerance failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("tight_check") && stderr.contains("FAIL"),
        "expected tight_check FAIL: {stderr}"
    );
    assert!(
        stderr.contains("off by"),
        "expected tolerance detail in message: {stderr}"
    );
}

#[test]
fn eval_assertions_assumes_affected_nodes() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_assumes.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for assumed assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("pressure_safe") && stderr.contains("FAIL"),
        "expected pressure_safe FAIL: {stderr}"
    );
    assert!(
        stderr.contains("affected") && stderr.contains("margin"),
        "expected affected: margin in output: {stderr}"
    );
}

#[test]
fn eval_assertions_indexed_fail() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for indexed assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok") && stderr.contains("FAIL"),
        "expected power_ok FAIL: {stderr}"
    );
    assert!(
        stderr.contains("Boost"),
        "expected Boost variant in failure message: {stderr}"
    );
    // Multi-index assertion: within_limits should fail with parenthesized paths
    assert!(
        stderr.contains("within_limits") && stderr.contains("FAIL"),
        "expected within_limits FAIL: {stderr}"
    );
    assert!(
        stderr.contains("(Mode.Normal, Phase.Cruise)"),
        "expected parenthesized multi-index path in failure message: {stderr}"
    );
}
#[test]
fn eval_assertions_compile_error_exit_code() {
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/assert_not_bool.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}
#[test]
fn eval_explicit_index_import() {
    // Bug 3: `import "./lib.gcl" { Color }` should import the Color index explicitly.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/explicit_index/src/lib/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("favorite") && l.contains("Red") && l.contains('1')),
        "expected favorite[Red] = 1 in output: {stdout}"
    );
}

// --- Variant comparison tests ---

#[test]
fn eval_variant_comparison() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/variant_comparison.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // selective[Departure] = 2*2460 = 4920 m/s (doubled)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective[Departure]") && l.contains("4920")),
        "expected selective[Departure] = 4920 in output: {stdout}"
    );
    // selective[Correction] = 120 m/s (unchanged)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective[Correction]") && l.contains("120")),
        "expected selective[Correction] = 120 in output: {stdout}"
    );

    // selective2[Insertion] = 3*1830 = 5490 m/s (tripled, variant on LHS)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective2[Insertion]") && l.contains("5490")),
        "expected selective2[Insertion] = 5490 in output: {stdout}"
    );

    // not_correction[Correction] = 0 m/s (zeroed via !=)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("not_correction[Correction]") && l.contains("0 m/s")),
        "expected not_correction[Correction] = 0 in output: {stdout}"
    );
}

// --- Variant match tests ---

#[test]
fn eval_variant_match() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/variant_match.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // scale_factor[Departure] = 2
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scale_factor[Departure]") && l.contains('2')),
        "expected scale_factor[Departure] = 2 in output: {stdout}"
    );
    // scaled_dv[Departure] = 2460 * 2 = 4920
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scaled_dv[Departure]") && l.contains("4920")),
        "expected scaled_dv[Departure] = 4920 in output: {stdout}"
    );
    // scaled_dv[Correction] = 120 * 0.5 = 60
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scaled_dv[Correction]") && l.contains("60")),
        "expected scaled_dv[Correction] = 60 in output: {stdout}"
    );

    // Multi-binding match: adjusted_cost is a 2D table.
    // Check the table header and key values.
    assert!(
        stdout.contains("adjusted_cost"),
        "expected adjusted_cost table in output: {stdout}"
    );
    // Departure row, Burn column = 2706
    assert!(
        stdout.contains("2706"),
        "expected 2706 (adjusted_cost[Departure][Burn]) in output: {stdout}"
    );
    // Departure row, Coast column = 0
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("Departure") && l.contains('0') && l.contains("2706")),
        "expected Departure row with 0 and 2706 in output: {stdout}"
    );
}

// --- Large / realistic fixture tests ---

#[test]
fn eval_power_budget() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/power_budget.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key computed nodes exist
    assert!(
        stdout.lines().any(|l| l.contains("peak_power")),
        "expected peak_power in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains("battery_dod")),
        "expected battery_dod in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains("sa_margin")),
        "expected sa_margin in output: {stdout}"
    );

    // Check assertions
    assert!(
        stdout.contains("sa_positive_margin") && stdout.contains("PASS"),
        "expected sa_positive_margin PASS: {stdout}"
    );
    assert!(
        stdout.contains("battery_dod_safe") && stdout.contains("PASS"),
        "expected battery_dod_safe PASS: {stdout}"
    );
}

#[test]
fn eval_multi_decl_sliced() {
    // Multi-decl v3: multi-axis shared prefix with slice sections.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_sliced.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    for phase in ["Launch", "Cruise", "Arrival"] {
        assert!(
            stdout.contains(phase),
            "expected {phase} in output: {stdout}",
        );
    }
    assert!(
        stdout.contains("total_active_power") && stdout.contains("peak_active_power"),
        "expected derived nodes in output: {stdout}",
    );
}

#[test]
fn eval_multi_decl_2d() {
    // Multi-decl v2: mixed 1-D and 2-D slots sharing one row axis.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_2d.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // The 2-D slot should render as a 2-D table with Safe / Nominal columns.
    assert!(
        stdout.contains("power_mode_active")
            && stdout.contains("Safe")
            && stdout.contains("Nominal"),
        "expected 2-D power_mode_active in output: {stdout}",
    );
    // Derived node that reads from both 1-D and 2-D slots.
    assert!(
        stdout.contains("total_safe_power"),
        "expected total_safe_power in output: {stdout}",
    );
}

#[test]
fn eval_multi_decl_1d() {
    // Multi-decl (issue #481) v1: homogeneous 1-D slots across
    // param/const-node kinds must evaluate end-to-end.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_1d.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Every slot in the multi-decl should appear as its own declaration.
    for name in ["power_consumption", "duty_cycle", "mass_per_unit"] {
        assert!(
            stdout.contains(name),
            "expected `{name}` in eval output: {stdout}",
        );
    }
    // Derived node reading cross-slot values.
    assert!(
        stdout.contains("peak_power"),
        "expected peak_power in output: {stdout}"
    );
}

#[test]
fn eval_power_budget_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/power_budget.gcl"),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // power_draw is a 2D indexed param
    let pd = &json["param"]["power_draw"];
    assert!(
        pd["entries"].is_object(),
        "expected power_draw entries: {pd}"
    );

    // peak_power is a scalar node
    assert!(
        json["node"]["peak_power"]["si_value"].as_f64().is_some(),
        "expected peak_power scalar value"
    );

    // assertions
    assert_eq!(
        json["assert"]["sa_positive_margin"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["battery_dod_safe"]["status"].as_str(),
        Some("pass")
    );
}

#[test]
fn eval_thermal_analysis() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/thermal_analysis.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key outputs
    assert!(
        stdout.lines().any(|l| l.contains("total_heater_power")),
        "expected total_heater_power in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("total_radiative_capacity")),
        "expected total_radiative_capacity in output: {stdout}"
    );

    // Check assertions
    assert!(
        stdout.contains("heater_budget_reasonable") && stdout.contains("PASS"),
        "expected heater_budget_reasonable PASS: {stdout}"
    );
    assert!(
        stdout.contains("has_radiative_capacity") && stdout.contains("PASS"),
        "expected has_radiative_capacity PASS: {stdout}"
    );
}

#[test]
fn eval_parenthesized_exprs() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/parenthesized_exprs.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key outputs
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("absorbed_power") && !l.contains("PASS")),
        "expected absorbed_power in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("voltage") && l.contains("50")),
        "expected voltage = 50 in output: {stdout}"
    );

    // All assertions should pass
    assert!(
        stdout.contains("absorbed_power_positive") && stdout.contains("PASS"),
        "expected absorbed_power_positive PASS: {stdout}"
    );
    assert!(
        stdout.contains("voltage_correct") && stdout.contains("PASS"),
        "expected voltage_correct PASS: {stdout}"
    );
    assert!(
        stdout.contains("charge_time_positive") && stdout.contains("PASS"),
        "expected charge_time_positive PASS: {stdout}"
    );
} // --- Expected-fail tests ---

#[test]
fn eval_expected_fail_pass() {
    // A failing assertion marked #[expected_fail] should invert to pass
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/expected_fail_pass.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected success for expected_fail on failing assertion, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("x_greater") && stdout.contains("PASS"),
        "expected x_greater PASS (inverted): {stdout}"
    );
}

#[test]
fn eval_expected_fail_unexpected_pass() {
    // A passing assertion marked #[expected_fail] should invert to fail
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_unexpected_pass.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for unexpected pass"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("x_less") && stderr.contains("FAIL"),
        "expected x_less FAIL (unexpected pass): {stderr}"
    );
    assert!(
        stderr.contains("expected_fail"),
        "expected mention of expected_fail in message: {stderr}"
    );
}

#[test]
fn eval_expected_fail_on_node_error() {
    // #[expected_fail] on a node should produce a compile error
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/expected_fail_on_node.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}

#[test]
fn eval_expected_fail_all_on_indexed_error() {
    // #[expected_fail] without arguments on an indexed assertion should produce a compile error
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/expected_fail_all_on_indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}

#[test]
fn eval_expected_fail_indexed_partial() {
    // Per-variant expected_fail should only suppress the specified variant;
    // other failing variants should still be reported.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_indexed_partial.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: Eco fails but is not expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok") && stderr.contains("FAIL") && stderr.contains("Mode.Eco"),
        "expected power_ok FAIL with Mode.Eco: {stderr}"
    );
}

#[test]
fn eval_expected_fail_indexed_unexpected_pass() {
    // Per-variant expected_fail where the expected-fail variant actually passes
    // should report "unexpected pass".
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_indexed_unexpected_pass.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: Boost passes but is marked expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok")
            && stderr.contains("FAIL")
            && stderr.contains("unexpected pass"),
        "expected power_ok FAIL with unexpected pass: {stderr}"
    );
}

#[test]
fn eval_expected_fail_multi_indexed_partial() {
    // Per-tuple-key expected_fail should only suppress specified tuple keys;
    // other failing keys should still be reported.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_multi_indexed_partial.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: (Eco, Cruise) fails but is not expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("within_limits") && stderr.contains("FAIL") && stderr.contains("Eco"),
        "expected within_limits FAIL with Eco: {stderr}"
    );
}

// --- Format command tests ---

#[test]
fn format_check_unformatted_exits_nonzero() {
    // Create a temp file with valid but unformatted graphcal
    let dir = std::env::temp_dir().join("graphcal_fmt_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unformatted.gcl");
    // Extra spaces and missing trailing newline
    std::fs::write(&path, "param   x  :  Dimensionless  =   1.0  ;").unwrap();

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for unformatted file"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("would be reformatted"),
        "expected 'would be reformatted' message: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_check_parse_error_fails() {
    // Files with parse errors are failures: CI must not pass on broken files.
    let dir = std::env::temp_dir().join("graphcal_fmt_test_err");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.gcl");
    std::fs::write(&path, "this is }{ not valid").unwrap();

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "expected failure on parse errors, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    // The formatter surfaces the specific parse-error message from the
    // underlying `FormatError::Parse` variant.
    assert!(
        stderr.contains("unexpected token") || stderr.contains("stray character"),
        "expected parse-error detail: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_in_place_then_check() {
    // Format a file in-place, then verify --check passes (idempotency via CLI)
    let dir = std::env::temp_dir().join("graphcal_fmt_test_inplace");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixme.gcl");
    std::fs::write(
        &path,
        "param   x:Dimensionless=1.0;\nparam y  : Dimensionless = 2.0 ;  \n",
    )
    .unwrap();

    // Format in-place
    let output = graphcal_bin()
        .args(["format", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "format in-place failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Now --check should pass
    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "expected --check to pass after formatting, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_check_recursive_directory() {
    // --check on a directory should recursively find .gcl files
    let output = graphcal_bin()
        .args(["format", "--check", &fixture("valid/multi/rocket_split")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected all multi/rocket_split files to be formatted, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn format_directory_skips_symlinked_gcl_files() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.gcl");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&tree).unwrap();

    let original = "param   x:Dimensionless=1.0;";
    std::fs::write(&outside, original).unwrap();
    std::fs::write(tree.join("inside.gcl"), "param x: Dimensionless = 1.0;\n").unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("link.gcl")).unwrap();

    let output = graphcal_bin()
        .args(["format", tree.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "BUG: directory formatter must skip symlinked .gcl files: format failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = std::fs::read_to_string(&outside).unwrap();
    assert_eq!(
        after, original,
        "BUG: directory formatter must skip symlinked .gcl files: symlink target outside the formatted tree was modified",
    );
}

#[test]
fn eval_datetime_basic() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_basic.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("launch"), "should contain launch");
    assert!(
        stdout.contains("2024-11-05T12:00:00 UTC"),
        "launch should be 2024-11-05T12:00:00 UTC"
    );
    assert!(
        stdout.contains("2024-11-05T13:00:00 UTC"),
        "one_hour_later should be 2024-11-05T13:00:00 UTC"
    );
    assert!(stdout.contains("3600"), "duration should be 3600 s");
    assert!(
        stdout.contains("2024-11-05T11:00:00 UTC"),
        "one_hour_before should be 2024-11-05T11:00:00 UTC"
    );
    assert!(stdout.contains("PASS"), "assertions should pass");
}

#[test]
fn eval_datetime_epoch() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_epoch.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.contains("2024-11-05T12:00:00 TT"),
        "t_tt should be in TT scale"
    );
    assert!(
        stdout.contains("2024-11-05T12:00:00 TAI"),
        "t_tai should be in TAI scale"
    );
    assert!(
        stdout.contains("2024-11-05T12:00:00 GPST"),
        "t_gpst should be in GPST scale"
    );
    assert!(
        stdout.contains("2024-11-05T13:00:00 TT"),
        "t_tt_later should be one hour later in TT"
    );
    assert!(stdout.contains("3600"), "tt_dur should be 3600 s");
    assert!(stdout.contains("PASS"), "assertions should pass");
}

#[test]
fn eval_datetime_scale_mismatch_error() {
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/datetime_scale_mismatch.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "cross-scale operation should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("time scale"),
        "error should mention dimension mismatch or time scale"
    );
}

#[test]
fn eval_datetime_conversion() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_conversion.gcl")])
        .output()
        .expect("failed to run graphcal");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "datetime conversion should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("t_utc"), "should output t_utc");
    assert!(stdout.contains("t_tai"), "should output t_tai");
    assert!(stdout.contains("t_tt_back"), "should output t_tt_back");
    assert!(stdout.contains("t_gpst"), "should output t_gpst");
    assert!(
        stdout.contains("roundtrip     PASS"),
        "roundtrip assert should pass"
    );
    assert!(
        stdout.contains("same_instant  PASS"),
        "same_instant assert should pass"
    );
}

#[test]
fn eval_datetime_conversion_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_conversion_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "to_utc on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn eval_datetime_timezone() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_timezone.gcl")])
        .output()
        .expect("failed to run graphcal");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "datetime timezone should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Timezone display produces IANA-zoned output
    assert!(
        stdout.contains("Asia/Tokyo"),
        "launch_tokyo should display in Asia/Tokyo timezone"
    );
    assert!(
        stdout.contains("America/New_York"),
        "launch_ny should display in America/New_York timezone"
    );

    // Two-arg constructor resolves to UTC
    assert!(
        stdout.contains("meeting_tokyo"),
        "should output meeting_tokyo"
    );

    // All assertions pass
    assert!(
        stdout.contains("same_instant               PASS"),
        "same_instant assert should pass"
    );
    assert!(
        stdout.contains("same_instant_ny            PASS"),
        "same_instant_ny assert should pass"
    );
    assert!(
        stdout.contains("display_preserves_instant  PASS"),
        "display_preserves_instant assert should pass"
    );
    assert!(
        stdout.contains("arith_works                PASS"),
        "arith_works assert should pass"
    );
}

#[test]
fn eval_datetime_timezone_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_timezone_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "timezone display on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn eval_datetime_extract() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_extract.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("y   = 2024"), "year should be 2024");
    assert!(stdout.contains("mo  = 11"), "month should be 11");
    assert!(stdout.contains("d   = 5"), "day should be 5");
    assert!(stdout.contains("h   = 14"), "hour should be 14");
    assert!(stdout.contains("mi  = 30"), "minute should be 30");
    assert!(stdout.contains("s   = 45"), "second should be 45");
    assert!(stdout.contains("wd  = 1"), "weekday should be 1 (Tuesday)");
    assert!(stdout.contains("doy = 310"), "day_of_year should be 310");
    assert!(!stdout.contains("FAIL"), "no assertions should fail");
}

#[test]
fn eval_datetime_extract_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_extract_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "extraction on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn eval_datetime_jd_unix() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_jd_unix.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.contains("unix_ts     = 1730808000"),
        "unix timestamp should be 1730808000"
    );
    assert!(!stdout.contains("FAIL"), "no assertions should fail");
}

// --- Instantiated import tests ---

#[test]
fn eval_instantiated_import_selective() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/instantiated_import/src/rocket/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // dry_mass=800, delta_v should be ~4719 m/s
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("result") && l.contains("4719")),
        "expected result ~4719 in output: {stdout}"
    );
}

#[test]
fn eval_instantiated_include_resolves_libraries_own_unbound_index() {
    // #851: an instantiated include of a library that declares and uses its
    // own index must resolve the inlined `for s: Step` / `Dimensionless[Step]`
    // / `Step.A` bodies without the consumer binding `Step`. Binding is an
    // override mechanism, not a requirement.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/include_uses_own_index/src/lib/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "instantiated include of a library's own index should evaluate: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let has = |needle: &str| {
        assert!(
            stdout.lines().any(|l| l.contains(needle)),
            "expected `{needle}` in output:\n{stdout}"
        );
    };
    // scale=10 applied to vals {A:1, B:2}; first = vals[A]*scale; total = sum.
    has("s.scaled[A] = 10");
    has("s.scaled[B] = 20");
    has("s.first     = 10");
    has("s.total     = 30");
}

// --- Partial overrides CLI tests ---

#[test]
fn eval_partial_set_uses_defaults() {
    // Partial --set falls back to defaults for the unset params.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=450.0 s"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "partial --set should fall back to defaults: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn eval_no_overrides_defaults_freely() {
    // No --set or --input at all → defaults used freely, no error
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "no overrides should use defaults freely: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- Plot tests (Vega-Lite JSON) ---

fn parse_plot_json_stdout(stdout: &str) -> serde_json::Value {
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap_or_else(|err| {
        panic!("expected --plot json stdout to be only JSON: {err}: {stdout}")
    });
    assert!(
        json.is_array(),
        "expected --plot json stdout to be a top-level array: {stdout}"
    );
    json
}

#[test]
fn eval_plot_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let _json = parse_plot_json_stdout(&stdout);
    // Vega-Lite specs: "mark": "line" and "mark": "bar"
    assert!(
        stdout.contains("\"mark\": \"line\""),
        "expected line mark in Vega-Lite JSON: {stdout}"
    );
    assert!(
        stdout.contains("\"mark\": \"bar\""),
        "expected bar mark in Vega-Lite JSON: {stdout}"
    );
    assert!(
        stdout.contains("vega-lite"),
        "expected Vega-Lite $schema: {stdout}"
    );
}

#[test]
fn eval_plot_json_suppresses_normal_eval_output_even_with_json_format() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/plot_basic.gcl"),
            "--format",
            "json",
            "--plot",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    assert!(
        json.get("param").is_none() && json.get("node").is_none(),
        "--plot json should print only the plot array, not eval JSON: {stdout}"
    );
}

#[test]
fn eval_plot_scatter_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_scatter.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"point\""),
        "expected point mark for scatter: {stdout}"
    );
}

#[test]
fn eval_plot_line_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_line.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"line\""),
        "expected line mark: {stdout}"
    );

    // Range-index data must plot as numeric values, not nominal "#N"
    // variant labels (#839).
    let json = parse_plot_json_stdout(&stdout);
    let spec = &json[0]["spec"];
    assert_eq!(
        spec["encoding"]["x"]["type"].as_str(),
        Some("quantitative"),
        "expected quantitative x for range-index data: {stdout}"
    );
    let values = spec["data"]["values"]
        .as_array()
        .expect("expected data values array");
    assert_eq!(values.len(), 5, "expected 5 time steps: {stdout}");
    assert_eq!(
        values[1]["x"].as_f64(),
        Some(0.5),
        "expected the second time step value 0.5 s: {stdout}"
    );
}

#[test]
fn eval_plot_bar_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_bar.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"bar\""),
        "expected bar mark: {stdout}"
    );
}

#[test]
fn eval_plot_heatmap_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_heatmap.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"rect\""),
        "expected rect mark for heatmap: {stdout}"
    );

    // The 2D color comprehension drives a Subsystem × OpMode cross product;
    // the 1D x/y channels broadcast across the axis they do not mention
    // (#840).
    let json = parse_plot_json_stdout(&stdout);
    let spec = &json[0]["spec"];
    let values = spec["data"]["values"]
        .as_array()
        .expect("expected data values array");
    assert_eq!(values.len(), 9, "expected 3×3 heat-map cells: {stdout}");
    assert_eq!(values[0]["x"].as_str(), Some("Safe"));
    assert_eq!(values[0]["y"].as_str(), Some("Comms"));
    assert_eq!(values[0]["color"].as_f64(), Some(2.0));
    assert_eq!(values[8]["x"].as_str(), Some("Science"));
    assert_eq!(values[8]["y"].as_str(), Some("Payload"));
    assert_eq!(values[8]["color"].as_f64(), Some(35.0));
    assert_eq!(
        spec["encoding"]["color"]["type"].as_str(),
        Some("quantitative"),
        "color carries the cell values: {stdout}"
    );
}

#[test]
fn eval_plot_mismatched_channel_axes_is_an_error() {
    // Channels over unrelated indexes have no meaningful row pairing; they
    // must fail loudly instead of zipping to the longest channel (#841).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mismatch.gcl");
    std::fs::write(
        &file,
        "pub index Step = { A, B, C, D };\n\
         pub index Pair = { L, R };\n\
         param values: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0, Step.C: 4.0, Step.D: 8.0 };\n\
         param twos: Dimensionless[Pair] = { Pair.L: 10.0, Pair.R: 20.0 };\n\
         pub plot mismatch = {\n\
             mark: line,\n\
             encode: {\n\
                 x: for s: Step { @values[s] },\n\
                 y: for p: Pair { @twos[p] },\n\
             },\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", file.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "mismatched channel axes must fail the run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plot `mismatch` not rendered")
            && stderr.contains("incompatible index axes"),
        "expected an axes mismatch report: {stderr}"
    );
    // Stdout keeps the JSON contract: an empty array, no misaligned rows.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    assert_eq!(json.as_array().map(Vec::len), Some(0));
}

#[test]
fn eval_plot_indexed_bools_encode_like_scalar_bools() {
    // Indexed Bool entries must encode as "true"/"false" labels, never as
    // index variant names (#840).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("flags.gcl");
    std::fs::write(
        &file,
        "pub index Step = { A, B, C };\n\
         param values: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0, Step.C: 3.0 };\n\
         node flags: Bool[Step] = for s: Step { @values[s] > 1.5 };\n\
         pub plot p = {\n\
             mark: point,\n\
             encode: { x: for s: Step { @values[s] }, y: for s: Step { @flags[s] } },\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", file.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    let values = json[0]["spec"]["data"]["values"]
        .as_array()
        .expect("expected data values array");
    let ys: Vec<&str> = values.iter().map(|v| v["y"].as_str().unwrap()).collect();
    assert_eq!(
        ys,
        ["false", "true", "true"],
        "expected bool labels: {stdout}"
    );
}

#[test]
fn eval_plot_negative_width_is_an_error() {
    // Sizes must be strictly positive; -500 was previously passed straight
    // through to the Vega-Lite spec (#845).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("neg.gcl");
    std::fs::write(
        &file,
        "pub index Step = { A, B };\n\
         param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };\n\
         pub plot p = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @vals[s] }, y: for s: Step { @vals[s] } },\n\
             width: -500.0,\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", file.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success(), "negative width must fail the run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`width` must be a positive number"),
        "expected positivity error: {stderr}"
    );
}

#[test]
fn check_rejects_typoed_plot_property() {
    // Mistyped property names must fail `graphcal check`, not vanish (#845).
    // `caption` is deliberately not a misspelling of a real property so the
    // typos pre-commit hook leaves it alone.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("typo.gcl");
    std::fs::write(
        &file,
        "pub index Step = { A, B };\n\
         param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };\n\
         pub plot p = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @vals[s] } },\n\
             caption: \"typo\",\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["check", file.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "typo'd property must fail graphcal check"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("caption"),
        "expected the typo'd name in the diagnostic: {stderr}"
    );
}

#[test]
fn eval_plot_no_plots_warns() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "[]",
        "expected --plot json stdout to remain valid JSON when no plots exist: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no plot declarations found"),
        "expected warning about no plots: {stderr}"
    );
}

// --- Figure tests ---

#[test]
fn eval_figure_basic_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/figure_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    // 3 figures: curve_a (standalone), curve_b (standalone), comparison (figure)
    assert_eq!(
        arr.len(),
        3,
        "expected 3 figures (2 standalone + 1 combined): {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("curve_a"));
    assert_eq!(arr[1]["name"].as_str(), Some("curve_b"));
    assert_eq!(arr[2]["name"].as_str(), Some("comparison"));

    // Standalone curve_a should have a line mark
    let curve_a_spec = &arr[0]["spec"];
    assert_eq!(
        curve_a_spec["mark"].as_str(),
        Some("line"),
        "expected line mark for curve_a: {curve_a_spec}"
    );

    // Standalone curve_b should have a bar mark
    let bar_spec = &arr[1]["spec"];
    assert_eq!(
        bar_spec["mark"].as_str(),
        Some("bar"),
        "expected bar mark for curve_b: {bar_spec}"
    );

    // Comparison figure should use hconcat with 2 sub-specs
    let comparison_hconcat = arr[2]["spec"]["hconcat"]
        .as_array()
        .expect("expected hconcat array in comparison");
    assert_eq!(
        comparison_hconcat.len(),
        2,
        "expected 2 sub-specs in comparison hconcat"
    );
}

#[test]
fn eval_figure_hidden_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/figure_hidden.gcl"),
            "--plot",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    // Only 1 figure: comparison (hidden plots suppress standalone output)
    assert_eq!(
        arr.len(),
        1,
        "expected 1 figure (hidden plots suppressed): {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("comparison"));

    // The comparison figure should still contain both sub-specs via hconcat
    let comparison_hconcat = arr[0]["spec"]["hconcat"]
        .as_array()
        .expect("expected hconcat array in comparison");
    assert_eq!(
        comparison_hconcat.len(),
        2,
        "expected 2 sub-specs in comparison hconcat even though plots are hidden"
    );
}

#[test]
fn eval_plot_basic_standalone_figures() {
    // plot_basic.gcl has 2 plots, no figures — should produce 2 standalone figures
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    assert_eq!(
        arr.len(),
        2,
        "expected 2 standalone figures from plot_basic.gcl: {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("my_line"));
    assert_eq!(arr[1]["name"].as_str(), Some("my_bar"));
}

#[test]
fn eval_plot_datetime_is_temporal_iso8601() {
    // Datetime values must plot as ISO 8601 strings with temporal encoding,
    // not nominal hifitime display strings (#846).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dt.gcl");
    std::fs::write(
        &file,
        "node t0: Datetime = datetime(\"2026-01-01T00:00:00Z\");\n\
         pub plot dt = {\n\
             mark: point,\n\
             encode: { x: @t0, y: 1.0 },\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", file.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    let spec = &json[0]["spec"];
    assert_eq!(
        spec["encoding"]["x"]["type"].as_str(),
        Some("temporal"),
        "expected temporal x encoding for Datetime: {stdout}"
    );
    assert_eq!(
        spec["data"]["values"][0]["x"].as_str(),
        Some("2026-01-01T00:00:00Z"),
        "expected RFC 3339 datetime string: {stdout}"
    );
}

#[test]
fn eval_plot_failure_reported_on_stderr() {
    // A plot skipped because a plotted node failed must be reported with the
    // plot name and root cause, not hidden behind "no plot declarations
    // found" (#842).
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("div.gcl");
    std::fs::write(
        &file,
        "pub index Step = { A, B, C };\n\
         param values: Dimensionless[Step] = { Step.A: 1.0, Step.B: 0.0, Step.C: 4.0 };\n\
         node inv: Dimensionless[Step] = for s: Step { 1.0 / @values[s] };\n\
         pub plot p = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @values[s] }, y: for s: Step { @inv[s] } },\n\
         };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", file.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    // The node error still drives the exit code.
    assert!(!output.status.success(), "expected exit code 1");
    // Stdout keeps the JSON contract: a single valid (empty) array.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    assert_eq!(json.as_array().map(Vec::len), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plot `p` not rendered"),
        "expected skipped-plot report naming the plot: {stderr}"
    );
    assert!(
        stderr.contains("inv"),
        "expected the failed dependency to be named: {stderr}"
    );
    assert!(
        stderr.contains("division by zero"),
        "expected the root cause in the report: {stderr}"
    );
    assert!(
        !stderr.contains("no plot declarations found"),
        "a plot declaration exists; the no-plots warning is misleading: {stderr}"
    );
}

#[test]
fn eval_plot_html_file_output() {
    // --plot <path>.html writes the self-contained HTML page to that path (#848)
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("plots.html");

    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/plot_basic.gcl"),
            "--plot",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(&out_path).expect("expected HTML file to be written");
    assert!(
        html.contains("vegaEmbed"),
        "expected Vega-Embed HTML page: {html}"
    );
    // Normal evaluation output still goes to stdout in HTML file mode.
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.trim().is_empty(),
        "expected normal eval output on stdout in HTML file mode"
    );
}

#[test]
fn eval_plot_rejects_non_html_path() {
    // Anything that is not browser/json/*.html is a CLI argument error (#848)
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/plot_basic.gcl"),
            "--plot",
            "out.png",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success(), "expected argument parse failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected `browser`, `json`, or a path ending in `.html`"),
        "expected helpful --plot error message, got: {stderr}"
    );
}

// --- Unit definitions referencing imported units (#822) ---

#[test]
fn eval_unit_def_from_module_import() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/unit_def_from_import/src/app/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // halfmile = 0.5 u.mile, so 1609.344 m converts to 2 halfmile.
    assert!(
        stdout
            .lines()
            .any(|l| l.contains('b') && l.contains("2 halfmile")),
        "expected `b = 2 halfmile` in output: {stdout}"
    );
}

#[test]
fn eval_unit_def_from_selective_import() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/unit_def_from_import_selective/src/app/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains('b') && l.contains("2 halfmile")),
        "expected `b = 2 halfmile` in output: {stdout}"
    );
}

// --- Dynamic units across a module-import boundary (#823) ---

#[test]
fn eval_dynamic_unit_via_module_import() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/dynamic_unit_import/src/app/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // 1 EUR = 1.08 USD, so 100 USD converts to ~92.59 fx.EUR.
    assert!(
        stdout
            .lines()
            .any(|l| l.contains('p') && l.contains("92.59") && l.contains("fx.EUR")),
        "expected `p = 92.59... fx.EUR` in output: {stdout}"
    );
}

#[test]
fn eval_dynamic_unit_via_selective_import() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/dynamic_unit_import_selective/src/app/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains('p') && l.contains("92.59") && l.contains("EUR")),
        "expected `p = 92.59... EUR` in output: {stdout}"
    );
}

// --- Dynamic units ---

#[test]
fn eval_dynamic_units() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/dynamic_units.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // price_eur = 100 EUR (100 * 1.08 = 108 USD in SI)
    assert!(stdout.contains("price_eur"), "missing price_eur");
    assert!(stdout.contains("EUR"), "missing EUR unit");

    // price_usd = 108 USD
    assert!(stdout.contains("price_usd"), "missing price_usd");
    assert!(stdout.contains("108"), "expected 108 USD");

    // total = 158 USD (108 + 50)
    assert!(stdout.contains("total"), "missing total");
    assert!(stdout.contains("158"), "expected 158 USD");
}

#[test]
fn eval_dynamic_units_with_override() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/dynamic_units.gcl"),
            "--set",
            "usd_per_eur=1.20",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // With usd_per_eur=1.20: price_eur = 100 * 1.20 = 120 USD
    assert!(stdout.contains("120"), "expected 120 USD for price_usd");

    // total = 120 + 50 = 170 USD
    assert!(stdout.contains("170"), "expected 170 USD for total");
}

// ---------------------------------------------------------------------------
// Invariant: any fixture that fails `check` must also fail `eval`.
// `eval` runs the static check pipeline as its first stage, so this is
// currently maintained only by call-order in the CLI. If a future refactor
// lets `eval` skip part of the check pipeline, this test surfaces the
// regression rather than letting check-level diagnostics be silently
// bypassed.
// ---------------------------------------------------------------------------

fn collect_entry_points(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut local_gcls: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut has_main = false;
    for entry in std::fs::read_dir(dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() && path.extension().is_some_and(|e| e == "gcl") {
            if path.file_name().is_some_and(|n| n == "main.gcl") {
                has_main = true;
            }
            local_gcls.push(path);
        } else {
            // skip non-gcl files (e.g. graphcal.toml, *.json fixtures)
        }
    }
    if has_main {
        out.extend(
            local_gcls
                .into_iter()
                .filter(|p| p.file_name().is_some_and(|n| n == "main.gcl")),
        );
    } else {
        out.extend(local_gcls);
    }
    subdirs.sort();
    for d in subdirs {
        collect_entry_points(&d, out);
    }
}

fn fixture_entry_points() -> Vec<PathBuf> {
    fixture_entry_points_by_category()
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// Collect `(category, entry_path)` pairs for every fixture entry point under
/// `tests/fixtures/{valid,valid_library,runtime_error,invalid}`, sorted by
/// path.
fn fixture_entry_points_by_category() -> Vec<(&'static str, PathBuf)> {
    let root = fixtures_root();
    let mut entries: Vec<(&'static str, PathBuf)> = Vec::new();
    for cat in ["valid", "valid_library", "runtime_error", "invalid"] {
        let mut cat_entries = Vec::new();
        collect_entry_points(&root.join(cat), &mut cat_entries);
        for path in cat_entries {
            entries.push((cat, path));
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries
}

#[test]
fn check_failure_implies_eval_failure() {
    let entries = fixture_entry_points();
    assert!(
        entries.len() >= 100,
        "found only {} entry points",
        entries.len()
    );

    let root = fixtures_root();
    let mut violations: Vec<String> = Vec::new();
    for path in &entries {
        let check = graphcal_bin()
            .args(["check", path.to_str().unwrap()])
            .output()
            .expect("graphcal check failed to spawn");
        if check.status.success() {
            continue;
        }
        let eval = graphcal_bin()
            .args(["eval", path.to_str().unwrap()])
            .output()
            .expect("graphcal eval failed to spawn");
        if eval.status.success() {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            violations.push(format!(
                "{}: check exit={:?}, eval exit={:?}",
                rel.display(),
                check.status.code(),
                eval.status.code()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "{} fixture(s) failed `check` but passed `eval` — invariant violated:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Categorization: each fixture must live under the directory whose name
// matches its actual `check`/`eval` outcome:
//   tests/fixtures/valid/         → check passes, eval passes
//   tests/fixtures/valid_library/ → check passes (eval may pass or fail —
//                                   library files aren't designed to be
//                                   evaluated standalone, so eval result is
//                                   not load-bearing for this category)
//   tests/fixtures/runtime_error/ → check passes, eval fails
//   tests/fixtures/invalid/       → check fails
// Without this guard, fixtures can drift into the wrong bucket as language
// semantics change, silently weakening the implicit contract that snapshot
// and integration tests rely on.
// ---------------------------------------------------------------------------

/// Fixtures that are placed in the wrong category but kept there because
/// the source-of-truth intent matters more than the current `check`/`eval`
/// outcome. Each entry MUST carry a tracking issue so the allowlist shrinks
/// over time.
///
/// Format: `(relative_path, expected_category, actual_category, reason)`.
const KNOWN_MISCLASSIFIED: &[(&str, &str, &str, &str)] = &[];

/// Outcomes that a fixture's directory placement can accept. `valid_library`
/// is intentionally lenient on `eval` — see the comment block above.
fn category_accepts(expected: &str, actual: &str) -> bool {
    match expected {
        "valid_library" => actual == "valid" || actual == "runtime_error",
        _ => expected == actual,
    }
}

#[test]
fn fixtures_match_their_category() {
    let entries = fixture_entry_points_by_category();
    assert!(
        entries.len() >= 100,
        "found only {} entry points",
        entries.len()
    );

    let root = fixtures_root();
    let mut new_violations: Vec<String> = Vec::new();
    let mut stale_allowlist: Vec<String> = Vec::new();
    for (expected, path) in &entries {
        let check = graphcal_bin()
            .args(["check", path.to_str().unwrap()])
            .output()
            .expect("graphcal check failed to spawn");
        let actual = if check.status.success() {
            let eval = graphcal_bin()
                .args(["eval", path.to_str().unwrap()])
                .output()
                .expect("graphcal eval failed to spawn");
            if eval.status.success() {
                "valid"
            } else {
                "runtime_error"
            }
        } else {
            "invalid"
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let allowlisted = KNOWN_MISCLASSIFIED
            .iter()
            .find(|(p, _, _, _)| *p == rel_str);
        let accepted = category_accepts(expected, actual);
        match (accepted, allowlisted) {
            (true, None) => {}
            (true, Some((_, _, _, reason))) => {
                stale_allowlist.push(format!("{rel_str}: now categorized correctly ({reason})"));
            }
            (false, Some((_, exp, act, _))) if expected == exp && actual == *act => {}
            (false, Some((_, exp, act, _))) => {
                new_violations.push(format!(
                    "{rel_str}: allowlist says expected `{exp}` actual `{act}`, \
                     but observed expected `{expected}` actual `{actual}`"
                ));
            }
            (false, None) => {
                new_violations.push(format!(
                    "{rel_str}: expected `{expected}`, actual `{actual}`"
                ));
            }
        }
    }

    assert!(
        new_violations.is_empty(),
        "{} fixture(s) misclassified — move each to the matching directory, \
         fix the underlying regression, or add to KNOWN_MISCLASSIFIED with a \
         tracking note:\n{}",
        new_violations.len(),
        new_violations.join("\n")
    );
    assert!(
        stale_allowlist.is_empty(),
        "{} fixture(s) on KNOWN_MISCLASSIFIED now match their category — \
         remove them from the allowlist:\n{}",
        stale_allowlist.len(),
        stale_allowlist.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Eval-idempotence: `graphcal format` must preserve `graphcal eval` output
// for every `valid/` fixture.
//
// The existing `idempotent_*` macros only check `format(format(x)) ==
// format(x)`; they happily accept a formatter that consistently changes
// semantics (issue #575 was exactly that). This stronger property test
// guards against any future paren-elision regression.
// ---------------------------------------------------------------------------

/// Walk up from `entry` until we hit either a directory containing
/// `graphcal.toml` (the package root for multi-file fixtures) or the
/// `valid/` parent (single-file fixture). Returns `(root_path,
/// entry_relative_to_root)` so the caller can mirror the structure into
/// a temp dir and re-run `graphcal eval` against the same logical entry.
fn fixture_format_scope(entry: &Path) -> (PathBuf, PathBuf) {
    let valid_root = fixtures_root().join("valid");
    let mut dir = entry.parent().expect("entry has parent").to_path_buf();
    while dir != valid_root {
        if dir.join("graphcal.toml").exists() {
            let rel = entry.strip_prefix(&dir).unwrap().to_path_buf();
            return (dir, rel);
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        if parent == valid_root {
            // Single-file or single-dir fixture sitting directly under valid/.
            let rel = entry.strip_prefix(&dir).unwrap().to_path_buf();
            return (dir, rel);
        }
        dir = parent.to_path_buf();
    }
    // Single file directly under valid/.
    (
        entry.to_path_buf(),
        PathBuf::from(entry.file_name().unwrap()),
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let target = dst.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy file");
        }
    }
}

#[test]
fn eval_idempotent_under_format() {
    let entries = fixture_entry_points_by_category();
    let valid_entries: Vec<&PathBuf> = entries
        .iter()
        .filter_map(|(cat, p)| (*cat == "valid").then_some(p))
        .collect();
    assert!(
        valid_entries.len() >= 40,
        "expected many valid entry points, got {}",
        valid_entries.len()
    );

    let temp_root = std::env::temp_dir().join("graphcal_eval_idempotent");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&temp_root).expect("create temp root");

    let mut failures: Vec<String> = Vec::new();
    for (idx, entry) in valid_entries.iter().enumerate() {
        let original = graphcal_bin()
            .args(["eval", entry.to_str().unwrap()])
            .output()
            .expect("eval original");
        if !original.status.success() {
            // `valid` fixtures should eval cleanly; if not, the
            // categorization test catches it. Skip here so this property
            // test stays focused on its own invariant.
            continue;
        }

        let (scope_root, entry_rel) = fixture_format_scope(entry);
        let target_root = temp_root.join(format!("scope_{idx}"));
        let _ = std::fs::remove_dir_all(&target_root);
        if scope_root.is_file() {
            std::fs::create_dir_all(&target_root).expect("create scope dir");
            let target_file = target_root.join(scope_root.file_name().unwrap());
            std::fs::copy(&scope_root, &target_file).expect("copy single-file scope");
        } else {
            copy_dir_recursive(&scope_root, &target_root);
        }

        let format_target = if scope_root.is_file() {
            target_root.join(scope_root.file_name().unwrap())
        } else {
            target_root.clone()
        };
        let format_out = graphcal_bin()
            .args(["format", format_target.to_str().unwrap()])
            .output()
            .expect("graphcal format");
        if !format_out.status.success() {
            failures.push(format!(
                "{}: format failed: {}",
                entry.display(),
                String::from_utf8_lossy(&format_out.stderr)
            ));
            continue;
        }

        let new_entry = if scope_root.is_file() {
            target_root.join(scope_root.file_name().unwrap())
        } else {
            target_root.join(&entry_rel)
        };
        let after = graphcal_bin()
            .args(["eval", new_entry.to_str().unwrap()])
            .output()
            .expect("eval formatted");
        if !after.status.success() {
            failures.push(format!(
                "{}: eval-after-format failed: {}",
                entry.display(),
                String::from_utf8_lossy(&after.stderr)
            ));
            continue;
        }

        if original.stdout != after.stdout {
            failures.push(format!(
                "{}: eval output diverged after format",
                entry.display()
            ));
        }
    }

    let _ = std::fs::remove_dir_all(&temp_root);
    assert!(
        failures.is_empty(),
        "{} fixture(s) had eval output that differs after `graphcal format` \
         — formatter is not eval-idempotent:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Invariant: every fixture we intend to be well-formed is already canonically
// formatted.
//
// The per-fixture `format_check_*` tests above only cover a hand-maintained
// list, so a newly added fixture in unformatted shape slips through. This test
// auto-discovers the `valid`/`valid_library`/`runtime_error` trees using the
// same `format::collect_gcl_files` walk and `format::format_status` decision
// the CLI uses, so there is nothing to keep in sync — and no process to spawn.
//
// `invalid/` is intentionally excluded: those fixtures exist to be rejected,
// and keeping them canonically formatted is not an invariant we want (some
// don't even parse). A `FormatStatus::Error` from this set would therefore be
// a fixture in the wrong directory, so we treat it as a violation too.
// ---------------------------------------------------------------------------

#[test]
fn well_formed_fixtures_are_formatted() {
    use graphcal::format::{FormatStatus, collect_gcl_files, format_status};

    let root = fixtures_root();
    let mut files = Vec::new();
    for category in ["valid", "valid_library", "runtime_error"] {
        let (mut found, _warnings) = collect_gcl_files(&root.join(category));
        files.append(&mut found);
    }
    assert!(
        files.len() >= 100,
        "expected to discover the well-formed fixture tree, found only {} files",
        files.len()
    );

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read fixture");
        let rel = file.strip_prefix(&root).unwrap_or(file).display();
        match format_status(&source) {
            FormatStatus::Unchanged => {}
            FormatStatus::Changed(_) => {
                violations.push(format!("{rel}: not canonically formatted"));
            }
            FormatStatus::Error(e) => {
                violations.push(format!("{rel}: does not parse ({e}) — belongs in invalid/"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{} well-formed fixture(s) failed the formatting invariant — run \
         `graphcal format tests/fixtures`:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[test]
fn format_check_fails_on_unparsable_file() {
    // Regression: `graphcal format --check` exited 0 when a file failed to
    // parse — CI passed silently on syntactically broken files while
    // `graphcal check` on the same file failed.
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "broken.gcl", "node x: = ;\n");

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "format --check must fail on a parse error"
    );

    // Plain `format` must also report failure.
    let output = graphcal_bin()
        .args(["format", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "format must fail on a parse error"
    );
}

#[test]
fn graph_rocket_dot_output() {
    let output = graphcal_bin()
        .args(["graph", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());

    // The experimental notice goes to stderr; stdout must stay a clean pipe
    // into `dot`.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("`graphcal graph` is experimental"),
        "expected experimental warning on stderr: {stderr}"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        r#"digraph graphcal {
    rankdir=LR;
    node [fontname="Helvetica,Arial,sans-serif"];
    "rocket.dry_mass" [label="dry_mass\nMass", shape=ellipse];
    "rocket.fuel_mass" [label="fuel_mass\nMass", shape=ellipse];
    "rocket.isp" [label="isp\nTime", shape=ellipse];
    "rocket.g0" [label="g0\nAcceleration", shape=box, style=rounded];
    "rocket.v_exhaust" [label="v_exhaust\nVelocity", shape=box];
    "rocket.mass_ratio" [label="mass_ratio\nDimensionless", shape=box];
    "rocket.delta_v" [label="delta_v\nVelocity", shape=box];
    "rocket.dry_mass" -> "rocket.mass_ratio";
    "rocket.fuel_mass" -> "rocket.mass_ratio";
    "rocket.g0" -> "rocket.v_exhaust";
    "rocket.isp" -> "rocket.v_exhaust";
    "rocket.mass_ratio" -> "rocket.delta_v";
    "rocket.v_exhaust" -> "rocket.delta_v";
}
"#
    );
}

#[test]
fn graph_explicit_dot_format_matches_default() {
    let default_out = graphcal_bin()
        .args(["graph", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");
    let dot_out = graphcal_bin()
        .args(["graph", &fixture("valid/rocket.gcl"), "--format", "dot"])
        .output()
        .expect("failed to run graphcal");

    assert!(dot_out.status.success());
    assert_eq!(default_out.stdout, dot_out.stdout);
}

#[test]
fn graph_inline_dag_renders_cluster() {
    let output = graphcal_bin()
        .args(["graph", &fixture("valid/inline_dag_call_basic/main.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("subgraph \"cluster_main.scale\" {"),
        "inline dag block should render as a cluster:\n{stdout}"
    );
    assert!(
        stdout.contains("label=\"dag scale\";"),
        "cluster should carry the dag's name as its label:\n{stdout}"
    );
    assert!(
        stdout.contains("\"main.scale.v\" -> \"main.scale.result\";"),
        "cluster-internal dataflow should be present:\n{stdout}"
    );
}

#[test]
fn graph_compile_error_exits_2() {
    let output = graphcal_bin()
        .args(["graph", &fixture("invalid/const_cycle.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn graph_imported_values_render_as_external_nodes() {
    let output = graphcal_bin()
        .args([
            "graph",
            &fixture("valid/multi/rocket_split/src/lib/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "\"src.lib.constants.g0\" [label=\"src.lib.constants.g0\", shape=box, style=dashed];"
        ),
        "imported value should render as a dashed external node:\n{stdout}"
    );
    assert!(
        stdout.contains("\"src.lib.constants.g0\" -> \"src.lib.main.v_exhaust\";"),
        "cross-file dataflow edge should be present:\n{stdout}"
    );
}

// --- Cross-file plots via include brace lists (#847) ---

/// Build a two-file project: a library with a pub plot and a main file with
/// the given contents. Returns (tempdir, main path).
fn plot_include_project(main_source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/pkg");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub index Step = { A, B };\n\
         param vals: Dimensionless[Step] = { Step.A: 1.0, Step.B: 2.0 };\n\
         pub plot lib_plot = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @vals[s] }, y: for s: Step { @vals[s] } },\n\
         };\n",
    )
    .unwrap();
    let main = root_dir.join("main.gcl");
    std::fs::write(&main, main_source).unwrap();
    (dir, main)
}

fn plot_names(main: &Path) -> (Vec<String>, std::process::Output) {
    let output = graphcal_bin()
        .args(["eval", main.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
    let names = json
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| f["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    (names, output)
}

#[test]
fn include_brace_plot_renders_under_alias() {
    let (_dir, main) = plot_include_project(
        "include pkg.lib().{ lib_plot as lp };\n\
         param own: Dimensionless = 5.0;\n",
    );
    let (names, output) = plot_names(&main);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(names, ["lp"], "included plot renders under its alias");
}

#[test]
fn unrequested_library_plots_do_not_render() {
    let (_dir, main) = plot_include_project(
        "include pkg.lib().{ vals };\n\
         param own: Dimensionless = 5.0;\n",
    );
    let (names, output) = plot_names(&main);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        names.is_empty(),
        "library plots not named in the brace list must not render: {names:?}"
    );
}

#[test]
fn hidden_include_item_composes_without_standalone_output() {
    let (_dir, main) = plot_include_project(
        "include pkg.lib().{ #[hidden] lib_plot as lp };\n\
         figure combo = { plots: [lp] };\n\
         param own: Dimensionless = 5.0;\n",
    );
    let (names, output) = plot_names(&main);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        names,
        ["combo"],
        "#[hidden] include item must compose without standalone output"
    );
}

#[test]
fn import_of_plot_is_an_error() {
    let (_dir, main) = plot_include_project(
        "import pkg.lib.{ lib_plot };\n\
         param own: Dimensionless = 5.0;\n",
    );
    let output = graphcal_bin()
        .args(["check", main.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(!output.status.success(), "import of a plot must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot `import` plot"),
        "expected the structured import-plot error: {stderr}"
    );
}

#[test]
fn instantiated_include_plot_evaluates_against_instance() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/pkg");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("eng.gcl"),
        "param scale: Dimensionless = 1.0;\n\
         pub node doubled: Dimensionless = @scale * 2.0;\n\
         pub plot scaled_plot = { mark: point, encode: { x: @scale, y: @doubled } };\n",
    )
    .unwrap();
    let main = root_dir.join("main.gcl");
    std::fs::write(
        &main,
        "include pkg.eng(scale: 10.0).{ scaled_plot as sp };\n\
         param own: Dimensionless = 5.0;\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", main.to_str().unwrap(), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    assert_eq!(json[0]["name"].as_str(), Some("sp"));
    assert_eq!(
        json[0]["spec"]["data"]["values"][0]["x"].as_f64(),
        Some(10.0),
        "included plot must evaluate against the instance's bindings: {stdout}"
    );
    assert_eq!(
        json[0]["spec"]["data"]["values"][0]["y"].as_f64(),
        Some(20.0)
    );
}

#[test]
fn hidden_on_non_plot_include_item_is_an_error() {
    let (_dir, main) = plot_include_project(
        "include pkg.lib().{ #[hidden] vals };\n\
         param own: Dimensionless = 5.0;\n",
    );
    let output = graphcal_bin()
        .args(["check", main.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("attribute `hidden` does not apply to include item"),
        "expected A018: {stderr}"
    );
}
