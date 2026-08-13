#![no_main]

use graphcal_plugin_abi::{MAX_MANIFEST_BYTES, PluginManifest};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_MANIFEST_BYTES {
        return;
    }

    if let Ok(manifest) = PluginManifest::from_json(data) {
        let encoded = manifest
            .to_json()
            .expect("an accepted manifest must remain encodable");
        let decoded =
            PluginManifest::from_json(encoded.as_bytes()).expect("encoded manifest must decode");
        assert_eq!(decoded, manifest);
    }
});
