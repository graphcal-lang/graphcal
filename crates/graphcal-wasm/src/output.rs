use std::collections::BTreeMap;

use graphcal_compiler::dimension::BaseDimId;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_eval::eval::{
    AssertResult, DeclType, DisplayProjectionError, DisplayUnit, EvalOutputView, EvalResult,
    NodeError, Value, format_epoch_with_tz, format_number, quantity_display_value,
};
use serde::Serialize;

/// Browser-facing successful compilation/evaluation result.
#[derive(Debug, Clone, Serialize)]
pub struct EvaluationView {
    pub compiler_version: &'static str,
    pub values: Vec<DeclarationView>,
    pub assertions: Vec<AssertionView>,
    pub figures: Vec<FigureView>,
    pub notices: Vec<NoticeView>,
    pub has_errors: bool,
}

/// One renderable figure: a Vega-Lite spec ready for `vegaEmbed`.
#[derive(Debug, Clone, Serialize)]
pub struct FigureView {
    pub name: String,
    pub spec: serde_json::Value,
}

impl From<&EvalResult> for EvaluationView {
    fn from(result: &EvalResult) -> Self {
        let values = result
            .output_values(EvalOutputView::Surface)
            .map(|(name, outcome, declaration_kind)| DeclarationView {
                name: name.to_string(),
                declaration_kind: DeclarationKindView::from(*declaration_kind),
                outcome: DeclarationOutcomeView::from_result(outcome, &result.base_dim_symbols),
            })
            .collect();

        let assertions = result
            .assertions
            .iter()
            .map(|(name, assertion, _)| AssertionView {
                name: name.to_string(),
                outcome: AssertionOutcomeView::from(assertion),
                affected_declarations: result
                    .assumes_map
                    .get(name)
                    .map(|affected| affected.iter().map(ToString::to_string).collect())
                    .unwrap_or_default(),
            })
            .collect();

        let mut notices = Vec::new();
        let figures = match graphcal_report::vega::build_figures(
            &result.plots,
            &result.figures,
            &result.layers,
        ) {
            Ok(figures) => figures
                .into_iter()
                .map(|figure| FigureView {
                    name: figure.name,
                    spec: figure.spec,
                })
                .collect(),
            Err(error) => {
                // Resolution rejects unknown plot references at compile
                // time (#843): reaching this is a compiler bug, reported
                // loudly rather than silently dropping figures.
                notices.push(NoticeView::InternalError {
                    message: error.to_string(),
                });
                Vec::new()
            }
        };
        notices.extend(
            result
                .plot_errors
                .iter()
                .map(|error| NoticeView::PlotError {
                    name: error.name.to_string(),
                    message: error.message.clone(),
                }),
        );

        Self {
            compiler_version: env!("CARGO_PKG_VERSION"),
            values,
            assertions,
            figures,
            notices,
            has_errors: result.has_errors(),
        }
    }
}

