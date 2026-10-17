//! How to invoke the `graphcal` executable for one model.

use std::path::{Path, PathBuf};
use std::process::Command;

use graphcal_model_protocol::{InterfaceFingerprint, SessionSchema};

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

    /// `graphcal model serve` for `session`, refusing any other interface.
    pub(crate) fn serve(&self, session: &SessionSchema) -> Command {
        let mut command = self.base("serve");
        for input in session.inputs() {
            command.arg("--input").arg(input.name().as_str());
        }
        for output in session.outputs() {
            command.arg("--output").arg(output.name().as_str());
        }
        command
            .arg("--expect-fingerprint")
            .arg(fingerprint_argument(session.interface_fingerprint()));
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
}
