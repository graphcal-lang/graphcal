//! Strict Tenax stdio Arrow IPC schema-v2 adapter for prepared Graphcal models.
//!
//! This crate is the imperative transport shell around the transport-independent
//! model projection in `graphcal-eval`. It intentionally contains no Graphcal
//! compilation logic and no dependency on Tenax's Rust implementation.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, FixedSizeBinaryArray, Float64Array, Int64Array,
    StringArray, UInt8Array, UInt64Array,
};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use graphcal_eval::eval::{
    ModelExecutionError, ParameterBindingBuilder, PreparedProject, TenaxV2Input, TenaxV2InputKind,
    TenaxV2Model, TenaxV2RowOutcome,
};
use thiserror::Error;

const SCHEMA_VERSION_KEY: &str = "tenax.schema.version";
const SCHEMA_VERSION: &str = "2";
const BATCH_KIND_KEY: &str = "tenax.batch.kind";
const STDIO_VERSION_KEY: &str = "tenax.stdio.version";
const STDIO_VERSION: &str = "1";
const MODEL_SCHEMA_KIND: &str = "model_schema";
const REQUEST_KIND: &str = "evaluation_request";
const RESULT_KIND: &str = "evaluation_result";

const FIELD_ROLE_KEY: &str = "tenax.field.role";
const FIELD_KIND_KEY: &str = "tenax.field.kind";
const INPUT_ROLE: &str = "input";
const OUTPUT_ROLE: &str = "output";
const CONTEXT_ROLE: &str = "context";
const OUTCOME_ROLE: &str = "outcome";
const INPUT_LOWER_KEY: &str = "tenax.input.lower";
const INPUT_UPPER_KEY: &str = "tenax.input.upper";
const INPUT_CATEGORIES_KEY: &str = "tenax.input.categories";
const INPUT_UNIT_KEY: &str = "tenax.input.unit";

const EVALUATION_ID_NAME: &str = "tenax.evaluation_id";
const EVALUATION_SEED_NAME: &str = "tenax.evaluation_seed";
const OUTCOME_STATUS_NAME: &str = "tenax.outcome_status";
const FAILURE_MESSAGE_NAME: &str = "tenax.failure_message";
const EVALUATION_ID_KIND: &str = "evaluation_id";
const EVALUATION_SEED_KIND: &str = "evaluation_seed";
const OUTCOME_STATUS_KIND: &str = "outcome_status";
const FAILURE_MESSAGE_KIND: &str = "failure_message";
const EVALUATION_ID_WIDTH: i32 = 16;
const STATUS_SUCCESS: u8 = 0;
const STATUS_MODEL_ERROR: u8 = 1;
const STATUS_INVALID_INPUT: u8 = 4;

/// Fully materialized Arrow schemas for one strict Tenax v2 model.
#[derive(Debug, Clone)]
pub struct ArrowModelSchemas {
    discovery: SchemaRef,
    request: SchemaRef,
    result: SchemaRef,
}

impl ArrowModelSchemas {
    /// Build all three fixed schemas from a validated Graphcal v2 projection.
    ///
    /// # Errors
    ///
    /// Returns a metadata encoding error if a categorical domain cannot be
    /// represented as the required JSON string array.
    pub fn new(model: &TenaxV2Model) -> Result<Self, TenaxProtocolError> {
        let input_fields = model
            .inputs()
            .iter()
            .map(input_field)
            .collect::<Result<Vec<_>, _>>()?;
        let discovery_fields = input_fields
            .iter()
            .cloned()
            .chain(model.outputs().iter().map(|output| {
                Field::new(output.name().as_str(), DataType::Boolean, false)
                    .with_metadata(role_metadata(OUTPUT_ROLE))
            }))
            .collect::<Vec<_>>();
        let mut discovery_metadata = batch_metadata(MODEL_SCHEMA_KIND);
        discovery_metadata.insert(STDIO_VERSION_KEY.to_string(), STDIO_VERSION.to_string());

        let request_fields = input_fields
            .into_iter()
            .chain([evaluation_id_field(), evaluation_seed_field()])
            .collect::<Vec<_>>();
        let result_fields = model
            .outputs()
            .iter()
            .map(|output| {
                Field::new(output.name().as_str(), DataType::Boolean, true)
                    .with_metadata(role_metadata(OUTPUT_ROLE))
            })
            .chain([
                evaluation_id_field(),
                outcome_status_field(),
                failure_message_field(),
            ])
            .collect::<Vec<_>>();

        Ok(Self {
            discovery: Arc::new(Schema::new_with_metadata(
                discovery_fields,
                discovery_metadata,
            )),
            request: Arc::new(Schema::new_with_metadata(
                request_fields,
                batch_metadata(REQUEST_KIND),
            )),
            result: Arc::new(Schema::new_with_metadata(
                result_fields,
                batch_metadata(RESULT_KIND),
            )),
        })
    }

