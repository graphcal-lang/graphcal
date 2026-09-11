//! Prepare-once, evaluate-repeatedly browser API.
//!
//! The one-shot [`crate::evaluate`] entry recompiles the project on every
//! call. Interactive consumers (the playground, hydrated reports) instead
//! prepare a project once, read its typed parameter ports, and re-evaluate
//! repeatedly with reader-supplied closed values — the same `--param`
//! discipline the CLI enforces: full literal syntax, unit-checked, no
//! expression injection into the prepared DAG.
//!
//! Cancellation: a running evaluation cannot be interrupted from JavaScript
//! (the worker's event loop is blocked while Wasm runs), so cancellation is
//! worker teardown — terminate the worker and prepare a fresh instance, the
//! same contract the playground already uses.

use graphcal_eval::eval::{
    EvalResult, ModelIndexKind, ModelIndexSchema, ModelSchemaGraph, ModelValueSchema,
    ParameterDomain, ParameterPort, PreparedProject, ProjectCompiler,
};
use graphcal_eval::host_fns::demo_registry;
use graphcal_eval::loader::{LoaderBudget, load_project_with_dependency_sources};
use serde::Serialize;

use crate::PlaygroundRequest;
use crate::bindings::{
    BindingPathSegmentView, BindingRequest, BrowserBindingRequest, StructuredBindingRequest,
    StructuredValueRequest, bind_browser_one, bind_one,
};
use crate::diagnostics::{DiagnosticView, compile_error_view};
use crate::output::EvaluationView;
use crate::project::{ProjectValidationError, RequestErrorView, VirtualProject};

/// A checked in-memory project ready for repeated evaluation.
pub struct PreparedPlayground {
    prepared: PreparedProject,
    report: crate::browser_report::ReportMetadata,
}

/// Outcome of preparing one browser project.
pub enum PrepareOutcome {
    /// The project compiled; keep the handle and evaluate repeatedly.
    Prepared(Box<PreparedPlayground>),
    /// The request violated a playground capability or limit.
    Rejected { error: RequestErrorView },
    /// The project failed to compile.
    CompileError { diagnostics: Vec<DiagnosticView> },
}

/// Validate and compile an in-memory browser project for repeated evaluation.
///
/// Text-only playground requests still reject plugins: they cannot supply
/// executable artifacts. Offline reports use [`prepare_bundle`] to provide
/// the complete validated snapshot, including pinned plugin binaries.
#[must_use]
pub fn prepare(request: PlaygroundRequest) -> PrepareOutcome {
    let project = match VirtualProject::try_from(request) {
        Ok(project) => project,
        Err(error) => {
            return PrepareOutcome::Rejected {
                error: RequestErrorView::from(&error),
            };
        }
    };

    prepare_virtual(&project, BrowserCapabilities::SourcesOnly)
}

/// Prepare an offline report snapshot, including sandboxed Wasm plugins.
#[must_use]
pub fn prepare_bundle(bundle: &graphcal_eval::project_bundle::ProjectBundle) -> PrepareOutcome {
    match VirtualProject::from_bundle(bundle) {
        Ok(project) => prepare_virtual(&project, BrowserCapabilities::BundledPlugins),
        Err(error) => PrepareOutcome::Rejected {
            error: RequestErrorView::from(&ProjectValidationError::InvalidBundle {
                message: error.to_string(),
            }),
        },
    }
}

#[derive(Clone, Copy)]
enum BrowserCapabilities {
    SourcesOnly,
    BundledPlugins,
}

fn prepare_virtual(project: &VirtualProject, capabilities: BrowserCapabilities) -> PrepareOutcome {
    let filesystem = project.filesystem();
    let loaded = match load_project_with_dependency_sources(
        &project.entry_path(),
        Some(VirtualProject::root_path()),
        &filesystem,
        project.dependency_sources(),
        LoaderBudget::default(),
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            return PrepareOutcome::CompileError {
                diagnostics: vec![compile_error_view(&error, project)],
            };
        }
    };

    if matches!(capabilities, BrowserCapabilities::SourcesOnly)
        && loaded
            .files()
            .values()
            .any(|file| file.ast().uses_plugins())
    {
        let error = ProjectValidationError::PluginsUnsupported;
        return PrepareOutcome::Rejected {
            error: RequestErrorView::from(&error),
        };
    }

    let report =
        crate::browser_report::ReportMetadata::from_loaded(&loaded, project.entry_display());
    let mut registry = demo_registry();
    graphcal_plugin_host::register_project_plugins(
        &graphcal_plugin_host::PluginHost::new(),
        &loaded,
        &mut registry,
    );
    match ProjectCompiler::new(&loaded).host_fns(&registry).prepare() {
        Ok(prepared) => PrepareOutcome::Prepared(Box::new(PreparedPlayground { prepared, report })),
        Err(error) => PrepareOutcome::CompileError {
            diagnostics: vec![compile_error_view(&error, project)],
        },
    }
}

/// One rejected binding, addressed to the control that produced it.
#[derive(Debug, Clone, Serialize)]
pub struct BindingErrorView {
    pub name: String,
    pub message: String,
    pub path: Vec<BindingPathSegmentView>,
}