/// One evaluated declaration in source order.
#[derive(Debug, Clone, Serialize)]
pub struct DeclarationView {
    pub name: String,
    pub declaration_kind: DeclarationKindView,
    pub outcome: DeclarationOutcomeView,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclarationKindView {
    Const,
    Param,
    Node,
}

impl From<DeclType> for DeclarationKindView {
    fn from(kind: DeclType) -> Self {
        match kind {
            DeclType::Const => Self::Const,
            DeclType::Param => Self::Param,
            DeclType::Node => Self::Node,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeclarationOutcomeView {
    Value { value: ValueView },
    Error { error: NodeErrorView },
}

impl DeclarationOutcomeView {
    fn from_result(
        result: &Result<Value, NodeError>,
        symbols: &BTreeMap<BaseDimId, String>,
    ) -> Self {
        match result {
            Ok(value) => ValueView::from_value(value, symbols).map_or_else(
                |error| Self::Error {
                    error: NodeErrorView::EvaluationFailed {
                        message: error.to_string(),
                    },
                },
                |value| Self::Value { value },
            ),
            Err(error) => Self::Error {
                error: NodeErrorView::from(error),
            },
        }
    }
}

/// Recursive, lossless-enough presentation model for a Graphcal runtime value.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueView {
    Quantity {
        display: String,
        value: f64,
        si_value: f64,
        unit: Option<String>,
    },
    Complex {
        display: String,
        real: f64,
        imaginary: f64,
        si_real: f64,
        si_imaginary: f64,
        unit: Option<String>,
    },
    Bool {
        display: String,
        value: bool,
    },
    Int {
        display: String,
        decimal: String,
    },
    Label {
        display: String,
        index: String,
        variant: String,
    },
    Struct {
        display: String,
        type_name: String,
        fields: Vec<StructFieldView>,
    },
    Indexed {
        display: String,
        index: String,
        entries: Vec<IndexedEntryView>,
    },
    Datetime {
        display: String,
        time_scale: String,
        display_timezone: Option<String>,
    },
}

impl ValueView {
    fn from_value(
        value: &Value,
        symbols: &BTreeMap<BaseDimId, String>,
    ) -> Result<Self, DisplayProjectionError> {
        Ok(match value {
            Value::Quantity {
                si_value,
                display_unit,
                ..
            } => Self::from_quantity(
                *si_value,
                display_unit.as_ref(),
                value.display_label(symbols),
            )?,
            Value::Complex {
                si_value,
                display_unit,
                ..
            } => Self::from_complex(
                si_value.re(),
                si_value.im(),
                display_unit.as_ref(),
                value.display_label(symbols),
            )?,
            Value::Bool(inner) => Self::Bool {
                display: inner.to_string(),
                value: *inner,
            },
            Value::Int(inner) => {
                let decimal = inner.to_string();
                Self::Int {
                    display: decimal.clone(),
                    decimal,
                }
            }
            Value::Label {
                index_name,
                variant,
            } => {
                let index = index_name.display_name().as_str().to_string();
                let variant = variant.as_str().to_string();
                Self::Label {
                    display: format!("{index}.{variant}"),
                    index,
                    variant,
                }
            }
            Value::Struct { type_name, fields } => {
                let type_name = type_name.as_str().to_string();
                Self::Struct {
                    display: type_name.clone(),
                    type_name,
                    fields: fields
                        .iter()
                        .map(|(name, field_value)| {
                            Self::from_value(field_value, symbols).map(|value| StructFieldView {
                                name: name.as_str().to_string(),
                                value,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
            Value::Indexed {
                index_name,
                entries,
                ..
            } => {
                let index = index_name.display_name().as_str().to_string();
                Self::Indexed {
                    display: format!("{index}[{}]", entries.len()),
                    index,
                    entries: entries
                        .iter()
                        .map(|(key, entry_value)| {
                            Self::from_value(entry_value, symbols).map(|entry_value| {
                                IndexedEntryView {
                                    key: IndexEntryKeyView::from(key),
                                    display_key: value.indexed_entry_display_name(key),
                                    value: entry_value,
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
            Value::Datetime {
                epoch,
                display_tz,
                time_scale,
                time_zones,
            } => Self::Datetime {
                display: format_epoch_with_tz(epoch, display_tz.as_ref(), time_zones),
                time_scale: time_scale.to_string(),
                display_timezone: display_tz.as_ref().map(|tz| tz.as_str().to_string()),
            },
        })
    }

    fn from_quantity(
        si_value: f64,
        display_unit: Option<&DisplayUnit>,
        unit: Option<String>,
    ) -> Result<Self, DisplayProjectionError> {
        let value = quantity_display_value(si_value, display_unit)?;
        Ok(Self::Quantity {
            display: display_number_with_unit(value, unit.as_deref()),
            value,
            si_value,
            unit,
        })
    }

    fn from_complex(
        si_real: f64,
        si_imaginary: f64,
        display_unit: Option<&DisplayUnit>,
        unit: Option<String>,
    ) -> Result<Self, DisplayProjectionError> {
        let real = quantity_display_value(si_real, display_unit)?;
        let imaginary = quantity_display_value(si_imaginary, display_unit)?;
        Ok(Self::Complex {
            display: display_complex_with_unit(real, imaginary, unit.as_deref()),
            real,
            imaginary,
            si_real,
            si_imaginary,
            unit,
        })
    }
}

fn display_number_with_unit(value: f64, unit: Option<&str>) -> String {
    let number = format_number(value);
    match unit {
        Some(unit) => format!("{number} {unit}"),
        None => number,
    }
}

fn display_complex_with_unit(real: f64, imaginary: f64, unit: Option<&str>) -> String {
    let sign = if imaginary.is_sign_negative() {
        "-"
    } else {
        "+"
    };
    let number = format!(
        "{} {sign} {}i",
        format_number(real),
        format_number(imaginary.abs())
    );
    match unit {
        Some(unit) => format!("{number} {unit}"),
        None => number,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StructFieldView {
    pub name: String,
    pub value: ValueView,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexedEntryView {
    pub key: IndexEntryKeyView,
    pub display_key: String,
    pub value: ValueView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexEntryKeyView {
    Named { name: String },
    Position { decimal: String },
}

impl From<&IndexEntryKey> for IndexEntryKeyView {
    fn from(key: &IndexEntryKey) -> Self {
        match key {
            IndexEntryKey::Named(name) => Self::Named {
                name: name.as_str().to_string(),
            },
            IndexEntryKey::Position(position) => Self::Position {
                decimal: position.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeErrorView {
    EvaluationFailed { message: String },
    DependencyFailed { failed_dependencies: Vec<String> },
}

impl From<&NodeError> for NodeErrorView {
    fn from(error: &NodeError) -> Self {
        match error {
            NodeError::EvalFailed { message } => Self::EvaluationFailed {
                message: message.clone(),
            },
            NodeError::DependencyFailed { failed_deps } => Self::DependencyFailed {
                failed_dependencies: failed_deps
                    .iter()
                    .map(|name| name.as_str().to_string())
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssertionView {
    pub name: String,
    pub outcome: AssertionOutcomeView,
    pub affected_declarations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AssertionOutcomeView {
    Pass,
    Fail { message: String },
    Error { message: String },
}

impl From<&AssertResult> for AssertionOutcomeView {
    fn from(result: &AssertResult) -> Self {
        match result {
            AssertResult::Pass => Self::Pass,
            AssertResult::Fail { message } => Self::Fail {
                message: message.clone(),
            },
            AssertResult::Error { message } => Self::Error {
                message: message.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoticeView {
    PlotError {
        name: String,
        message: String,
    },
    /// A compiler invariant failed while assembling browser output. Shown to
    /// the user instead of silently dropping the affected output.
    InternalError {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use graphcal_eval::eval::compile_and_eval;

    use super::*;

    #[test]
    fn plots_render_as_vega_lite_figures() {
        let result = compile_and_eval(
            "node y: Dimensionless = 2.0;\nplot p = { mark: line, encode: { x: 1.0, y: @y } };",
        )
        .unwrap();
        let view = EvaluationView::from(&result);
        assert_eq!(view.figures.len(), 1);
        assert_eq!(view.figures[0].name, "p");
        assert_eq!(
            view.figures[0].spec["$schema"],
            serde_json::json!("https://vega.github.io/schema/vega-lite/v5.json")
        );
        assert!(view.notices.is_empty());
    }

    #[test]
    fn indexed_values_remain_structured() {
        let result = compile_and_eval(
            "index Axis = { A, B };\nnode xs: Dimensionless[Axis] = { Axis#A: 1.0, Axis#B: 2.0 };",
        )
        .unwrap();
        let view = EvaluationView::from(&result);
        let value = &view.values[0].outcome;
        assert!(matches!(
            value,
            DeclarationOutcomeView::Value {
                value: ValueView::Indexed { entries, .. }
            } if entries.len() == 2 && entries[0].display_key == "A"
        ));
    }

    #[test]
    fn non_indexed_transport_variants_are_explicit() {
        let result = compile_and_eval(
            "index Axis = { A, B };\ntype Pair { Pair(left: Dimensionless, right: Bool), }\nnode distance: Length = 3.0 m;\nnode flag: Bool = true;\nnode count: Int = 3;\nnode selected: Key<Axis> = Axis#A;\nnode impedance: Complex<Length> = complex(3.0 m, 4.0 m);\nnode pair: Pair = Pair(left: 1.0, right: true);\nnode instant: Datetime = datetime(\"2024-11-05T12:00:00Z\");",
        )
        .unwrap();
        let view = EvaluationView::from(&result);

        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Quantity { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Bool { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Int { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Label { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Complex { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Struct { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Value {
                value: ValueView::Datetime { .. }
            }
        )));
    }

    #[test]
    fn runtime_and_assertion_failures_remain_structured() {
        let result = compile_and_eval(
            "param divisor: Dimensionless = 0.0;\nnode quotient: Dimensionless = 1.0 / @divisor;\nnode dependent: Dimensionless = @quotient + 1.0;\nassert always_passes = 1.0 == 1.0;\nassert expected_two = @divisor == 2.0;\nassert quotient_is_zero = @quotient == 0.0;",
        )
        .unwrap();
        let view = EvaluationView::from(&result);

        assert!(view.has_errors);
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Error {
                error: NodeErrorView::EvaluationFailed { .. }
            }
        )));
        assert!(view.values.iter().any(|declaration| matches!(
            &declaration.outcome,
            DeclarationOutcomeView::Error {
                error: NodeErrorView::DependencyFailed { .. }
            }
        )));
        assert!(
            view.assertions
                .iter()
                .any(|assertion| matches!(&assertion.outcome, AssertionOutcomeView::Pass))
        );
        assert!(
            view.assertions
                .iter()
                .any(|assertion| matches!(&assertion.outcome, AssertionOutcomeView::Fail { .. }))
        );
        assert!(
            view.assertions
                .iter()
                .any(|assertion| matches!(&assertion.outcome, AssertionOutcomeView::Error { .. }))
        );
    }
}