    /// Schema-only discovery stream schema.
    #[must_use]
    pub const fn discovery(&self) -> &SchemaRef {
        &self.discovery
    }

    /// Required stdin request-stream schema.
    #[must_use]
    pub const fn request(&self) -> &SchemaRef {
        &self.request
    }

    /// Fixed stdout result-stream schema.
    #[must_use]
    pub const fn result(&self) -> &SchemaRef {
        &self.result
    }
}

/// Serve one already-prepared Graphcal model over process stdin/stdout.
///
/// Stdout is reserved exclusively for Arrow IPC. Callers must route all human
/// diagnostics and logs to stderr before entering this function.
///
/// # Errors
///
/// Returns a process-scoped protocol, IPC, I/O, or Graphcal invariant error.
pub fn serve_stdio(
    project: &PreparedProject,
    model: &TenaxV2Model,
) -> Result<(), TenaxProtocolError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let input = stdin.lock();
    let output = stdout.lock();
    serve(project, model, input, output)
}

/// Serve the persistent protocol over supplied byte streams.
///
/// This generic form is used for in-memory protocol tests and alternate local
/// process shells. Startup order exactly follows Tenax stdio protocol v1.
pub fn serve<R: Read, W: Write>(
    project: &PreparedProject,
    model: &TenaxV2Model,
    input: R,
    mut output: W,
) -> Result<(), TenaxProtocolError> {
    let schemas = ArrowModelSchemas::new(model)?;

    {
        let mut discovery_writer = StreamWriter::try_new(&mut output, schemas.discovery())?;
        discovery_writer.finish()?;
    }
    output.flush()?;

    let mut result_writer = StreamWriter::try_new(output, schemas.result())?;
    result_writer.get_mut().flush()?;

    // Normative startup order requires writing both stdout headers before the
    // child opens/reads the stdin stream.
    let mut request_reader = StreamReader::try_new(input, None)?;
    validate_request_schema(model, request_reader.schema().as_ref(), schemas.request())?;

    let mut seen_ids = HashSet::new();
    for request_batch in &mut request_reader {
        let request_batch = request_batch?;
        if request_batch.num_rows() == 0 {
            return Err(TenaxProtocolError::EmptyRequestBatch);
        }
        validate_request_arrays(model, &request_batch)?;
        let evaluation_id = request_evaluation_id(model, &request_batch)?;
        if !seen_ids.insert(evaluation_id) {
            return Err(TenaxProtocolError::DuplicateEvaluationId {
                id: u128::from_be_bytes(evaluation_id),
            });
        }
        validate_constant_seed(model, &request_batch)?;
        let result = evaluate_request_batch(
            project,
            model,
            schemas.result().clone(),
            &request_batch,
            evaluation_id,
        )?;
        result_writer.write(&result)?;
        result_writer.get_mut().flush()?;
    }
    result_writer.finish()?;
    Ok(())
}

