#![no_main]

use std::collections::HashMap;
use std::sync::LazyLock;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, compile_and_eval_project};
use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use graphcal_test_support::bytes::project_from_bytes;
use graphcal_test_support::project::GenerationLimits;
use libfuzzer_sys::fuzz_target;

const VIRTUAL_ROOT: &str = "/generated";

/// `GRAPHCAL_FUZZ_PROJECT_LIMITS=deep` widens generation bounds and the input
/// budget for long local or nightly campaigns; CI smoke runs use the default.
static CAMPAIGN: LazyLock<(GenerationLimits, usize)> =
    LazyLock::new(|| match std::env::var("GRAPHCAL_FUZZ_PROJECT_LIMITS") {
        Ok(mode) if mode == "deep" => (GenerationLimits::DEEP, 16 * 1024),
        _ => (GenerationLimits::SMOKE, 4 * 1024),
    });

fuzz_target!(|data: &[u8]| {
    let (limits, max_input_bytes) = *CAMPAIGN;
    if data.len() > max_input_bytes {
        return;
    }
    let project = project_from_bytes(data, limits);
    let rendered = project.render();
    let root = VirtualAbsolutePath::new(VIRTUAL_ROOT).expect("static virtual root");
    let mut fs = InMemoryFileSystem::new();
    for (path, source) in rendered.files() {
        fs.add_file(
            VirtualAbsolutePath::new(root.as_path().join(path)).expect("typed relative path"),
            source.to_string(),
        )
        .expect("typed project has a valid file topology");
    }

    if let Err(CompileError::Eval(GraphcalError::InternalError { message, .. })) =
        compile_and_eval_project(
            &root.as_path().join(rendered.root()),
            &HashMap::new(),
            Some(root.as_path()),
            &fs,
        )
    {
        panic!("typed generated project reached an internal error: {message}");
    }
});
