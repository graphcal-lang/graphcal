#![no_main]

use graphcal_fmt::{FormatError, format_source};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    match format_source(source) {
        Ok(formatted) => {
            let reformatted = format_source(&formatted)
                .expect("formatter output must always remain valid Graphcal");
            assert_eq!(reformatted, formatted, "formatting must be idempotent");
        }
        Err(FormatError::Parse(_)) => {}
        Err(error) => panic!("valid formatter input reached an internal failure: {error}"),
    }
});