fn evaluate_request_batch(
    project: &PreparedProject,
    model: &TenaxV2Model,
    result_schema: SchemaRef,
    request: &arrow_array::RecordBatch,
    evaluation_id: [u8; EVALUATION_ID_WIDTH as usize],
) -> Result<arrow_array::RecordBatch, TenaxProtocolError> {
    let mut output_columns = (0..model.outputs().len())
        .map(|_| Vec::with_capacity(request.num_rows()))
        .collect::<Vec<Vec<Option<bool>>>>();
    let mut statuses = Vec::with_capacity(request.num_rows());
    let mut messages = Vec::with_capacity(request.num_rows());

    for row in 0..request.num_rows() {
        match bind_request_row(project.binding_builder(), model, request, row) {
            Err(message) => {
                for column in &mut output_columns {
                    column.push(None);
                }
                statuses.push(STATUS_INVALID_INPUT);
                messages.push(Some(message));
            }
            Ok(bindings) => match project.evaluate_tenax_v2_row(&bindings, model)? {
                TenaxV2RowOutcome::Success(outputs) => {
                    for (column, value) in output_columns.iter_mut().zip(outputs) {
                        column.push(Some(value));
                    }
                    statuses.push(STATUS_SUCCESS);
                    messages.push(None);
                }
                TenaxV2RowOutcome::Failure(failure) => {
                    for column in &mut output_columns {
                        column.push(None);
                    }
                    statuses.push(STATUS_MODEL_ERROR);
                    messages.push(Some(failure.message().to_string()));
                }
            },
        }
    }

    let mut columns = output_columns
        .into_iter()
        .map(|values| Arc::new(BooleanArray::from(values)) as ArrayRef)
        .collect::<Vec<_>>();
    columns.push(evaluation_id_array(evaluation_id, request.num_rows())?);
    columns.push(Arc::new(UInt8Array::from(statuses)));
    columns.push(Arc::new(StringArray::from(messages)));
    arrow_array::RecordBatch::try_new(result_schema, columns).map_err(Into::into)
}

fn bind_request_row(
    mut builder: ParameterBindingBuilder<'_>,
    model: &TenaxV2Model,
    request: &arrow_array::RecordBatch,
    row: usize,
) -> Result<graphcal_eval::eval::ParameterBindingRow, String> {
    let request_schema = request.schema();
    for (position, input) in model.inputs().iter().enumerate() {
        let field = request_schema.field(position);
        let array = request.column(position);
        let result = match input.kind() {
            TenaxV2InputKind::Continuous { scale_to_si, .. } => {
                let values = array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| invalid_array_message(field))?;
                let value = values.value(row) * scale_to_si;
                builder.bind_quantity(input.position(), value)
            }
            TenaxV2InputKind::Integer { .. } => {
                let values = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| invalid_array_message(field))?;
                builder.bind_integer(input.position(), values.value(row))
            }
            TenaxV2InputKind::Categorical { categories } => {
                let values = array
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| invalid_array_message(field))?;
                let dictionary = values
                    .values()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| invalid_array_message(field))?;
                let key = usize::try_from(values.keys().value(row)).map_err(|_| {
                    format!(
                        "input `{}` has a negative dictionary key at row {row}",
                        input.name()
                    )
                })?;
                let category = (key < dictionary.len())
                    .then(|| dictionary.value(key))
                    .ok_or_else(|| {
                        format!(
                            "input `{}` has an invalid dictionary key at row {row}",
                            input.name()
                        )
                    })?;
                let variant = categories
                    .iter()
                    .find(|candidate| candidate.as_str() == category)
                    .ok_or_else(|| {
                        format!(
                            "input `{}` has unknown category `{category}` at row {row}",
                            input.name()
                        )
                    })?;
                builder.bind_named_key(input.position(), variant)
            }
        };
        result.map_err(|error| error.to_string())?;
    }
    builder.finish().map_err(|error| error.to_string())
}

fn validate_request_schema(
    model: &TenaxV2Model,
    actual: &Schema,
    expected: &Schema,
) -> Result<(), TenaxProtocolError> {
    require_schema_metadata(actual, SCHEMA_VERSION_KEY, SCHEMA_VERSION)?;
    require_schema_metadata(actual, BATCH_KIND_KEY, REQUEST_KIND)?;
    if actual.fields().len() != expected.fields().len() {
        return Err(TenaxProtocolError::RequestSchema(format!(
            "expected {} fields, received {}",
            expected.fields().len(),
            actual.fields().len()
        )));
    }
    for (position, input) in model.inputs().iter().enumerate() {
        let actual_field = actual.field(position);
        let expected_field = expected.field(position);
        if actual_field.name() != expected_field.name()
            || actual_field.data_type() != expected_field.data_type()
            || actual_field.is_nullable()
            || actual_field
                .metadata()
                .get(FIELD_ROLE_KEY)
                .map(String::as_str)
                != Some(INPUT_ROLE)
        {
            return Err(TenaxProtocolError::RequestSchema(format!(
                "field {position} `{}` does not match input `{}`",
                actual_field.name(),
                input.name()
            )));
        }
        validate_input_metadata(input, actual_field)?;
    }
    let protocol_start = model.inputs().len();
    validate_protocol_field(
        actual.field(protocol_start),
        EVALUATION_ID_NAME,
        &DataType::FixedSizeBinary(EVALUATION_ID_WIDTH),
        false,
        CONTEXT_ROLE,
        EVALUATION_ID_KIND,
    )?;
    validate_protocol_field(
        actual.field(protocol_start + 1),
        EVALUATION_SEED_NAME,
        &DataType::UInt64,
        false,
        CONTEXT_ROLE,
        EVALUATION_SEED_KIND,
    )
}

