#![no_main]

//! Report bundles: the browser engine decodes an untrusted bundle envelope
//! with `ProjectBundle::from_json` and mounts it before compiling. The
//! JavaScript-side request decoding around it needs a JavaScript host and is
//! covered by the engine's `wasm-pack` tests instead.

use std::path::Path;

use graphcal_project::project_bundle::ProjectBundle;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 256 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(json) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(bundle) = ProjectBundle::from_json(json) else {
        return;
    };

    let canonical = bundle
        .to_json()
        .expect("a decoded bundle must re-encode within the size limit");
    let reparsed = ProjectBundle::from_json(&canonical).expect("a re-encoded bundle must decode");
    assert_eq!(
        reparsed.to_json().expect("a reparsed bundle must re-encode"),
        canonical
    );

    // Mounting validates paths, categories, and limits; rejection is fine.
    let _ = bundle.mount(Path::new("/playground"));
});
