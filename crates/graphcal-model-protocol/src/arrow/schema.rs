//! Building and validating the Arrow schemas of a session.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, Metadata, Schema, SchemaRef};
use thiserror::Error;

use super::metadata::{FieldMetadata, METADATA_KEY, SchemaMetadata, StreamKind};
use super::value_array::data_type;
use crate::label::PortName;
use crate::session::SessionSchema;
use crate::unit_scale::SiScale;
use crate::version::PROTOCOL_VERSION;

pub(super) const OUTPUTS_FIELD: &str = "outputs";
pub(super) const OUTCOME_FIELD: &str = "outcome";
pub(super) const STATUS_FIELD: &str = "status";
pub(super) const SUBJECT_FIELD: &str = "subject";
pub(super) const MESSAGE_FIELD: &str = "message";

#[expect(
    clippy::disallowed_methods,
    reason = "the metadata types hold only strings, numbers, and string-keyed maps, which always serialize"
)]
fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn metadata<T: serde::Serialize>(value: &T) -> HashMap<String, String> {
    HashMap::from([(METADATA_KEY.to_owned(), json(value))])
}

fn schema_metadata(session: &SessionSchema, stream: StreamKind) -> HashMap<String, String> {
    let inputs = session
        .inputs()
        .iter()
        .map(|input| (input.name(), input.si_scale()));
    let outputs = session
        .outputs()
        .iter()
        .map(|output| (output.name(), output.si_scale()));
    metadata(&SchemaMetadata {
        protocol_version: PROTOCOL_VERSION,
        interface_fingerprint: session.interface_fingerprint(),
        stream,
        si_scales: inputs
            .chain(outputs)
            .filter_map(|(name, scale)| scale.map(|scale| (name.clone(), scale)))
            .collect(),
    })
}

/// The unit scales a received stream schema publishes, by port.
///
/// A client reads them from the server's result-stream schema to configure
/// the same session the server resolved (see
/// [`UnitResolver`](crate::UnitResolver)), then validates the schema as a
/// whole with [`validate_schema`].
///
/// # Errors
///
/// Returns [`SchemaMismatch::SchemaMetadata`] when the schema carries no
/// valid `graphcal` metadata.
pub fn si_scales(schema: &Schema) -> Result<BTreeMap<PortName, SiScale>, SchemaMismatch> {
    schema
        .metadata()
        .get(METADATA_KEY)
        .and_then(|value| serde_json::from_str::<SchemaMetadata>(value).ok())
        .map(|metadata| metadata.si_scales)
        .ok_or_else(|| SchemaMismatch::SchemaMetadata {
            expected: "valid graphcal schema metadata".to_owned(),
            received: describe_metadata(schema.metadata()),
        })
}

/// The fields of `session`'s varied parameters in session order: one
/// non-nullable field per parameter, named after it and carrying its wire
/// type and domain as `graphcal` metadata.
#[must_use]
pub fn input_fields(session: &SessionSchema) -> Fields {
    session
        .inputs()
        .iter()
        .map(|input| {
            Field::new(input.name().as_str(), data_type(input.wire()), false).with_metadata(
                metadata(&FieldMetadata::Input {
                    wire: input.wire().clone(),
                    domain: input.domain().copied(),
                }),
            )
        })
        .collect()
}

/// The schema of `session`'s request stream.
#[must_use]
pub fn request_schema(session: &SessionSchema) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        input_fields(session),
        schema_metadata(session, StreamKind::Request),
    ))
}

/// The fields of `session`'s read outputs in session order: one nullable
/// field per output, named after it and carrying its wire type as
/// `graphcal` metadata.
#[must_use]
pub fn output_fields(session: &SessionSchema) -> Fields {
    session
        .outputs()
        .iter()
        .map(|output| {
            Field::new(output.name().as_str(), data_type(output.wire()), true).with_metadata(
                metadata(&FieldMetadata::Output {
                    wire: output.wire().clone(),
                }),
            )
        })
        .collect()
}

pub(super) fn outcome_fields() -> Fields {
    Fields::from(vec![
        Field::new(STATUS_FIELD, DataType::UInt8, false),
        Field::new(SUBJECT_FIELD, DataType::Utf8, true),
        Field::new(MESSAGE_FIELD, DataType::Utf8, true),
    ])
}

/// The schema of `session`'s result stream.
#[must_use]
pub fn result_schema(session: &SessionSchema) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new(
                OUTPUTS_FIELD,
                DataType::Struct(output_fields(session)),
                false,
            )
            .with_metadata(metadata(&FieldMetadata::Outputs)),
            Field::new(OUTCOME_FIELD, DataType::Struct(outcome_fields()), false)
                .with_metadata(metadata(&FieldMetadata::Outcome)),
        ],
        schema_metadata(session, StreamKind::Result),
    ))
}

/// How a received schema differs from the expected one.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaMismatch {
    #[error("the schema metadata differs: expected {expected}, received {received}")]
    SchemaMetadata { expected: String, received: String },
    #[error("expected {expected} fields at {path}, received {received}")]
    FieldCount {
        path: String,
        expected: usize,
        received: usize,
    },
    #[error("field {path} differs: expected {expected}, received {received}")]
    Field {
        path: String,
        expected: String,
        received: String,
    },
}

/// Check that `received` is the schema `expected`, comparing metadata as
/// JSON values so formatting and key order do not matter.
///
/// # Errors
///
/// Returns the first [`SchemaMismatch`] found.
pub fn validate_schema(expected: &Schema, received: &Schema) -> Result<(), SchemaMismatch> {
    if !metadata_matches(expected.metadata(), received.metadata()) {
        return Err(SchemaMismatch::SchemaMetadata {
            expected: describe_metadata(expected.metadata()),
            received: describe_metadata(received.metadata()),
        });
    }
    fields_match("", expected.fields(), received.fields())
}

