//! Report provenance: the baseline a report shows and the `graphcal eval`
//! command that reproduces it.
//!
//! The CLI and the browser engine build provenance through this one pure
//! builder, so a report reproduces and lists the same baseline wherever it
//! was built. Only the shells differ: what the sources are, and which
//! parameter arguments the invocation carried.

use graphcal_eval::eval::EvalResult;

use crate::report_ir::{Provenance, SourceDigest};
use crate::value_display::scalar_display;

/// Where an invocation's `--params-json` document came from.
#[derive(Debug, Clone, Copy)]
pub enum ParameterJsonArgument<'a> {
    /// `--params-json <json>`.
    Inline(&'a str),
    /// `--params-json-file <path>`.
    File(&'a str),
    /// `--params-json-file -`.
    Stdin,
}

/// The `graphcal eval` invocation that reproduces one report's baseline.
#[derive(Debug, Clone)]
pub struct ReproInvocation<'a> {
    /// Entry file, as the reader passes it to `graphcal eval`.
    pub entry: &'a str,
    /// Explicit `--root`, if any.
    pub root: Option<&'a str>,
    /// `--param name=expr` bindings, in order, each as the CLI spells it.
    pub parameters: Vec<(&'a str, &'a str)>,
    /// `--params-json` / `--params-json-file` document, if any.
    pub parameter_json: Option<ParameterJsonArgument<'a>>,
    /// `--params-json-max-bytes`, if any.
    pub parameter_json_max_bytes: Option<u64>,
}

impl ReproInvocation<'_> {
    /// The copy-pasteable POSIX shell command line.
    #[must_use]
    fn command_line(&self) -> String {
        let mut parts = vec![
            "graphcal".to_string(),
            "eval".to_string(),
            shell_quote(self.entry),
        ];
        if let Some(root) = self.root {
            parts.push("--root".to_string());
            parts.push(shell_quote(root));
        }
        for (name, expr) in &self.parameters {
            parts.push("--param".to_string());
            parts.push(shell_quote(&format!("{name}={expr}")));
        }
        match self.parameter_json {
            Some(ParameterJsonArgument::Inline(json)) => {
                parts.push("--params-json".to_string());
                parts.push(shell_quote(json));
            }
            Some(ParameterJsonArgument::File(path)) => {
                parts.push("--params-json-file".to_string());
                parts.push(shell_quote(path));
            }
            Some(ParameterJsonArgument::Stdin) => {
                parts.push("--params-json-file".to_string());
                parts.push("-".to_string());
            }
            None => {}
        }
        if let Some(limit) = self.parameter_json_max_bytes {
            parts.push("--params-json-max-bytes".to_string());
            parts.push(limit.to_string());
        }
        parts.join(" ")
    }
}

/// Build the provenance of a report evaluated as `result`.
///
/// The baseline lists the entry's own parameters only: an included DAG's
/// input port is bound by its includer, not by the reader, so it is no
/// baseline input.
#[must_use]
pub fn build_provenance(
    compiler_version: &str,
    sources: Vec<SourceDigest>,
    result: &EvalResult,
    invocation: &ReproInvocation<'_>,
) -> Provenance {
    Provenance {
        compiler_version: compiler_version.to_string(),
        sources,
        baseline_params: baseline_params(result),
        repro_command: invocation.command_line(),
    }
}

/// Baseline entry parameter values as displayed, in declaration order.
fn baseline_params(result: &EvalResult) -> Vec<(String, String)> {
    result
        .entry_params()
        .map(|(name, outcome)| {
            let display = match outcome {
                Ok(value) => scalar_display(value, &result.render)
                    .unwrap_or_else(|error| format!("ERROR: {error}")),
                Err(error) => format!("ERROR: {error}"),
            };
            (name.to_string(), display)
        })
        .collect()
}

/// Quote one shell word.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_handles_plain_text_and_single_quotes() {
        assert_eq!(shell_quote("model.gcl"), "'model.gcl'");
        assert_eq!(shell_quote("scenario's.json"), "'scenario'\"'\"'s.json'");
    }

    #[test]
    fn command_line_spells_every_argument_in_order() {
        let invocation = ReproInvocation {
            entry: "src/main.gcl",
            root: Some("."),
            parameters: vec![("mass", "12.0 kg"), ("note", "it's")],
            parameter_json: Some(ParameterJsonArgument::Stdin),
            parameter_json_max_bytes: Some(1024),
        };
        assert_eq!(
            invocation.command_line(),
            "graphcal eval 'src/main.gcl' --root '.' --param 'mass=12.0 kg' \
             --param 'note=it'\"'\"'s' --params-json-file - --params-json-max-bytes 1024"
        );
    }

    #[test]
    fn baseline_lists_only_entry_parameters() {
        let result = graphcal_project::prepare::compile_and_eval(
            "param k: Length = 2.0 m;\n\
             dag lib { param gain: Dimensionless = 1.0; pub node out: Dimensionless = @gain; }\n\
             include lib(gain: 3.0)::{ gain, out };",
        )
        .unwrap();
        let provenance = build_provenance(
            "0.0.0",
            Vec::new(),
            &result,
            &ReproInvocation {
                entry: "main.gcl",
                root: None,
                parameters: Vec::new(),
                parameter_json: None,
                parameter_json_max_bytes: None,
            },
        );
        assert_eq!(
            provenance.baseline_params,
            vec![("k".to_string(), "2 m".to_string())]
        );
        assert_eq!(provenance.repro_command, "graphcal eval 'main.gcl'");
    }
}
