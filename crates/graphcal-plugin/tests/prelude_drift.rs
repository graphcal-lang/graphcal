//! Drift test for the macro's dimension vocabulary.
//!
//! The `plugin!` macro mirrors the prelude's base and derived dimensions
//! (it cannot depend on `graphcal-compiler`), so the mirror must be
//! verified against the real thing: for every vocabulary name, a `.gcl`
//! extern declaration spelling that name is compiled against the
//! macro-produced manifest signature. The loader's structural verification
//! (P005) then proves both sides denote the same dimension — if the
//! macro's table ever disagrees with the prelude, these tests fail.

#![cfg(test)]
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

use std::path::Path;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::prelude::PRELUDE_BASE_DIMENSION_NAMES;
use graphcal_compiler::syntax::plugin::PluginPath;
use graphcal_eval::eval::CompileError;
use graphcal_eval::host_fns::HostFunctionRegistry;
use graphcal_eval::loader::load_project;
use graphcal_io::RealFileSystem;
use graphcal_plugin_abi::PluginManifest;

graphcal_plugin::plugin! {
    fn f_length(x: Length) -> Length { x }
    fn f_time(x: Time) -> Time { x }
    fn f_mass(x: Mass) -> Mass { x }
    fn f_temperature(x: Temperature) -> Temperature { x }
    fn f_electric_current(x: ElectricCurrent) -> ElectricCurrent { x }
    fn f_amount(x: Amount) -> Amount { x }
    fn f_luminous_intensity(x: LuminousIntensity) -> LuminousIntensity { x }
    fn f_angle(x: Angle) -> Angle { x }
    fn f_velocity(x: Velocity) -> Velocity { x }
    fn f_acceleration(x: Acceleration) -> Acceleration { x }
    fn f_force(x: Force) -> Force { x }
    fn f_energy(x: Energy) -> Energy { x }
    fn f_power(x: Power) -> Power { x }
    fn f_frequency(x: Frequency) -> Frequency { x }
    fn f_pressure(x: Pressure) -> Pressure { x }
    fn f_area(x: Area) -> Area { x }
    fn f_volume(x: Volume) -> Volume { x }
    fn f_dimensionless(x: Dimensionless) -> Dimensionless { x }
}

/// The `.gcl` extern declarations naming the same vocabulary, in the same
/// order as the `plugin!` block above.
const GCL_DECLARATIONS: &str = r#"
import plugin "graphcal:sdk-drift" as sdk {
    fn f_length(x: Length) -> Length;
    fn f_time(x: Time) -> Time;
    fn f_mass(x: Mass) -> Mass;
    fn f_temperature(x: Temperature) -> Temperature;
    fn f_electric_current(x: ElectricCurrent) -> ElectricCurrent;
    fn f_amount(x: Amount) -> Amount;
    fn f_luminous_intensity(x: LuminousIntensity) -> LuminousIntensity;
    fn f_angle(x: Angle) -> Angle;
    fn f_velocity(x: Velocity) -> Velocity;
    fn f_acceleration(x: Acceleration) -> Acceleration;
    fn f_force(x: Force) -> Force;
    fn f_energy(x: Energy) -> Energy;
    fn f_power(x: Power) -> Power;
    fn f_frequency(x: Frequency) -> Frequency;
    fn f_pressure(x: Pressure) -> Pressure;
    fn f_area(x: Area) -> Area;
    fn f_volume(x: Volume) -> Volume;
    fn f_dimensionless(x: Dimensionless) -> Dimensionless;
}

node ok: Dimensionless = sdk.f_dimensionless(1.0);
"#;

/// Register every macro-manifest signature under the drift identity, the
/// same way the wasm loader registers real plugin functions.
fn registry_from_embedded_manifest() -> HostFunctionRegistry {
    let manifest = PluginManifest::from_json(&GRAPHCAL_PLUGIN_MANIFEST).expect("manifest decodes");
    let functions =
        graphcal_plugin_host::convert_manifest(&manifest).expect("manifest converts to typed IR");
    let plugin = PluginPath::new("graphcal:sdk-drift");
    let mut registry = HostFunctionRegistry::new();
    for (name, signature) in functions {
        registry
            .register_with_signature(plugin.clone(), name, signature, |args| Ok(args[0].clone()));
    }
    registry
}

fn compile(dir: &Path, source: &str, registry: &HostFunctionRegistry) -> Result<(), CompileError> {
    let entry = dir.join("main.gcl");
    std::fs::write(&entry, source).expect("write test project");
    let fs = RealFileSystem::default();
    let project = load_project(&entry, None, &fs)?;
    graphcal_eval::eval::compile_to_tir_from_project_with_host_fns(&project, registry)?;
    Ok(())
}

#[test]
fn every_vocabulary_name_matches_the_prelude() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_from_embedded_manifest();
    // Structural verification of every declaration against the macro's
    // manifest happens during compilation; any table drift is a P005.
    compile(dir.path(), GCL_DECLARATIONS, &registry)
        .expect("macro vocabulary must match the prelude");
}

#[test]
fn drift_would_be_detected() {
    // Negative control: declare one function with the wrong signature and
    // require P005, proving the positive test actually verifies signatures.
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_from_embedded_manifest();
    let source = r#"
import plugin "graphcal:sdk-drift" as sdk {
    fn f_pressure(x: Pressure) -> Area;
}

node ok: Dimensionless = 1.0;
"#;
    let err = compile(dir.path(), source, &registry).expect_err("mismatch must be rejected");
    let CompileError::Eval(GraphcalError::ExternSignatureMismatch { name, .. }) = err else {
        panic!("expected ExternSignatureMismatch, got {err:?}");
    };
    assert_eq!(name.as_str(), "f_pressure");
}

#[test]
fn manifest_fixed_dimensions_stay_in_the_base_alphabet() {
    // The ABI only speaks base dimensions; derived sugar must never leak
    // its name into the manifest, and spellings must match the prelude's.
    let manifest = PluginManifest::from_json(&GRAPHCAL_PLUGIN_MANIFEST).expect("manifest decodes");
    for function in &manifest.functions {
        let kinds = function
            .params
            .iter()
            .map(|param| &param.kind)
            .chain([&function.result]);
        for kind in kinds {
            let graphcal_plugin_abi::ManifestValueKind::Scalar(monomial) = kind else {
                continue;
            };
            for factor in &monomial.fixed {
                assert!(
                    PRELUDE_BASE_DIMENSION_NAMES.contains(&factor.dim.as_str()),
                    "function `{}` leaked non-base dimension `{}` into the manifest",
                    function.name,
                    factor.dim
                );
            }
        }
    }
    // Base-dimension functions must carry exactly their own base name.
    for (function_name, base_name) in [
        ("f_length", "Length"),
        ("f_time", "Time"),
        ("f_mass", "Mass"),
        ("f_temperature", "Temperature"),
        ("f_electric_current", "ElectricCurrent"),
        ("f_amount", "Amount"),
        ("f_luminous_intensity", "LuminousIntensity"),
        ("f_angle", "Angle"),
    ] {
        let function = manifest
            .functions
            .iter()
            .find(|function| function.name == function_name)
            .expect("declared above");
        let graphcal_plugin_abi::ManifestValueKind::Scalar(monomial) = &function.result else {
            panic!("base-dimension functions return scalars");
        };
        assert_eq!(monomial.fixed.len(), 1, "{function_name}");
        assert_eq!(monomial.fixed[0].dim, base_name);
        assert_eq!(
            (monomial.fixed[0].pow.num, monomial.fixed[0].pow.den),
            (1, 1)
        );
    }
}
