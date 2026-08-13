#![no_main]

use graphcal_plugin_abi::{MAX_MANIFEST_BYTES, PluginManifest};
use libfuzzer_sys::fuzz_target;

const MAX_WASM_BYTES: usize = 512 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_WASM_BYTES {
        return;
    }

    if let Ok(manifest) = PluginManifest::from_wasm(data) {
        let encoded = manifest
            .to_json()
            .expect("an extracted manifest must remain encodable");
        assert!(encoded.len() <= MAX_MANIFEST_BYTES);

        let embedded = manifest
            .embed_into(&graphcal_plugin_abi::section::EMPTY_MODULE)
            .expect("an extracted manifest must embed into an empty module");
        let decoded =
            PluginManifest::from_wasm(&embedded).expect("an embedded manifest must be extractable");
        assert_eq!(decoded, manifest);
    }
});
