#![cfg(test)]

use std::process::{Command, Output};

fn run(source: &str, args: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.gcl");
    std::fs::write(&path, source).unwrap();
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
        .args(args)
        .arg(path)
        .output()
        .unwrap()
}

const MODEL: &str = "node unfinished: Length = todo<Length> {}; node blocked: Length = @unfinished; node known: Length = 1.0 m;";

#[test]
fn check_accepts_incomplete_source_but_deny_todo_rejects_it_without_evaluation() {
    let source = format!("{MODEL} node bad: Length = 1.0 m / 0.0;");
    let output = run(&source, &["check"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 unfinished declaration"));
    assert!(!run(&source, &["check", "--deny-todo"]).status.success());
    assert!(
        !run(
            "dag unused { node x: Length = todo<Length> {}; }",
            &["check", "--deny-todo"]
        )
        .status
        .success()
    );
}

#[test]
fn eval_prints_partial_results_and_requires_explicit_permission_for_success() {
    let output = run(MODEL, &["eval"]);
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("TODO"));
    assert!(text.contains("BLOCKED"));
    assert!(text.contains("known"));
    assert!(!text.contains("ERROR"));
    assert!(run(MODEL, &["eval", "--allow-incomplete"]).status.success());
}

#[test]
fn json_distinguishes_incompleteness_without_fabricating_values() {
    let output = run(MODEL, &["eval", "--allow-incomplete", "--format", "json"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["incomplete"], true);
    assert_eq!(json["node"]["unfinished"]["status"], "todo");
    assert_eq!(json["node"]["blocked"]["status"], "blocked");
    assert!(json["node"]["unfinished"].get("si").is_none());
    assert!(json["node"]["unfinished"].get("error").is_none());
    assert!(json["node"]["known"].get("status").is_none());
}

#[test]
fn allowing_incomplete_does_not_hide_private_call_failures() {
    let source = "dag model { node missing: Length = todo<Length> {}; node bad: Length = 1.0 m / 0.0; pub node known: Length = 1.0 m; } node known: Length = @model()::known;";
    let output = run(source, &["eval", "--allow-incomplete"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ERROR"));
}

#[test]
fn allowing_incomplete_never_suppresses_real_failures() {
    for extra in ["node bad: Length = 1.0 m / 0.0;", "assert bad = false;"] {
        let output = run(&format!("{MODEL} {extra}"), &["eval", "--allow-incomplete"]);
        assert!(!output.status.success());
    }
}

/// Unavailability reasons name declarations as the output does, never by
/// their internal identities (`input.unfinished`, `input.inner.pending`).
#[test]
fn unavailability_reasons_name_declarations_as_the_output_does() {
    let source = "dag inner { node pending: Length = todo<Length> {}; pub node out: Length = @pending; } \
                  include inner() as inst; \
                  node unfinished: Length = todo<Length> {}; \
                  node blocked: Length = @unfinished + @inst::out; \
                  node bad: Length = 1.0 m / 0.0; \
                  node failed: Length = @bad + @inst::out;";
    let output = run(source, &["eval", "--allow-incomplete"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("BLOCKED — unfinished dependencies: unfinished, inst::pending"),
        "{text}"
    );
    assert!(!text.contains("model.inner"), "{text}");
    assert!(!text.contains("model.unfinished"), "{text}");

    let output = run(source, &["eval", "--allow-incomplete", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["node"]["unfinished"]["declaration"], "unfinished");
    assert_eq!(
        json["node"]["blocked"]["unfinished"],
        serde_json::json!(["unfinished", "inst::pending"])
    );
    assert_eq!(
        json["node"]["inst::out"]["unfinished"],
        serde_json::json!(["inst::pending"])
    );
    assert_eq!(
        json["node"]["failed"]["failed_deps"],
        serde_json::json!(["bad"])
    );
}

/// A package whose `main` uses `lib`'s constant but none of the unfinished
/// declarations of `lib` and `parts`; each other root file reaches exactly
/// one unfinished declaration of another module.
fn package_with_unused_imported_todos() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let sources = directory.path().join("src/demo");
    std::fs::create_dir_all(&sources).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\nsource_dir = \"src\"\n",
    )
    .unwrap();
    for (file, source) in [
        (
            "lib.gcl",
            "pub const node g0: Acceleration = 9.80665 m/s^2;\nnode unused: Length = todo<Length> {};\n",
        ),
        (
            "parts.gcl",
            "pub node missing: Length = todo<Length> {};\npub node known: Length = 2.0 m;\n",
        ),
        (
            "main.gcl",
            "import demo.lib::{g0};\nimport demo.consts as c;\ninclude demo.parts()::{ known };\nnode w: Force = 2.0 kg * @g0;\nnode x: Force = c::g0 * 2.0 kg;\n",
        ),
        (
            "uses.gcl",
            "import demo.parts as parts;\ninclude demo.parts() as a;\ninclude demo.parts() as b;\nnode called: Length = @parts()::missing;\n",
        ),
        (
            "plots.gcl",
            "import demo.parts as parts;\nplot p = { mark: point, encode: { x: @parts()::missing, y: 1.0 m } };\n",
        ),
        (
            "charts.gcl",
            "node hidden: Length = todo<Length> {};\npub plot shown = { mark: point, encode: { x: @hidden, y: 1.0 m } };\n",
        ),
        (
            "fx.gcl",
            "pub base dim Money;\npub base unit USD: Money;\nnode rate: Dimensionless = todo<Dimensionless> {};\npub unit EUR: Money = (@rate) USD;\n",
        ),
        (
            "nested_fx.gcl",
            "pub base dim Money;\npub base unit USD: Money;\nparam j: Dimensionless = 1.0;\nnode k: Dimensionless = todo<Dimensionless> {};\nunit inner: Dimensionless = (@k) USD/USD;\npub unit EUR: Money = (@j * 2.0 inner) USD;\n",
        ),
        (
            "nested_unit.gcl",
            "import demo.nested_fx::{dim Money, unit USD};\ninclude demo.nested_fx()::{ unit EUR as euros };\nassert rich = 2.0 euros > 1.0 USD;\n",
        ),
        (
            "consts.gcl",
            "pub const node g0: Acceleration = 9.80665 m/s^2;\npub const node v: Velocity = todo<Velocity> { @g0 };\npub const node v2: Velocity = 2.0 * @v;\n",
        ),
        (
            "uses_constants.gcl",
            "import demo.consts::{v2};\nimport demo.consts as c;\nnode a: Velocity = 3.0 * @v2;\nnode n: Velocity = 2.0 * c::v2;\nassert t = c::v2 > 0.0 m/s;\nplot p = { mark: point, encode: { x: c::v2, y: 1.0 m } };\n",
        ),
        ("member.gcl", "include demo.parts() as a;\n"),
        ("requested_plot.gcl", "include demo.charts()::{ shown };\n"),
        (
            "unit_assert.gcl",
            "import demo.fx::{dim Money, unit USD};\ninclude demo.fx()::{ unit EUR as euros };\nassert rich = 2.0 euros > 1.0 USD;\n",
        ),
        (
            "unit_plot.gcl",
            "import demo.fx::{dim Money, unit USD};\ninclude demo.fx()::{ unit EUR as euros };\nplot p = { mark: point, encode: { x: 1.0 euros, y: 1.0 m } };\n",
        ),
    ] {
        std::fs::write(sources.join(file), source).unwrap();
    }
    directory
}

fn check_in(directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
        .current_dir(directory)
        .arg("check")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn check_ignores_unfinished_declarations_of_imported_modules_that_the_file_does_not_use() {
    let package = package_with_unused_imported_todos();
    let output = check_in(package.path(), &["src/demo/main.gcl"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "ok: src/demo/main.gcl\n"
    );
    assert!(
        check_in(package.path(), &["--deny-todo", "src/demo/main.gcl"])
            .status
            .success()
    );
}

#[test]
fn check_counts_each_used_unfinished_declaration_once_by_its_definition() {
    let package = package_with_unused_imported_todos();
    for file in [
        "src/demo/uses.gcl",
        "src/demo/plots.gcl",
        "src/demo/member.gcl",
        "src/demo/requested_plot.gcl",
        "src/demo/unit_assert.gcl",
        "src/demo/unit_plot.gcl",
        "src/demo/nested_unit.gcl",
        "src/demo/uses_constants.gcl",
    ] {
        let output = check_in(package.path(), &[file]);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("ok: {file} (model incomplete: 1 unfinished declaration(s))\n")
        );
        assert!(
            !check_in(package.path(), &["--deny-todo", file])
                .status
                .success()
        );
    }
}

#[test]
fn checking_the_package_directory_reports_every_unfinished_declaration_where_it_is_defined() {
    let package = package_with_unused_imported_todos();
    let output = check_in(package.path(), &["src"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Directory traversal prints paths with the platform's separator.
    let line = |file: &str| {
        format!(
            "ok: {}",
            std::path::Path::new("src")
                .join("demo")
                .join(file)
                .display()
        )
    };
    for file in ["lib.gcl", "parts.gcl", "charts.gcl", "fx.gcl", "consts.gcl"] {
        assert!(stdout.contains(&format!(
            "{} (model incomplete: 1 unfinished declaration(s))\n",
            line(file)
        )));
    }
    assert!(stdout.contains(&format!("{}\n", line("main.gcl"))));
    assert!(
        !check_in(package.path(), &["--deny-todo", "src"])
            .status
            .success()
    );
}

#[test]
fn readers_of_an_imported_unfinished_constant_are_blocked_however_they_name_it() {
    let package = package_with_unused_imported_todos();
    let output = Command::new(env!("CARGO_BIN_EXE_graphcal"))
        .current_dir(package.path())
        .args(["eval", "--allow-incomplete", "src/demo/uses_constants.gcl"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in [
        "a     = BLOCKED — unfinished dependencies: demo.consts::v",
        "n     = BLOCKED — unfinished dependencies: demo.consts::v",
        "t  BLOCKED — unfinished dependencies: demo.consts::v",
    ] {
        assert!(stdout.contains(line), "missing `{line}` in:\n{stdout}");
    }
    assert!(
        stderr.contains("plot `p` not rendered: BLOCKED — unfinished dependencies: demo.consts::v"),
        "{stderr}"
    );
}

#[test]
fn a_bound_reading_an_imported_unfinished_constant_is_rejected() {
    let package = package_with_unused_imported_todos();
    std::fs::write(
        package.path().join("src/demo/bound.gcl"),
        "import demo.consts::{v2};\nparam x: Velocity(max: @v2) = 1.0 m/s;\n",
    )
    .unwrap();
    let output = check_in(package.path(), &["src/demo/bound.gcl"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("graphcal::C010"));
}
