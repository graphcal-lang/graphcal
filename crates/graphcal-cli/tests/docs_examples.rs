//! Regression checks against the actual documentation sources, not copied fixtures.

use std::path::Path;
use std::process::Command;

#[test]
fn index_key_and_argmax_documentation_examples_compile() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    for locale in ["en", "ja"] {
        for page in ["indexes", "built-ins"] {
            let path = repository.join(format!("docs/{locale}/language/{page}.md"));
            let markdown = std::fs::read_to_string(&path).unwrap();
            // These pages also contain fragments that need surrounding context.
            // Select the complete Maneuver/key example, retaining its actual source.
            let examples: Vec<_> = markdown
                .split("```")
                .skip(1)
                .step_by(2)
                .filter(|block| {
                    block.contains("index Maneuver =")
                        && block.contains("node critical: Key<Maneuver>")
                })
                .collect();
            assert_eq!(examples.len(), 1, "{}", path.display());
            let (_, source) = examples[0].split_once('\n').unwrap();
            let directory = tempfile::tempdir().unwrap();
            let file = directory.path().join("example.gcl");
            std::fs::write(&file, source).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_graphcal"))
                .arg("check")
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} example failed:\n{}\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
}
