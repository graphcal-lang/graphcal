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

/// JavaScript boundary: `packageLayout(manifestText)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = packageLayout)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "wasm-bindgen JavaScript exports receive owned strings"
)]
pub fn package_layout_js(manifest: String) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    crate::output::to_js(&package_layout(&manifest)).map_err(|error| {
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
    fn an_invalid_manifest_says_why() {
        let PackageLayoutView::Invalid { message } = package_layout("[package]\n") else {
            panic!("a manifest without a name is invalid");
        };
        assert!(message.contains("name"), "{message}");
    }
}
