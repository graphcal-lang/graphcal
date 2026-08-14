#![no_main]

use graphcal_compiler::syntax::parser::Parser;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    let _ = Parser::with_name(source, "fuzz.gcl").parse_file();
});