/// Outcome of one repeated evaluation.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvaluateOutcome {
    /// The row evaluated; per-node failures are inside the view.
    Evaluated { evaluation: EvaluationView },
    /// One or more bindings were rejected; nothing was evaluated.
    BindingErrors { errors: Vec<BindingErrorView> },
    /// The evaluator failed as a whole (e.g. a required parameter without a
    /// default was left unbound).
    EvalError { message: String },
}

/// Outcome of evaluating and rendering a host-asset report.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvaluateReportOutcome {
    Evaluated {
        evaluation: EvaluationView,
        html: String,
    },
    BindingErrors {
        errors: Vec<BindingErrorView>,
    },
    EvalError {
        message: String,
    },
}

enum EvaluationFailure {
    BindingErrors(Vec<BindingErrorView>),
    EvalError(String),
}

/// Browser-facing schema of one entry parameter port.
#[derive(Debug, Clone, Serialize)]
pub struct ParameterPortView {
    pub name: String,
    /// Whether evaluation can fall back to a compiled default when the
    /// parameter is left unbound.
    pub has_default: bool,
    /// Legacy scalar control metadata, retained for compatibility.
    pub control: ControlView,
    /// Recursive root schema for the structured editor.
    pub schema: ValueSchemaView,
    /// Finite algebraic definition arena referenced by `schema`.
    pub definitions: Vec<AlgebraicDefinitionView>,
}