fn fields_match(parent: &str, expected: &Fields, received: &Fields) -> Result<(), SchemaMismatch> {
    if expected.len() != received.len() {
        return Err(SchemaMismatch::FieldCount {
            path: if parent.is_empty() {
                "the top level".to_owned()
            } else {
                format!("`{parent}`")
            },
            expected: expected.len(),
            received: received.len(),
        });
    }
    for (expected, received) in expected.iter().zip(received) {
        let path = if parent.is_empty() {
            expected.name().clone()
        } else {
            format!("{parent}.{}", expected.name())
        };
        let mismatch = |expected: String, received: String| SchemaMismatch::Field {
            path: format!("`{path}`"),
            expected,
            received,
        };
        if expected.name() != received.name() {
            return Err(mismatch(
                format!("name `{}`", expected.name()),
                format!("name `{}`", received.name()),
            ));
        }
        if expected.is_nullable() != received.is_nullable() {
            return Err(mismatch(
                format!("nullable {}", expected.is_nullable()),
                format!("nullable {}", received.is_nullable()),
            ));
        }
        if !metadata_matches(expected.metadata(), received.metadata()) {
            return Err(mismatch(
                describe_metadata(expected.metadata()),
                describe_metadata(received.metadata()),
            ));
        }
        match (expected.data_type(), received.data_type()) {
            (DataType::Struct(expected), DataType::Struct(received)) => {
                fields_match(&path, expected, received)?;
            }
            (expected, received) if expected == received => {}
            (expected, received) => {
                return Err(mismatch(format!("{expected}"), format!("{received}")));
            }
        }
    }
    Ok(())
}

fn metadata_matches(expected: &Metadata, received: &Metadata) -> bool {
    let parse = |metadata: &Metadata| {
        metadata
            .get(METADATA_KEY)
            .map(|value| serde_json::from_str::<serde_json::Value>(value).ok())
    };
    match (parse(expected), parse(received)) {
        (None, None) => true,
        (Some(Some(expected)), Some(Some(received))) => expected == received,
        _ => false,
    }
}

fn describe_metadata(metadata: &Metadata) -> String {
    metadata
        .get(METADATA_KEY)
        .map_or_else(|| "no graphcal metadata".to_owned(), Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::sample_session;

    #[test]
    fn request_schema_lists_inputs_with_json_metadata() {
        let schema = request_schema(&sample_session());
        assert_eq!(schema.fields().len(), 2);
        assert_eq!(schema.field(0).data_type(), &DataType::Float64);
        let field: FieldMetadata =
            serde_json::from_str(&schema.field(0).metadata()[METADATA_KEY]).unwrap();
        assert!(matches!(
            field,
            FieldMetadata::Input {
                domain: Some(_),
                ..
            }
        ));
        let schema_metadata: SchemaMetadata =
            serde_json::from_str(&schema.metadata()[METADATA_KEY]).unwrap();
        assert_eq!(schema_metadata.stream, StreamKind::Request);
        assert_eq!(schema_metadata.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn validation_ignores_json_formatting_but_not_content() {
        let expected = request_schema(&sample_session());
        let reformatted = |value: &str| {
            let parsed: serde_json::Value = serde_json::from_str(value).unwrap();
            serde_json::to_string_pretty(&parsed).unwrap()
        };
        let fields = expected
            .fields()
            .iter()
            .map(|field| {
                field.as_ref().clone().with_metadata(HashMap::from([(
                    METADATA_KEY.to_owned(),
                    reformatted(&field.metadata()[METADATA_KEY]),
                )]))
            })
            .collect::<Vec<_>>();
        let received = Schema::new_with_metadata(fields.clone(), expected.metadata().clone());
        assert_eq!(validate_schema(&expected, &received), Ok(()));

        let mut renamed = fields;
        renamed[0] = renamed[0].clone().with_name("other");
        let received = Schema::new_with_metadata(renamed, expected.metadata().clone());
        assert!(matches!(
            validate_schema(&expected, &received),
            Err(SchemaMismatch::Field { .. })
        ));

        let received = Schema::new(expected.fields().clone());
        assert!(matches!(
            validate_schema(&expected, &received),
            Err(SchemaMismatch::SchemaMetadata { .. })
        ));
    }

    #[test]
    fn stream_schemas_publish_chosen_unit_scales() {
        use crate::interface::tests::{name, sample_interface};

        assert_eq!(
            si_scales(&result_schema(&sample_session())),
            Ok(BTreeMap::new())
        );
        let spec = crate::SessionSpec::new(vec![name("load"), name("mode")], vec![name("margin")])
            .with_unit(name("margin"), crate::UnitSpelling::new("km").unwrap());
        let kilo = SiScale::new(1000.0).unwrap();
        let session = sample_interface()
            .configure(&spec, |_: &PortName, _: &crate::UnitSpelling| Ok(kilo))
            .unwrap();
        for schema in [request_schema(&session), result_schema(&session)] {
            assert_eq!(
                si_scales(&schema),
                Ok(BTreeMap::from([(name("margin"), kilo)]))
            );
        }
        assert!(si_scales(&Schema::empty()).is_err());
    }

    #[test]
    fn result_schema_nests_outputs_and_outcome() {
        let schema = result_schema(&sample_session());
        assert_eq!(schema.fields().len(), 2);
        let DataType::Struct(outputs) = schema.field(0).data_type() else {
            panic!("outputs is a struct");
        };
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().all(|field| field.is_nullable()));
        assert_eq!(validate_schema(&schema, &schema), Ok(()));
    }
}