fn validate_input_metadata(input: &TenaxV2Input, field: &Field) -> Result<(), TenaxProtocolError> {
    match input.kind() {
        TenaxV2InputKind::Continuous {
            lower, upper, unit, ..
        } => {
            compare_parsed_metadata::<f64>(field, INPUT_LOWER_KEY, lower)?;
            compare_parsed_metadata::<f64>(field, INPUT_UPPER_KEY, upper)?;
            compare_optional_metadata(field, INPUT_UNIT_KEY, unit.as_deref())
        }
        TenaxV2InputKind::Integer { lower, upper } => {
            compare_parsed_metadata::<i64>(field, INPUT_LOWER_KEY, lower)?;
            compare_parsed_metadata::<i64>(field, INPUT_UPPER_KEY, upper)?;
            compare_optional_metadata(field, INPUT_UNIT_KEY, None)
        }
        TenaxV2InputKind::Categorical { categories } => {
            let encoded = required_field_metadata(field, INPUT_CATEGORIES_KEY)?;
            let actual = serde_json::from_str::<Vec<String>>(encoded).map_err(|error| {
                TenaxProtocolError::RequestSchema(format!(
                    "field `{}` has invalid category metadata: {error}",
                    field.name()
                ))
            })?;
            let expected = categories
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(TenaxProtocolError::RequestSchema(format!(
                    "field `{}` category domain differs from discovery",
                    field.name()
                )));
            }
            compare_optional_metadata(field, INPUT_UNIT_KEY, None)
        }
    }
}

fn validate_request_arrays(
    model: &TenaxV2Model,
    batch: &arrow_array::RecordBatch,
) -> Result<(), TenaxProtocolError> {
    let batch_schema = batch.schema();
    for (position, input) in model.inputs().iter().enumerate() {
        let field = batch_schema.field(position);
        let array = batch.column(position);
        reject_nulls(field, array.as_ref())?;
        match input.kind() {
            TenaxV2InputKind::Continuous { lower, upper, .. } => {
                let values = array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| TenaxProtocolError::InvalidArray {
                        field: field.name().clone(),
                    })?;
                if let Some(row) = values
                    .values()
                    .iter()
                    .position(|value| !value.is_finite() || !(*lower..=*upper).contains(value))
                {
                    return Err(TenaxProtocolError::InputOutsideDomain {
                        field: field.name().clone(),
                        row,
                    });
                }
            }
            TenaxV2InputKind::Integer { lower, upper } => {
                let values = array.as_any().downcast_ref::<Int64Array>().ok_or_else(|| {
                    TenaxProtocolError::InvalidArray {
                        field: field.name().clone(),
                    }
                })?;
                if let Some(row) = values
                    .values()
                    .iter()
                    .position(|value| !(*lower..=*upper).contains(value))
                {
                    return Err(TenaxProtocolError::InputOutsideDomain {
                        field: field.name().clone(),
                        row,
                    });
                }
            }
            TenaxV2InputKind::Categorical { categories } => {
                let values = array
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| TenaxProtocolError::InvalidArray {
                        field: field.name().clone(),
                    })?;
                let dictionary = values
                    .values()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| TenaxProtocolError::InvalidArray {
                        field: field.name().clone(),
                    })?;
                reject_nulls(field, dictionary)?;
                let mut seen = HashSet::with_capacity(dictionary.len());
                for dictionary_position in 0..dictionary.len() {
                    let value = dictionary.value(dictionary_position);
                    if !seen.insert(value) {
                        return Err(TenaxProtocolError::DuplicateDictionaryValue {
                            field: field.name().clone(),
                            value: value.to_string(),
                        });
                    }
                    if !categories.iter().any(|category| category.as_str() == value) {
                        return Err(TenaxProtocolError::UnknownCategory {
                            field: field.name().clone(),
                            value: value.to_string(),
                        });
                    }
                }
                if let Some(row) = values.keys().values().iter().position(|key| {
                    usize::try_from(*key).map_or(true, |key| key >= dictionary.len())
                }) {
                    return Err(TenaxProtocolError::InvalidDictionaryKey {
                        field: field.name().clone(),
                        row,
                    });
                }
            }
        }
    }
    let protocol_start = model.inputs().len();
    reject_nulls(
        batch.schema().field(protocol_start),
        batch.column(protocol_start).as_ref(),
    )?;
    reject_nulls(
        batch.schema().field(protocol_start + 1),
        batch.column(protocol_start + 1).as_ref(),
    )
}

