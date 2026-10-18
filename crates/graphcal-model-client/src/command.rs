//! How to invoke the `graphcal` executable for one model.

use std::path::{Path, PathBuf};
use std::process::Command;

use graphcal_model_protocol::{InterfaceFingerprint, SessionSpec};

/// The `graphcal` executable and the model file it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphcalCommand {
    program: PathBuf,
    file: PathBuf,
    root: Option<PathBuf>,
}

impl GraphcalCommand {
    /// Serve `file` with the `graphcal` executable found on `PATH`.
    #[must_use]
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("graphcal"),
            file: file.into(),
            root: None,
        }
    }

    /// Use the executable at `program` instead.
    #[must_use]
    pub fn program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Pass `--root` to override project-root detection.
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// The model file.
    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }

    fn base(&self, subcommand: &str) -> Command {
        let mut command = Command::new(&self.program);
        command.args(["model", subcommand]).arg(&self.file);
        if let Some(root) = &self.root {
            command.arg("--root").arg(root);
        }
        command
    }

    /// `graphcal model describe`.
    pub(crate) fn describe(&self) -> Command {
        self.base("describe")
    }

    /// `graphcal model serve` for `spec`, refusing any interface but the
    /// one with `fingerprint`.
    pub(crate) fn serve(&self, spec: &SessionSpec, fingerprint: InterfaceFingerprint) -> Command {
        let mut command = self.base("serve");
        for input in spec.inputs() {
            command.arg("--input").arg(input.as_str());
        }
        for output in spec.outputs() {
            command.arg("--output").arg(output.as_str());
        }
        for choice in spec.units() {
            command
                .arg("--unit")
                .arg(format!("{}={}", choice.port(), choice.unit()));
        }
        command
            .arg("--expect-fingerprint")
            .arg(fingerprint_argument(fingerprint));
        command
    }
}

fn fingerprint_argument(fingerprint: InterfaceFingerprint) -> String {
    fingerprint.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_passes_the_file_and_root() {
        let command = GraphcalCommand::new("model.gcl")
            .program("/opt/graphcal")
            .root("/project")
            .describe();
        assert_eq!(command.get_program(), "/opt/graphcal");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            ["model", "describe", "model.gcl", "--root", "/project"]
        );
    }

    #[test]
    fn serve_passes_the_selection_units_and_fingerprint() {
        use graphcal_model_protocol::{PortName, UnitSpelling};

        let name = |name: &str| PortName::new(name).unwrap();
        let spec = SessionSpec::new(vec![name("load")], vec![name("margin")])
            .with_unit(name("load"), UnitSpelling::new("kN").unwrap());
        let fingerprint = InterfaceFingerprint::parse(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let command = GraphcalCommand::new("model.gcl").serve(&spec, fingerprint);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "model",
                "serve",
                "model.gcl",
                "--input",
                "load",
                "--output",
                "margin",
                "--unit",
                "load=kN",
                "--expect-fingerprint",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            ]
        );
    }
}
