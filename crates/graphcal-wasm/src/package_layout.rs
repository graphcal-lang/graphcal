//! Where a package keeps its sources, read from its `graphcal.toml`.
//!
//! A browser host gathers a project from a remote repository file by file; it
//! asks this before downloading so it fetches only the package namespace
//! (`<source_dir>/<name>.gcl` and `<source_dir>/<name>/`) and not every file
//! under the package root. The manifest is parsed by the same validator the
//! loader uses, so the two cannot disagree.

use serde::Serialize;

/// The package's name and source directory, or why the manifest is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PackageLayoutView {
    /// A valid manifest.
    Package {
        /// The package name, the first segment of its module paths.
        name: String,
        /// The source directory relative to the package root, `/`-separated.
        source_dir: String,
    },
    /// The manifest does not parse or validate.
    Invalid {
        /// The manifest error, rendered.
        message: String,
    },
}

/// The layout the manifest text declares.
#[must_use]
pub fn package_layout(manifest: &str) -> PackageLayoutView {
    match graphcal_package::parse_manifest_str(manifest) {
        Ok(manifest) => PackageLayoutView::Package {
            name: manifest.name.to_string(),
            source_dir: manifest.source_dir.to_string(),
        },
        Err(error) => PackageLayoutView::Invalid {
            message: error.to_string(),
        },
    }
}

/// The layout reported for a manifest too large to read: it is rejected
/// before its text is copied, with the limit every project file shares.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub fn oversized_manifest_layout() -> PackageLayoutView {
    PackageLayoutView::Invalid {
        message: format!(
            "graphcal.toml exceeds the {} byte limit for a project file",
            crate::project::MAX_PLAYGROUND_FILE_BYTES
        ),
    }
}

/// JavaScript boundary: `packageLayout(manifestText)`.
///
/// The text is bounded before it is copied into linear memory (#1344).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = packageLayout)]
pub fn package_layout_js(
    manifest: wasm_bindgen::JsValue,
) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    let layout = match crate::js_request::decode_manifest_text(manifest)? {
        crate::js_request::ManifestText::Within(text) => package_layout(&text),
        crate::js_request::ManifestText::TooLarge => oversized_manifest_layout(),
    };
    crate::output::to_js(&layout).map_err(|error| {
        wasm_bindgen::JsValue::from_str(&format!("could not serialize package layout: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_dir_defaults_to_src() {
        assert_eq!(
            package_layout("[package]\nname = \"power\"\n"),
            PackageLayoutView::Package {
                name: "power".to_string(),
                source_dir: "src".to_string(),
            }
        );
    }

    #[test]
    fn reads_a_nested_source_dir() {
        assert_eq!(
            package_layout("[package]\nname = \"power\"\nsource_dir = \"lib/gcl\"\n"),
            PackageLayoutView::Package {
                name: "power".to_string(),
                source_dir: "lib/gcl".to_string(),
            }
        );
    }

    #[test]
    fn an_oversized_manifest_names_the_limit() {
        let PackageLayoutView::Invalid { message } = oversized_manifest_layout() else {
            panic!("an oversized manifest is invalid");
        };
        assert!(
            message.contains(&crate::project::MAX_PLAYGROUND_FILE_BYTES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn an_invalid_manifest_says_why() {
        let PackageLayoutView::Invalid { message } = package_layout("[package]\n") else {
            panic!("a manifest without a name is invalid");
        };
        assert!(message.contains("name"), "{message}");
    }
}