fn request_evaluation_id(
    model: &TenaxV2Model,
    batch: &arrow_array::RecordBatch,
) -> Result<[u8; EVALUATION_ID_WIDTH as usize], TenaxProtocolError> {
    let position = model.inputs().len();
    let batch_schema = batch.schema();
    let field = batch_schema.field(position);
    let ids = batch
        .column(position)
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .ok_or_else(|| TenaxProtocolError::InvalidArray {
            field: field.name().clone(),
        })?;
    let first: [u8; EVALUATION_ID_WIDTH as usize] =
        ids.value(0)
            .try_into()
            .map_err(|_| TenaxProtocolError::InvalidArray {
                field: field.name().clone(),
            })?;
    if let Some(row) = (1..ids.len()).find(|row| ids.value(*row) != first) {
        return Err(TenaxProtocolError::InconsistentEvaluationId { row });
    }
    Ok(first)
}

fn validate_constant_seed(
    model: &TenaxV2Model,
    batch: &arrow_array::RecordBatch,
) -> Result<(), TenaxProtocolError> {
    let position = model.inputs().len() + 1;
    let batch_schema = batch.schema();
    let field = batch_schema.field(position);
    let seeds = batch
        .column(position)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| TenaxProtocolError::InvalidArray {
            field: field.name().clone(),
        })?;
    let first = seeds.value(0);
    seeds
        .values()
        .iter()
        .position(|seed| *seed != first)
        .map_or_else(
            || Ok(()),
            |row| Err(TenaxProtocolError::InconsistentEvaluationSeed { row }),
        )
}

fn input_field(input: &TenaxV2Input) -> Result<Field, TenaxProtocolError> {
    let mut metadata = role_metadata(INPUT_ROLE);
    let data_type = match input.kind() {
        TenaxV2InputKind::Continuous {
            lower, upper, unit, ..
        } => {
            metadata.insert(INPUT_LOWER_KEY.to_string(), lower.to_string());
            metadata.insert(INPUT_UPPER_KEY.to_string(), upper.to_string());
            if let Some(unit) = unit {
                metadata.insert(INPUT_UNIT_KEY.to_string(), unit.clone());
            }
            DataType::Float64
        }
        TenaxV2InputKind::Integer { lower, upper } => {
            metadata.insert(INPUT_LOWER_KEY.to_string(), lower.to_string());
            metadata.insert(INPUT_UPPER_KEY.to_string(), upper.to_string());
            DataType::Int64
        }
        TenaxV2InputKind::Categorical { categories } => {
            metadata.insert(
                INPUT_CATEGORIES_KEY.to_string(),
                serde_json::to_string(
                    &categories
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                )?,
            );
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        }
    };
    Ok(Field::new(input.name().as_str(), data_type, false).with_metadata(metadata))
}

fn evaluation_id_array(
    id: [u8; EVALUATION_ID_WIDTH as usize],
    rows: usize,
) -> Result<ArrayRef, TenaxProtocolError> {
    let values = FixedSizeBinaryArray::try_from_iter((0..rows).map(|_| id))?;
    Ok(Arc::new(values))
}

