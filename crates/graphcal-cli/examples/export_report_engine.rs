//! Export the exact bundle embedded by this build for Node tests or packaging.
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("usage: export_report_engine <destination>")?,
    );
    fs::create_dir_all(&destination)?;
    [
        (
            "graphcal_wasm.js",
            include_bytes!(concat!(env!("OUT_DIR"), "/report-engine/graphcal_wasm.js")).as_slice(),
        ),
        (
            "graphcal_wasm_bg.wasm",
            include_bytes!(concat!(
                env!("OUT_DIR"),
                "/report-engine/graphcal_wasm_bg.wasm"
            ))
            .as_slice(),
        ),
        (
            "bundle.json",
            include_bytes!(concat!(env!("OUT_DIR"), "/report-engine/bundle.json")).as_slice(),
        ),
    ]
    .iter()
    .try_for_each(|(name, bytes)| fs::write(destination.join(name), bytes))?;
    Ok(())
}
