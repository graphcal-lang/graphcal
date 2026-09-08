//! Pure identity and integrity checks for generated report-engine bundles.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GLUE: &str = "graphcal_wasm.js";
pub const WASM: &str = "graphcal_wasm_bg.wasm";
pub const MANIFEST: &str = "bundle.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    pub rustc: String,
    pub cargo: String,
    pub bindgen: String,
    pub optimizer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub source: [u8; 32],
    pub tools: Toolchain,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub inputs: Inputs,
    glue: [u8; 32],
    wasm: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum IntegrityError {
    #[error("report-engine bundle belongs to a different Graphcal version")]
    Version,
    #[error("report-engine bundle is stale for the current build inputs")]
    Stale,
    #[error("report-engine JavaScript checksum mismatch")]
    Glue,
    #[error("report-engine Wasm checksum mismatch")]
    Wasm,
}

pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl Manifest {
    pub fn new(inputs: Inputs, glue: &[u8], wasm: &[u8]) -> Self {
        Self {
            inputs,
            glue: digest(glue),
            wasm: digest(wasm),
        }
    }

    pub fn verify(&self, version: &str, glue: &[u8], wasm: &[u8]) -> Result<(), IntegrityError> {
        if self.inputs.version != version {
            return Err(IntegrityError::Version);
        }
        if self.glue != digest(glue) {
            return Err(IntegrityError::Glue);
        }
        if self.wasm != digest(wasm) {
            return Err(IntegrityError::Wasm);
        }
        Ok(())
    }

    pub fn verify_inputs(&self, inputs: &Inputs) -> Result<(), IntegrityError> {
        if self.inputs == *inputs {
            Ok(())
        } else {
            Err(IntegrityError::Stale)
        }
    }
}