fn evaluation_id_field() -> Field {
    protocol_field(
        EVALUATION_ID_NAME,
        DataType::FixedSizeBinary(EVALUATION_ID_WIDTH),
        false,
        CONTEXT_ROLE,
        EVALUATION_ID_KIND,
    )
}

fn evaluation_seed_field() -> Field {
    protocol_field(
        EVALUATION_SEED_NAME,
        DataType::UInt64,
        false,
        CONTEXT_ROLE,
        EVALUATION_SEED_KIND,
    )
}

fn outcome_status_field() -> Field {
    protocol_field(
        OUTCOME_STATUS_NAME,
        DataType::UInt8,
        false,
        OUTCOME_ROLE,
        OUTCOME_STATUS_KIND,
    )
}

fn failure_message_field() -> Field {
    protocol_field(
        FAILURE_MESSAGE_NAME,
        DataType::Utf8,
        true,
        OUTCOME_ROLE,
        FAILURE_MESSAGE_KIND,
    )
}

fn protocol_field(
    name: &'static str,
    data_type: DataType,
    nullable: bool,
    role: &'static str,
    kind: &'static str,
) -> Field {
    Field::new(name, data_type, nullable).with_metadata(HashMap::from([
        (FIELD_ROLE_KEY.to_string(), role.to_string()),
        (FIELD_KIND_KEY.to_string(), kind.to_string()),
    ]))
}

fn validate_protocol_field(
    field: &Field,
    name: &'static str,
    data_type: &DataType,
    nullable: bool,
    role: &'static str,
    kind: &'static str,
) -> Result<(), TenaxProtocolError> {
    if field.name() == name
        && field.data_type() == data_type
        && field.is_nullable() == nullable
        && field.metadata().get(FIELD_ROLE_KEY).map(String::as_str) == Some(role)
        && field.metadata().get(FIELD_KIND_KEY).map(String::as_str) == Some(kind)
    {
        Ok(())
    } else {
        Err(TenaxProtocolError::RequestSchema(format!(
            "protocol field `{}` does not match `{name}`",
            field.name()
        )))
    }
}

fn role_metadata(role: &str) -> HashMap<String, String> {
    HashMap::from([(FIELD_ROLE_KEY.to_string(), role.to_string())])
}

fn batch_metadata(kind: &str) -> HashMap<String, String> {
    HashMap::from([
        (SCHEMA_VERSION_KEY.to_string(), SCHEMA_VERSION.to_string()),
        (BATCH_KIND_KEY.to_string(), kind.to_string()),
    ])
}

fn require_schema_metadata(
    schema: &Schema,
    key: &'static str,
    expected: &'static str,
) -> Result<(), TenaxProtocolError> {
    match schema.metadata().get(key) {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(TenaxProtocolError::RequestSchema(format!(
            "metadata `{key}` is `{actual}`, expected `{expected}`"
        ))),
        None => Err(TenaxProtocolError::RequestSchema(format!(
            "metadata `{key}` is missing"
        ))),
    }
}

fn required_field_metadata<'field>(
    field: &'field Field,
    key: &'static str,
) -> Result<&'field str, TenaxProtocolError> {
    field
        .metadata()
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| {
            TenaxProtocolError::RequestSchema(format!(
                "field `{}` metadata `{key}` is missing",
                field.name()
            ))
        })
}

fn compare_parsed_metadata<T>(
    field: &Field,
    key: &'static str,
    expected: &T,
) -> Result<(), TenaxProtocolError>
where
    T: std::str::FromStr + PartialEq + std::fmt::Display,
{
    let encoded = required_field_metadata(field, key)?;
    match encoded.parse::<T>() {
        Ok(actual) if &actual == expected => Ok(()),
        _ => Err(TenaxProtocolError::RequestSchema(format!(
            "field `{}` metadata `{key}` differs from `{expected}`",
            field.name()
        ))),
    }
}

fn compare_optional_metadata(
    field: &Field,
    key: &'static str,
    expected: Option<&str>,
) -> Result<(), TenaxProtocolError> {
    if field.metadata().get(key).map(String::as_str) == expected {
        Ok(())
    } else {
        Err(TenaxProtocolError::RequestSchema(format!(
            "field `{}` metadata `{key}` differs from discovery",
            field.name()
        )))
    }
}

