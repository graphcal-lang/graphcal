#![no_main]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, compile_and_eval_project};
use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use graphcal_test_support::bytes::project_from_bytes;
use graphcal_test_support::project::GenerationLimits;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 4 * 1024;
const VIRTUAL_ROOT: &str = "/generated";

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let project = project_from_bytes(data, GenerationLimits::SMOKE);
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
