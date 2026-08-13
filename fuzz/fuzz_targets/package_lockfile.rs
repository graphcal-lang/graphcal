#![no_main]

use graphcal_package::{LockfileParseLimits, parse_lockfile_str_with_limits};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 128 * 1024;
const MAX_PACKAGES: usize = 128;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(lockfile) =
        parse_lockfile_str_with_limits(source, LockfileParseLimits::new(MAX_PACKAGES))
    {
        let canonical = lockfile.to_deterministic_toml();
        let reparsed =
            parse_lockfile_str_with_limits(&canonical, LockfileParseLimits::new(MAX_PACKAGES))
                .expect("canonical lockfile must reparse under the same limit");
        assert_eq!(reparsed.to_deterministic_toml(), canonical);
    }
});