fn reject_nulls(field: &Field, array: &dyn Array) -> Result<(), TenaxProtocolError> {
    if array.null_count() == 0 {
        Ok(())
    } else {
        Err(TenaxProtocolError::NullValues {
            field: field.name().clone(),
            count: array.null_count(),
        })
    }
}

fn invalid_array_message(field: &Field) -> String {
    format!(
        "field `{}` cannot be read as its declared Arrow type {:?}",
        field.name(),
        field.data_type()
    )
}

/// Process-scoped stdio/Arrow protocol failure.
#[derive(Debug, Error)]
pub enum TenaxProtocolError {
    #[error(transparent)]
    Arrow(#[from] ArrowError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    ModelExecution(Box<ModelExecutionError>),
    #[error("invalid Tenax request-stream schema: {0}")]
    RequestSchema(String),
    #[error("Tenax evaluation request batches must contain at least one row")]
    EmptyRequestBatch,
    #[error("Tenax request field `{field}` contains {count} null value(s)")]
    NullValues { field: String, count: usize },
    #[error("Tenax request field `{field}` has an invalid Arrow array representation")]
    InvalidArray { field: String },
    #[error("Tenax request field `{field}` has a non-finite or out-of-domain value at row {row}")]
    InputOutsideDomain { field: String, row: usize },
    #[error("Tenax categorical field `{field}` repeats dictionary value `{value}`")]
    DuplicateDictionaryValue { field: String, value: String },
    #[error("Tenax categorical field `{field}` contains unknown value `{value}`")]
    UnknownCategory { field: String, value: String },
    #[error("Tenax categorical field `{field}` has an invalid dictionary key at row {row}")]
    InvalidDictionaryKey { field: String, row: usize },
    #[error("evaluation ID differs from the first request row at row {row}")]
    InconsistentEvaluationId { row: usize },
    #[error("evaluation seed differs from the first request row at row {row}")]
    InconsistentEvaluationSeed { row: usize },
    #[error("duplicate evaluation ID {id}")]
    DuplicateEvaluationId { id: u128 },
}

impl From<ModelExecutionError> for TenaxProtocolError {
    fn from(error: ModelExecutionError) -> Self {
        Self::ModelExecution(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use arrow_array::types::Int32Type;
    use arrow_array::{
        Array, BooleanArray, DictionaryArray, FixedSizeBinaryArray, Float64Array, Int32Array,
        Int64Array, StringArray, UInt8Array, UInt64Array,
    };
    use arrow_ipc::reader::StreamReader;
    use arrow_ipc::writer::StreamWriter;
    use graphcal_compiler::syntax::decl_name::DeclName;
    use graphcal_eval::eval::prepare_from_project;
    use graphcal_eval::loader::LoadedProject;

    use super::*;

    const MODEL: &str = r"
        pub index Mode = { A, B };
        param load: Length(min: 0.0 m, max: 10.0 m);
        param count: Int(min: 0, max: 10);
        param mode: Key<Mode>;
        pub node failure: Bool =
            @load / @load > 0.5 && @count > 0 && @mode == Mode#B;
    ";

    fn prepared_model() -> (PreparedProject, TenaxV2Model, ArrowModelSchemas) {
        let project = LoadedProject::from_source(MODEL, "model.gcl").unwrap();
        let prepared = prepare_from_project(&project).unwrap();
        let model = prepared
            .tenax_v2_model(&[DeclName::expect_valid("failure")])
            .unwrap();
        let schemas = ArrowModelSchemas::new(&model).unwrap();
        (prepared, model, schemas)
    }

    #[test]
    fn discovery_schema_is_strict_tenax_v2() {
        let (_, model, schemas) = prepared_model();
        let discovery = schemas.discovery();
        assert_eq!(
            discovery
                .metadata()
                .get(SCHEMA_VERSION_KEY)
                .map(String::as_str),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(
            discovery
                .metadata()
                .get(STDIO_VERSION_KEY)
                .map(String::as_str),
            Some(STDIO_VERSION)
        );
        assert_eq!(discovery.fields().len(), 4);
        assert_eq!(discovery.field(0).data_type(), &DataType::Float64);
        assert_eq!(
            discovery
                .field(0)
                .metadata()
                .get(INPUT_UNIT_KEY)
                .map(String::as_str),
            Some("m")
        );
        assert_eq!(discovery.field(1).data_type(), &DataType::Int64);
        assert_eq!(
            discovery.field(2).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        assert_eq!(discovery.field(3).data_type(), &DataType::Boolean);
        assert_eq!(model.inputs()[0].name().as_str(), "load");
    }

    #[test]
    fn serves_two_concatenated_streams_and_binds_dictionary_values() {
        let (prepared, model, schemas) = prepared_model();
        let input = request_stream(&schemas, [7_u8; 16]);
        let mut output = Vec::new();
        serve(&prepared, &model, Cursor::new(input), &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        {
            let mut discovery = StreamReader::try_new(&mut cursor, None).unwrap();
            assert_eq!(
                discovery
                    .schema()
                    .metadata()
                    .get(BATCH_KIND_KEY)
                    .map(String::as_str),
                Some(MODEL_SCHEMA_KIND)
            );
            assert!(discovery.next().is_none());
        }
        let mut results = StreamReader::try_new(&mut cursor, None).unwrap();
        assert_eq!(
            results
                .schema()
                .metadata()
                .get(BATCH_KIND_KEY)
                .map(String::as_str),
            Some(RESULT_KIND)
        );
        let batch = results.next().unwrap().unwrap();
        assert!(results.next().is_none());
        assert_eq!(batch.num_rows(), 3);

        let output = batch
            .column(0)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        let statuses = batch
            .column(2)
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let messages = batch
            .column(3)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();

        assert!(!output.value(0));
        assert!(output.value(1));
        assert_eq!(
            statuses.values(),
            &[STATUS_SUCCESS, STATUS_SUCCESS, STATUS_MODEL_ERROR]
        );
        assert!(output.is_null(2));
        assert!(messages.value(2).contains("division by zero"));
    }

    #[test]
    fn rejects_shared_contract_values_outside_the_discovery_domain() {
        let (prepared, model, schemas) = prepared_model();
        let input = invalid_request_stream(&schemas);
        let mut output = Vec::new();
        let error = serve(&prepared, &model, Cursor::new(input), &mut output).unwrap_err();
        assert!(matches!(
            error,
            TenaxProtocolError::InputOutsideDomain { ref field, row: 0 }
                if field == "load"
        ));
        assert!(
            !output.is_empty(),
            "startup headers are written before stdin is read"
        );
    }

    fn request_stream(schemas: &ArrowModelSchemas, evaluation_id: [u8; 16]) -> Vec<u8> {
        // The dictionary deliberately reverses lexical category order. Correct
        // adapters bind by dictionary value, never by dictionary code.
        let categories = Arc::new(StringArray::from(vec!["B", "A"]));
        let mode =
            DictionaryArray::<Int32Type>::try_new(Int32Array::from(vec![1, 0, 0]), categories)
                .unwrap();
        let ids = FixedSizeBinaryArray::try_from_iter((0..3).map(|_| evaluation_id)).unwrap();
        let batch = arrow_array::RecordBatch::try_new(
            schemas.request().clone(),
            vec![
                Arc::new(Float64Array::from(vec![4.0, 6.0, 0.0])),
                Arc::new(Int64Array::from(vec![1, 1, 1])),
                Arc::new(mode),
                Arc::new(ids),
                Arc::new(UInt64Array::from(vec![42; 3])),
            ],
        )
        .unwrap();
        let mut writer = StreamWriter::try_new(Vec::new(), schemas.request()).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    fn invalid_request_stream(schemas: &ArrowModelSchemas) -> Vec<u8> {
        let mode = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(vec![0]),
            Arc::new(StringArray::from(vec!["A"])),
        )
        .unwrap();
        let ids = FixedSizeBinaryArray::try_from_iter([[9_u8; 16]].into_iter()).unwrap();
        let batch = arrow_array::RecordBatch::try_new(
            schemas.request().clone(),
            vec![
                Arc::new(Float64Array::from(vec![f64::NAN])),
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(mode),
                Arc::new(ids),
                Arc::new(UInt64Array::from(vec![42])),
            ],
        )
        .unwrap();
        let mut writer = StreamWriter::try_new(Vec::new(), schemas.request()).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }
}
