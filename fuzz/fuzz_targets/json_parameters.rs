#![no_main]

//! The CLI's exact-JSON parameter input (`--params-json`): an untrusted
//! document is converted to parameter expressions and bound to a prepared
//! project, as `graphcal eval --params-json` does.

use std::sync::{Arc, LazyLock};

use graphcal_project::loader::LoadedProject;
use graphcal_project::prepare::{PreparedProject, prepare_from_project};
use libfuzzer_sys::fuzz_target;
use miette::NamedSource;

/// The CLI module under test. It lives in the `graphcal` binary crate, whose
/// build script embeds the browser engine, so it is compiled into this target
/// directly instead of through a dependency on the whole CLI.
#[path = "../../crates/graphcal-cli/src/json_input.rs"]
#[allow(
    dead_code,
    reason = "the target exercises only the module's public entry point"
)]
mod json_input;

const MAX_INPUT_BYTES: usize = 64 * 1024;

/// One parameter of each value shape the JSON schema distinguishes.
const MODEL: &str = r#"
pub index Phase = { Climb, Cruise };
pub type Shape {
    Circle(r: Length),
    Dot,
}
pub type Stage {
    Stage(dry: Mass, burns: Int),
}
param load: Length(min: 0.0 m, max: 10.0 m) = 1.0 m;
param cycles: Int = 1;
param enabled: Bool = true;
param ratio: Dimensionless = 0.5;
param profile: Length[Phase] = { Phase#Climb: 1.0 m, Phase#Cruise: 2.0 m };
param shape: Shape = Dot;
param stage: Stage = Stage(dry: 1.0 kg, burns: 1);
param at: Datetime<TAI> = epoch<TAI>("2024-11-05T12:00:00");
pub node out: Length = @load * 2.0;
"#;

static PROJECT: LazyLock<PreparedProject> = LazyLock::new(|| {
    prepare_from_project(
        &LoadedProject::from_source(MODEL, "model.gcl").expect("the fuzz model loads"),
    )
    .expect("the fuzz model prepares")
});

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(overrides) = json_input::json_to_overrides(text) else {
        return;
    };

    let source = NamedSource::new("params.json", Arc::new(text.to_string()));
    let mut bindings = PROJECT.binding_builder();
    for (name, expression) in overrides {
        // Unknown names and ill-typed values are ordinary binding errors.
        let _ = bindings.bind_external_expression(
            &name,
            &expression.into(),
            &source,
            (0usize, 0usize).into(),
        );
    }
});