/// Recursive browser value schema. Algebraic references keep recursive and
/// mutually recursive declarations finite.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueSchemaView {
    Quantity {
        unit: Option<String>,
    },
    Complex {
        unit: Option<String>,
    },
    Boolean,
    Integer,
    Datetime {
        time_scale: String,
    },
    Key {
        axis: IndexSchemaView,
    },
    Algebraic {
        definition: usize,
    },
    Indexed {
        axis: IndexSchemaView,
        element: Box<Self>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexSchemaView {
    Named { name: String, variants: Vec<String> },
    Coordinate { name: String, labels: Vec<String> },
    Finite { cardinality: usize },
}

#[derive(Debug, Clone, Serialize)]
pub struct AlgebraicDefinitionView {
    pub id: usize,
    pub constructors: Vec<ConstructorSchemaView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConstructorSchemaView {
    pub id: usize,
    pub name: String,
    pub fields: Vec<FieldSchemaView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldSchemaView {
    pub id: usize,
    pub name: String,
    pub schema: ValueSchemaView,
}

/// The control family a parameter port maps to, derived from its declared
/// type and domain constraints.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlView {
    /// Real quantity: unit-checked entry field; a slider when both bounds
    /// are declared. Bounds are SI values; `unit` is the canonical SI label
    /// (absent for dimensionless quantities).
    Quantity {
        unit: Option<String>,
        lower_si: Option<f64>,
        upper_si: Option<f64>,
    },
    /// Exact integer endpoints in lossless decimal notation. Numeric sliders
    /// are a browser concern and require a safe-integer proof.
    Integer {
        lower: Option<String>,
        upper: Option<String>,
    },
    /// Boolean: checkbox.
    Boolean,
    /// Named index key: select over the declared variants.
    Select {
        index: String,
        variants: Vec<String>,
    },
    /// Physical instant in one declared time scale: unit-checked field.
    Datetime { time_scale: String },
    /// Structured or otherwise unenumerable value: full closed-literal
    /// expression entry only.
    Expression,
}

impl PreparedPlayground {
    /// Typed entry parameter ports in source declaration order.
    #[must_use]
    pub fn ports(&self) -> Vec<ParameterPortView> {
        self.prepared
            .parameter_ports()
            .iter()
            .map(|port| parameter_port_view(port, &self.prepared))
            .collect()
    }

    /// Bind the supplied closed values and evaluate the full project view.
    ///
    /// Unbound parameters fall back to their compiled defaults. Every
    /// rejected binding is reported (not just the first) so a controls panel
    /// can annotate each offending input.
    #[must_use]
    pub fn evaluate(&self, bindings: &[BindingRequest]) -> EvaluateOutcome {
        match self.evaluate_result(bindings) {
            Ok(result) => EvaluateOutcome::Evaluated {
                evaluation: EvaluationView::from(&result),
            },
            Err(EvaluationFailure::BindingErrors(errors)) => {
                EvaluateOutcome::BindingErrors { errors }
            }
            Err(EvaluationFailure::EvalError(message)) => EvaluateOutcome::EvalError { message },
        }
    }

    /// Evaluate browser bindings, including recursively structured values.
    #[must_use]
    pub fn evaluate_browser(&self, bindings: &[BrowserBindingRequest]) -> EvaluateOutcome {
        match self.evaluate_browser_result(bindings) {
            Ok(result) => EvaluateOutcome::Evaluated {
                evaluation: EvaluationView::from(&result),
            },
            Err(EvaluationFailure::BindingErrors(errors)) => {
                EvaluateOutcome::BindingErrors { errors }
            }
            Err(EvaluationFailure::EvalError(message)) => EvaluateOutcome::EvalError { message },
        }
    }

    /// Evaluate browser bindings and render a static report from the same
    /// result. Structured bindings are rendered canonically only for the
    /// provenance reproduction command.
    #[must_use]
    pub fn evaluate_report_browser(
        &self,
        bindings: &[BrowserBindingRequest],
    ) -> EvaluateReportOutcome {
        match self.evaluate_browser_result(bindings) {
            Ok(result) => {
                let rendered = bindings
                    .iter()
                    .map(|binding| self.render_browser_binding(binding))
                    .collect::<Result<Vec<_>, _>>();
                match rendered.and_then(|bindings| self.report.render(&result, &bindings)) {
                    Ok(html) => EvaluateReportOutcome::Evaluated {
                        evaluation: EvaluationView::from(&result),
                        html,
                    },
                    Err(message) => EvaluateReportOutcome::EvalError { message },
                }
            }
            Err(EvaluationFailure::BindingErrors(errors)) => {
                EvaluateReportOutcome::BindingErrors { errors }
            }
            Err(EvaluationFailure::EvalError(message)) => {
                EvaluateReportOutcome::EvalError { message }
            }
        }
    }

    fn evaluate_browser_result(
        &self,
        bindings: &[BrowserBindingRequest],
    ) -> Result<EvalResult, EvaluationFailure> {
        let mut builder = self.prepared.binding_builder();
        let errors = bindings
            .iter()
            .filter_map(|binding| {
                bind_browser_one(&mut builder, binding)
                    .err()
                    .map(|error| BindingErrorView {
                        name: binding.name().to_string(),
                        message: error.message,
                        path: error.path,
                    })
            })
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(EvaluationFailure::BindingErrors(errors));
        }
        let row = builder
            .finish()
            .map_err(|error| EvaluationFailure::EvalError(error.to_string()))?;
        self.prepared
            .evaluate(&row)
            .map_err(|error| EvaluationFailure::EvalError(error.to_string()))
    }

    fn render_browser_binding(
        &self,
        binding: &BrowserBindingRequest,
    ) -> Result<BindingRequest, String> {
        match binding {
            BrowserBindingRequest::Expression(binding) => Ok(binding.clone()),
            BrowserBindingRequest::Structured(StructuredBindingRequest { name, value }) => {
                let declared_name = graphcal_compiler::syntax::decl_name::DeclName::try_new(name)
                    .map_err(|error| error.to_string())?;
                let port = self
                    .prepared
                    .parameter_ports()
                    .iter()
                    .find(|port| port.name() == &declared_name)
                    .ok_or_else(|| format!("unknown parameter `{name}`"))?;
                let mut expr = String::new();
                self.render_structured_value(value, port.value_schema(), &mut expr)?;
                Ok(BindingRequest {
                    name: name.clone(),
                    expr,
                })
            }
        }
    }

    fn render_structured_value(
        &self,
        value: &StructuredValueRequest,
        schema: &ModelValueSchema,
        output: &mut String,
    ) -> Result<(), String> {
        use std::fmt::Write as _;

        match (value, schema) {
            (StructuredValueRequest::Literal { expr }, _) => output.push_str(expr),
            (
                StructuredValueRequest::Algebraic {
                    definition,
                    constructor,
                    fields,
                },
                ModelValueSchema::Algebraic(expected),
            ) => {
                let graph = self.prepared.model_schema_graph();
                if graph.definition_index(expected) != Some(*definition) {
                    return Err("structured constructor has the wrong definition".to_string());
                }
                let constructor = graph
                    .definition_at(*definition)
                    .and_then(|definition| definition.constructors().get(*constructor))
                    .ok_or_else(|| "structured constructor does not exist".to_string())?;
                if fields.len() != constructor.fields().len() {
                    return Err("structured constructor has the wrong field count".to_string());
                }
                write!(output, "{}(", constructor.name()).map_err(|error| error.to_string())?;
                for (index, (field, field_schema)) in
                    fields.iter().zip(constructor.fields()).enumerate()
                {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    write!(output, "{}: ", field_schema.name())
                        .map_err(|error| error.to_string())?;
                    self.render_structured_value(field, field_schema.value(), output)?;
                }
                output.push(')');
            }
            (
                StructuredValueRequest::Indexed { entries },
                ModelValueSchema::Indexed { element, axis },
            ) => {
                let axis_name = match axis.kind() {
                    ModelIndexKind::Finite { cardinality } => format!("Fin({cardinality})"),
                    ModelIndexKind::Named { .. } | ModelIndexKind::Coordinate { .. } => axis
                        .identity()
                        .declared_resolved()
                        .and_then(|name| self.prepared.source_index_path(name))
                        .map(|path| path.to_string())
                        .ok_or_else(|| "indexed axis is not visible at the boundary".to_string())?,
                };
                write!(output, "table[{axis_name}] {{ ").map_err(|error| error.to_string())?;
                for entry in entries {
                    self.render_structured_value(entry, element, output)?;
                    output.push_str("; ");
                }
                output.push('}');
            }
            (StructuredValueRequest::Algebraic { .. }, _) => {
                return Err("unexpected algebraic structured value".to_string());
            }
            (StructuredValueRequest::Indexed { .. }, _) => {
                return Err("unexpected indexed structured value".to_string());
            }
        }
        Ok(())
    }

    /// Evaluate once and derive both the complete view and static report from
    /// that exact result.
    #[must_use]
    pub fn evaluate_report(&self, bindings: &[BindingRequest]) -> EvaluateReportOutcome {
        match self.evaluate_result(bindings) {
            Ok(result) => match self.report.render(&result, bindings) {
                Ok(html) => EvaluateReportOutcome::Evaluated {
                    evaluation: EvaluationView::from(&result),
                    html,
                },
                Err(message) => EvaluateReportOutcome::EvalError { message },
            },
            Err(EvaluationFailure::BindingErrors(errors)) => {
                EvaluateReportOutcome::BindingErrors { errors }
            }
            Err(EvaluationFailure::EvalError(message)) => {
                EvaluateReportOutcome::EvalError { message }
            }
        }
    }

    fn evaluate_result(
        &self,
        bindings: &[BindingRequest],
    ) -> Result<EvalResult, EvaluationFailure> {
        let mut builder = self.prepared.binding_builder();
        let mut errors = Vec::new();
        for binding in bindings {
            if let Err(message) = bind_one(&mut builder, binding) {
                errors.push(BindingErrorView {
                    name: binding.name.clone(),
                    message,
                    path: Vec::new(),
                });
            }
        }
        if !errors.is_empty() {
            return Err(EvaluationFailure::BindingErrors(errors));
        }
        let row = match builder.finish() {
            Ok(row) => row,
            Err(error) => {
                return Err(EvaluationFailure::EvalError(error.to_string()));
            }
        };
        self.prepared
            .evaluate(&row)
            .map_err(|error| EvaluationFailure::EvalError(error.to_string()))
    }
}

fn parameter_port_view(port: &ParameterPort, prepared: &PreparedProject) -> ParameterPortView {
    let graph = prepared.model_schema_graph();
    ParameterPortView {
        name: port.name().to_string(),
        has_default: port.has_default(),
        control: control_view(port, prepared),
        schema: value_schema_view(port.value_schema(), graph, prepared),
        definitions: graph
            .definitions()
            .iter()
            .enumerate()
            .map(|(id, definition)| AlgebraicDefinitionView {
                id,
                constructors: definition
                    .constructors()
                    .iter()
                    .enumerate()
                    .map(|(id, constructor)| ConstructorSchemaView {
                        id,
                        name: constructor.name().to_string(),
                        fields: constructor
                            .fields()
                            .iter()
                            .enumerate()
                            .map(|(id, field)| FieldSchemaView {
                                id,
                                name: field.name().to_string(),
                                schema: value_schema_view(field.value(), graph, prepared),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "model schema construction retains every algebraic reference in the same immutable arena"
)]
fn value_schema_view(
    schema: &ModelValueSchema,
    graph: &ModelSchemaGraph,
    prepared: &PreparedProject,
) -> ValueSchemaView {
    match schema {
        ModelValueSchema::Quantity(quantity) => ValueSchemaView::Quantity {
            unit: quantity
                .canonical_unit()
                .map(|unit| unit.label().to_string()),
        },
        ModelValueSchema::Complex(quantity) => ValueSchemaView::Complex {
            unit: quantity
                .canonical_unit()
                .map(|unit| unit.label().to_string()),
        },
        ModelValueSchema::Bool => ValueSchemaView::Boolean,
        ModelValueSchema::Int => ValueSchemaView::Integer,
        ModelValueSchema::Datetime(scale) => ValueSchemaView::Datetime {
            time_scale: scale.to_string(),
        },
        ModelValueSchema::Key(axis) => ValueSchemaView::Key {
            axis: index_schema_view(axis, prepared),
        },
        ModelValueSchema::Algebraic(id) => ValueSchemaView::Algebraic {
            definition: graph
                .definition_index(id)
                .expect("parameter algebraic schema must be retained in its arena"),
        },
        ModelValueSchema::Indexed { element, axis } => ValueSchemaView::Indexed {
            axis: index_schema_view(axis, prepared),
            element: Box::new(value_schema_view(element, graph, prepared)),
        },
    }
}

fn index_schema_view(axis: &ModelIndexSchema, prepared: &PreparedProject) -> IndexSchemaView {
    match axis.kind() {
        ModelIndexKind::Named { variants } => IndexSchemaView::Named {
            name: axis
                .identity()
                .declared_resolved()
                .and_then(|name| prepared.source_index_path(name))
                .map_or_else(|| axis.identity().to_string(), |name| name.to_string()),
            variants: variants.iter().map(ToString::to_string).collect(),
        },
        ModelIndexKind::Coordinate {
            coordinates_si,
            display_label,
            display_scale,
            ..
        } => IndexSchemaView::Coordinate {
            name: axis.identity().to_string(),
            labels: coordinates_si
                .iter()
                .map(|coordinate| {
                    let value = coordinate / display_scale;
                    display_label
                        .as_ref()
                        .map_or_else(|| value.to_string(), |label| format!("{value} {label}"))
                })
                .collect(),
        },
        ModelIndexKind::Finite { cardinality } => IndexSchemaView::Finite {
            cardinality: *cardinality,
        },
    }
}

fn control_view(port: &ParameterPort, prepared: &PreparedProject) -> ControlView {
    match port.value_schema() {
        ModelValueSchema::Quantity(quantity) => {
            let (lower_si, upper_si) = match port.domain() {
                Some(ParameterDomain::Quantity(bounds)) => {
                    (bounds.lower().copied(), bounds.upper().copied())
                }
                _ => (None, None),
            };
            ControlView::Quantity {
                unit: quantity
                    .canonical_unit()
                    .map(|unit| unit.label().to_string()),
                lower_si,
                upper_si,
            }
        }
        ModelValueSchema::Int => {
            let (lower, upper) = match port.domain() {
                Some(ParameterDomain::Integer(bounds)) => (
                    bounds.lower().map(ToString::to_string),
                    bounds.upper().map(ToString::to_string),
                ),
                _ => (None, None),
            };
            ControlView::Integer { lower, upper }
        }
        ModelValueSchema::Bool => ControlView::Boolean,
        ModelValueSchema::Key(index) => match index.kind() {
            ModelIndexKind::Named { variants } => index
                .identity()
                .declared_resolved()
                .and_then(|index| prepared.source_index_path(index))
                .map_or(ControlView::Expression, |path| ControlView::Select {
                    index: path.to_string(),
                    variants: variants.iter().map(ToString::to_string).collect(),
                }),
            ModelIndexKind::Coordinate { .. } | ModelIndexKind::Finite { .. } => {
                ControlView::Expression
            }
        },
        ModelValueSchema::Datetime(scale) => ControlView::Datetime {
            time_scale: scale.to_string(),
        },
        ModelValueSchema::Complex(_)
        | ModelValueSchema::Algebraic(_)
        | ModelValueSchema::Indexed { .. } => ControlView::Expression,
    }
}

/// JavaScript boundary for the prepare-once API, mirroring the transport
/// contract of the one-shot `evaluateProject` export.
#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::JsValue;
    use wasm_bindgen::prelude::wasm_bindgen;

    use super::{BrowserBindingRequest, PrepareOutcome, PreparedPlayground};
    use crate::PlaygroundOutcome;

    /// Handle to one prepared project owned by JavaScript.
    ///
    /// Call `.free()` when done. A running evaluation cannot be interrupted:
    /// to cancel, terminate the worker and prepare a fresh instance.
    #[wasm_bindgen]
    pub struct PreparedProjectHandle {
        inner: PreparedPlayground,
    }

    fn outcome_js(outcome: &PlaygroundOutcome) -> JsValue {
        crate::output::to_js(outcome).unwrap_or_else(|error| {
            JsValue::from_str(&format!("could not serialize prepare outcome: {error}"))
        })
    }

    fn serialization_error(error: &serde_wasm_bindgen::Error) -> JsValue {
        JsValue::from_str(&format!("could not serialize prepared result: {error}"))
    }

    /// Prepare an in-memory project once for repeated evaluation.
    ///
    /// The request shape and limits match `evaluateProject`. On rejection or
    /// compile error this throws the same tagged outcome object
    /// (`{status: "rejected" | "compile_error", ...}`); malformed transport
    /// shapes throw a string.
    #[wasm_bindgen(js_name = prepareProject)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen JavaScript exports receive owned JsValue handles"
    )]
    pub fn prepare_project_js(request: JsValue) -> Result<PreparedProjectHandle, JsValue> {
        use crate::js_request::{BoundedJsRequest, JsRequestBoundaryError};

        let outcome = match BoundedJsRequest::decode(&request) {
            Ok(request) => super::prepare(request.into_request()),
            Err(JsRequestBoundaryError::Rejected(error)) => PrepareOutcome::Rejected {
                error: crate::RequestErrorView::from(&error),
            },
            Err(JsRequestBoundaryError::InvalidShape(error)) => {
                return Err(JsValue::from_str(&format!(
                    "invalid playground request: {error}"
                )));
            }
        };
        match outcome {
            PrepareOutcome::Prepared(inner) => Ok(PreparedProjectHandle { inner: *inner }),
            PrepareOutcome::Rejected { error } => {
                Err(outcome_js(&PlaygroundOutcome::Rejected { error }))
            }
            PrepareOutcome::CompileError { diagnostics } => {
                Err(outcome_js(&PlaygroundOutcome::CompileError { diagnostics }))
            }
        }
    }

    /// Prepare a bounded serialized offline bundle, including its pinned plugins.
    #[wasm_bindgen(js_name = prepareReportBundle)]
    pub fn prepare_report_bundle_js(request: JsValue) -> Result<PreparedProjectHandle, JsValue> {
        let bundle = crate::js_request::decode_report_bundle(request)?;
        match super::prepare_bundle(&bundle) {
            PrepareOutcome::Prepared(inner) => Ok(PreparedProjectHandle { inner: *inner }),
            PrepareOutcome::Rejected { error } => {
                Err(outcome_js(&PlaygroundOutcome::Rejected { error }))
            }
            PrepareOutcome::CompileError { diagnostics } => {
                Err(outcome_js(&PlaygroundOutcome::CompileError { diagnostics }))
            }
        }
    }

    #[cfg(test)]
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn node_prepared_binding_failure_recovery_and_reset_preserve_si_and_notices() {
        use js_sys::{Array, Object, Reflect};
        fn set(value: &JsValue, name: &str, item: &JsValue) {
            assert!(Reflect::set(value, &JsValue::from_str(name), item).unwrap());
        }
        fn get(value: &JsValue, name: &str) -> JsValue {
            Reflect::get(value, &JsValue::from_str(name)).unwrap()
        }
        fn request(source: &str) -> JsValue {
            let file = Object::new();
            set(&file, "path", &JsValue::from_str("main.gcl"));
            set(&file, "content", &JsValue::from_str(source));
            let files = Array::new();
            files.push(&file);
            let request = Object::new();
            set(&request, "entry", &JsValue::from_str("main.gcl"));
            set(&request, "files", &files);
            request.into()
        }
        let source = "param rate: Dimensionless = 0.0; unit scaled: Length = (@rate) m; node out: Length = 12.0 m -> scaled;";
        let handle = prepare_project_js(request(source)).unwrap();
        for rate in [None, Some("3.0"), Some("0.0"), Some("6.0"), None] {
            let bindings = Array::new();
            if let Some(rate) = rate {
                let binding = Object::new();
                set(&binding, "name", &JsValue::from_str("rate"));
                set(&binding, "expr", &JsValue::from_str(rate));
                bindings.push(&binding);
            }
            let result = handle.evaluate_bindings(bindings.into()).unwrap();
            assert_eq!(
                get(&result, "status").as_string().as_deref(),
                Some("evaluated")
            );
            let evaluation = get(&result, "evaluation");
            let fresh = crate::evaluate_project_js(request(
                &source.replace("= 0.0;", &format!("= {};", rate.unwrap_or("0.0"))),
            ))
            .unwrap();
            let fresh = get(&fresh, "evaluation");
            assert_eq!(get(&evaluation, "has_errors"), get(&fresh, "has_errors"));
            let notices = Array::from(&get(&evaluation, "notices"));
            assert_eq!(
                notices.length(),
                Array::from(&get(&fresh, "notices")).length()
            );
            let output = Array::from(&get(&evaluation, "values"))
                .iter()
                .find(|value| get(value, "name").as_string().as_deref() == Some("out"))
                .unwrap();
            let outcome = get(&output, "outcome");
            assert_eq!(
                get(&outcome, "status").as_string().as_deref(),
                Some("value")
            );
            assert_eq!(
                get(&get(&outcome, "value"), "si_value").as_f64(),
                Some(12.0)
            );
        }
    }

    #[wasm_bindgen]
    impl PreparedProjectHandle {
        /// Typed entry parameter ports in source declaration order.
        #[wasm_bindgen(js_name = parameterPorts)]
        pub fn parameter_ports(&self) -> Result<JsValue, JsValue> {
            crate::output::to_js(&self.inner.ports()).map_err(|error| serialization_error(&error))
        }

        /// Evaluate with legacy `{name, expr}` or recursively structured
        /// `{name, value}` closed-value bindings; unbound parameters use their
        /// compiled defaults. Returns the tagged
        /// `EvaluateOutcome` (`evaluated` / `binding_errors` / `eval_error`).
        #[wasm_bindgen(js_name = evaluateBindings)]
        #[expect(
            clippy::needless_pass_by_value,
            reason = "wasm-bindgen JavaScript exports receive owned JsValue handles"
        )]
        pub fn evaluate_bindings(&self, bindings: JsValue) -> Result<JsValue, JsValue> {
            let bindings: Vec<BrowserBindingRequest> = serde_wasm_bindgen::from_value(bindings)
                .map_err(|error| JsValue::from_str(&format!("invalid bindings: {error}")))?;
            crate::output::to_js(&self.inner.evaluate_browser(&bindings))
                .map_err(|error| serialization_error(&error))
        }

        /// Evaluate once and return both the complete evaluation view and a
        /// static report whose chart runtime is supplied by the host.
        #[wasm_bindgen(js_name = evaluateReport)]
        pub fn evaluate_report(&self, bindings: JsValue) -> Result<JsValue, JsValue> {
            let bindings: Vec<BrowserBindingRequest> = serde_wasm_bindgen::from_value(bindings)
                .map_err(|error| JsValue::from_str(&format!("invalid bindings: {error}")))?;
            crate::output::to_js(&self.inner.evaluate_report_browser(&bindings))
                .map_err(|error| serialization_error(&error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::bindings::{
        BrowserBindingRequest, StructuredBindingRequest, StructuredValueRequest,
    };
    use crate::output::{DeclarationOutcomeView, ValueView};
    use crate::{PlaygroundFile, PlaygroundRequest};

    fn single_file(source: &str) -> PlaygroundRequest {
        PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: source.to_string(),
            }],
        }
    }

    fn prepare_ok(source: &str) -> PreparedPlayground {
        match prepare(single_file(source)) {
            PrepareOutcome::Prepared(prepared) => *prepared,
            PrepareOutcome::Rejected { error } => panic!("rejected: {error:?}"),
            PrepareOutcome::CompileError { diagnostics } => {
                panic!("compile error: {diagnostics:?}")
            }
        }
    }

    const DELTA_V: &str = "\
param dry_mass: Mass(min: 800.0 kg, max: 2000.0 kg) = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time(min: 200.0 s, max: 460.0 s) = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;
node v_exhaust: Velocity = @isp * @g0;
node delta_v: Velocity = @v_exhaust * ln((@dry_mass + @fuel_mass) / @dry_mass);
assert positive = @delta_v > 0.0 m/s;
";

    #[test]
    fn ports_expose_typed_controls() {
        let prepared = prepare_ok(DELTA_V);
        let ports = prepared.ports();
        assert_eq!(ports.len(), 3);

        assert_eq!(ports[0].name, "dry_mass");
        assert!(ports[0].has_default);
        let ControlView::Quantity {
            unit,
            lower_si,
            upper_si,
        } = &ports[0].control
        else {
            panic!("expected quantity control: {:?}", ports[0].control);
        };
        assert_eq!(unit.as_deref(), Some("kg"));
        assert_eq!(*lower_si, Some(800.0));
        assert_eq!(*upper_si, Some(2000.0));

        let ControlView::Quantity {
            lower_si, upper_si, ..
        } = &ports[1].control
        else {
            panic!("expected quantity control for fuel_mass");
        };
        assert_eq!((*lower_si, *upper_si), (None, None));
    }

    #[test]
    fn repeated_evaluation_binds_closed_values() {
        let prepared = prepare_ok(DELTA_V);

        let baseline = prepared.evaluate(&[]);
        let EvaluateOutcome::Evaluated { evaluation } = baseline else {
            panic!("expected baseline evaluation");
        };
        assert!(!evaluation.has_errors);

        let outcome = prepared.evaluate(&[BindingRequest {
            name: "isp".to_string(),
            expr: "450.0 s".to_string(),
        }]);
        let EvaluateOutcome::Evaluated { evaluation } = outcome else {
            panic!("expected evaluation with binding");
        };
        assert!(!evaluation.has_errors);
        assert_eq!(evaluation.assertions.len(), 1);
    }

    #[test]
    fn structured_bindings_select_constructors_and_validate_shape() {
        let prepared = prepare_ok(
            "pub type Choice { Amount(value: Dimensionless), Switch(enabled: Bool), }\n\
             param choice: Choice = Amount(value: 2.0);",
        );
        let ports = prepared.ports();
        assert!(matches!(
            ports[0].schema,
            ValueSchemaView::Algebraic { definition: 0 }
        ));
        assert_eq!(ports[0].definitions.len(), 1);
        assert_eq!(ports[0].definitions[0].constructors[1].name, "Switch");
        assert_eq!(
            ports[0].definitions[0].constructors[1].fields[0].name,
            "enabled"
        );

        let binding = BrowserBindingRequest::Structured(StructuredBindingRequest {
            name: "choice".to_string(),
            value: StructuredValueRequest::Algebraic {
                definition: 0,
                constructor: 1,
                fields: vec![StructuredValueRequest::Literal {
                    expr: "true".to_string(),
                }],
            },
        });
        let EvaluateOutcome::Evaluated { evaluation } =
            prepared.evaluate_browser(std::slice::from_ref(&binding))
        else {
            panic!("expected structured evaluation");
        };
        let DeclarationOutcomeView::Value {
            value: ValueView::Struct {
                type_name, fields, ..
            },
            ..
        } = &evaluation.values[0].outcome
        else {
            panic!("expected structured parameter value");
        };
        assert_eq!(type_name, "Switch");
        assert!(matches!(
            fields[0].value,
            ValueView::Bool { value: true, .. }
        ));
        let rendered = prepared.render_browser_binding(&binding).unwrap();
        assert_eq!(rendered.expr, "Switch(enabled: true)");
        assert!(matches!(
            prepared.evaluate(std::slice::from_ref(&rendered)),
            EvaluateOutcome::Evaluated { .. }
        ));
        let EvaluateReportOutcome::Evaluated { html, .. } =
            prepared.evaluate_report_browser(&[binding])
        else {
            panic!("expected a report with a structured baseline");
        };
        assert!(html.contains("Switch(enabled: true)"));

        let malformed = BrowserBindingRequest::Structured(StructuredBindingRequest {
            name: "choice".to_string(),
            value: StructuredValueRequest::Algebraic {
                definition: 0,
                constructor: 1,
                fields: Vec::new(),
            },
        });
        let EvaluateOutcome::BindingErrors { errors } = prepared.evaluate_browser(&[malformed])
        else {
            panic!("expected an addressed binding error");
        };
        assert!(errors[0].message.contains("expects 1 fields"));

        let invalid_leaf = BrowserBindingRequest::Structured(StructuredBindingRequest {
            name: "choice".to_string(),
            value: StructuredValueRequest::Algebraic {
                definition: 0,
                constructor: 1,
                fields: vec![StructuredValueRequest::Literal {
                    expr: "1".to_string(),
                }],
            },
        });
        let EvaluateOutcome::BindingErrors { errors } = prepared.evaluate_browser(&[invalid_leaf])
        else {
            panic!("expected a field-local literal error");
        };
        assert!(matches!(
            errors[0].path.as_slice(),
            [BindingPathSegmentView::Field { index: 0 }]
        ));
    }

    #[test]
    fn structured_indexed_leaf_errors_retain_the_entry_path() {
        let prepared =
            prepare_ok("param samples: Length[Fin(2)] = table[Fin(2)] { 1.0 m; 2.0 m; };");
        let binding = BrowserBindingRequest::Structured(StructuredBindingRequest {
            name: "samples".to_string(),
            value: StructuredValueRequest::Indexed {
                entries: vec![
                    StructuredValueRequest::Literal {
                        expr: "3.0 m".to_string(),
                    },
                    StructuredValueRequest::Literal {
                        expr: "4.0 s".to_string(),
                    },
                ],
            },
        });
        let EvaluateOutcome::BindingErrors { errors } = prepared.evaluate_browser(&[binding])
        else {
            panic!("expected an entry-local unit error");
        };
        assert!(matches!(
            errors[0].path.as_slice(),
            [BindingPathSegmentView::Entry { index: 1 }]
        ));
    }

    #[test]
    fn report_escapes_captions_and_tracks_bound_baseline_without_an_engine() {
        let source = "/// Speed <unsafe> & caption\nparam speed: Velocity = 1.0 m/s;\nnode doubled: Velocity = @speed * 2.0;";
        let prepared = prepare_ok(source);
        let binding = BindingRequest {
            name: "speed".to_string(),
            expr: "36.0 km/h".to_string(),
        };
        let EvaluateReportOutcome::Evaluated { evaluation, html } =
            prepared.evaluate_report(std::slice::from_ref(&binding))
        else {
            panic!("expected report");
        };
        assert!(!evaluation.has_errors);
        assert!(html.contains("Speed &lt;unsafe&gt; &amp; caption"));
        assert!(html.contains("m/s"));
        assert!(html.contains("--param"));
        assert!(html.contains("speed=36.0 km/h"));
        assert!(html.contains("sha256:"));
        assert!(!html.contains("vega.min.js"));
        assert!(!html.contains("wasm"));

        let EvaluateReportOutcome::Evaluated { html, .. } = prepared.evaluate_report(&[]) else {
            panic!("expected reset report");
        };
        assert!(!html.contains("speed=36.0 km/h"));
    }

    #[test]
    fn report_binding_rejection_is_recoverable_and_reports_all_errors() {
        let prepared = prepare_ok(DELTA_V);
        let bad = [
            BindingRequest {
                name: "isp".to_string(),
                expr: "2.0 kg".to_string(),
            },
            BindingRequest {
                name: "missing".to_string(),
                expr: "1.0".to_string(),
            },
        ];
        let EvaluateReportOutcome::BindingErrors { errors } = prepared.evaluate_report(&bad) else {
            panic!("expected binding errors");
        };
        assert_eq!(errors.len(), 2);
        assert!(matches!(
            prepared.evaluate_report(&[]),
            EvaluateReportOutcome::Evaluated { .. }
        ));
    }

    #[test]
    fn every_rejected_binding_is_reported() {
        let prepared = prepare_ok(DELTA_V);
        let outcome = prepared.evaluate(&[
            BindingRequest {
                name: "isp".to_string(),
                // Wrong dimension: a mass is not a time.
                expr: "450.0 kg".to_string(),
            },
            BindingRequest {
                name: "unknown".to_string(),
                expr: "1.0".to_string(),
            },
        ]);
        let EvaluateOutcome::BindingErrors { errors } = outcome else {
            panic!("expected binding errors, got {outcome:?}");
        };
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].name, "isp");
        assert_eq!(errors[1].name, "unknown");
    }

    #[test]
    fn out_of_domain_binding_is_rejected() {
        let prepared = prepare_ok(DELTA_V);
        let outcome = prepared.evaluate(&[BindingRequest {
            name: "isp".to_string(),
            expr: "1000.0 s".to_string(),
        }]);
        assert!(
            matches!(outcome, EvaluateOutcome::BindingErrors { .. }),
            "a value outside the declared domain must be rejected at binding time"
        );
    }

    #[test]
    fn expression_injection_is_rejected() {
        let prepared = prepare_ok(DELTA_V);
        let outcome = prepared.evaluate(&[BindingRequest {
            name: "isp".to_string(),
            expr: "@g0 * 40.0 s^2/m".to_string(),
        }]);
        assert!(
            matches!(outcome, EvaluateOutcome::BindingErrors { .. }),
            "references must be rejected: readers bind closed values only"
        );
    }

    #[test]
    fn plugin_projects_are_rejected_at_prepare_time() {
        let outcome = prepare(single_file(
            "import plugin \"graphcal:demo\" as demo { fn add(x: Dimensionless, y: Dimensionless) -> Dimensionless; }",
        ));
        let PrepareOutcome::Rejected { error } = outcome else {
            panic!("expected rejection");
        };
        assert_eq!(error.kind, crate::RequestErrorKind::PluginsUnsupported);
    }

    #[test]
    fn boolean_and_named_key_ports_map_to_controls() {
        let prepared = prepare_ok(
            "pub index Mode = { Nominal, Safe };\n\
             param enabled: Bool = true;\n\
             param mode: Key<Mode> = Mode#Nominal;\n\
             node out: Dimensionless = if @enabled { 1.0 } else { 0.0 };\n",
        );
        let ports = prepared.ports();
        assert!(matches!(ports[0].control, ControlView::Boolean));
        let ControlView::Select { index, variants } = &ports[1].control else {
            panic!("expected select control: {:?}", ports[1].control);
        };
        assert_eq!(index, "Mode");
        assert_eq!(variants, &["Nominal".to_string(), "Safe".to_string()]);
    }

    /// The hydration runtime emits these exact expression shapes from its
    /// controls: checkbox -> `true`/`false`, stepper -> integer text,
    /// select -> `Index#Variant`. Each must bind as a closed value.
    #[test]
    fn control_emitted_expressions_bind() {
        let prepared = prepare_ok(
            "pub index Mode = { Nominal, Safe };\n\
             param enabled: Bool = true;\n\
             param retries: Int(min: 0, max: 5) = 1;\n\
             param mode: Key<Mode> = Mode#Nominal;\n\
             node out: Dimensionless = if @enabled { 1.0 } else { 0.0 };\n",
        );
        let outcome = prepared.evaluate(&[
            BindingRequest {
                name: "enabled".to_string(),
                expr: "false".to_string(),
            },
            BindingRequest {
                name: "retries".to_string(),
                expr: "3".to_string(),
            },
            BindingRequest {
                name: "mode".to_string(),
                expr: "Mode#Safe".to_string(),
            },
        ]);
        let EvaluateOutcome::Evaluated { evaluation } = outcome else {
            panic!("expected evaluation, got {outcome:?}");
        };
        assert!(!evaluation.has_errors);
    }
}
