//! The schema of a stored session record.
//!
//! A session record is an application's stored stream of request rows joined
//! with their output values, such as an experiment log. Its schema holds a
//! non-nullable `inputs` struct (one child per varied parameter, see
//! [`input_fields`]) and a non-nullable `outputs` struct (one nullable child
//! per read output, see [`output_fields`]), located by their `graphcal` field
//! metadata rather than by name. Any other top-level field belongs to the
//! application and carries no `graphcal` metadata. The schema metadata
//! carries the session's fingerprint and unit scales, so the record restores
//! its own [`SessionSchema`] without the model.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use arrow_schema::{DataType, Field, FieldRef, Fields, Schema};
use thiserror::Error;

use super::metadata::{FieldMetadata, METADATA_KEY, SchemaMetadata, StreamKind};
use super::schema::{
    SchemaMismatch, fields_match, input_fields, metadata, output_fields, schema_metadata,
};
use crate::interface::domain_fits;
use crate::label::{LabelError, PortName};
use crate::session::{SessionError, SessionInput, SessionOutput, SessionSchema};
use crate::unit_scale::SiScale;
use crate::version::PROTOCOL_VERSION;
use crate::wire_type::WireType;

const INPUTS_FIELD: &str = "inputs";
const OUTPUTS_FIELD: &str = "outputs";

/// The `inputs` field of `session`'s records.
#[must_use]
pub fn record_inputs_field(session: &SessionSchema) -> Field {
    Field::new(INPUTS_FIELD, DataType::Struct(input_fields(session)), false)
        .with_metadata(metadata(&FieldMetadata::Inputs))
}

/// The `outputs` field of `session`'s records.
#[must_use]
pub fn record_outputs_field(session: &SessionSchema) -> Field {
    Field::new(
        OUTPUTS_FIELD,
        DataType::Struct(output_fields(session)),
        false,
    )
    .with_metadata(metadata(&FieldMetadata::Outputs))
}

/// The `graphcal` schema metadata of `session`'s records.
#[must_use]
pub fn record_metadata(session: &SessionSchema) -> HashMap<String, String> {
    schema_metadata(session, StreamKind::Record)
}

/// Why a schema is not a session record schema.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RecordSchemaError {
    #[error("the schema carries no valid graphcal metadata")]
    SchemaMetadata,
    #[error("the schema describes a {received:?} stream, not a record")]
    Stream { received: StreamKind },
    #[error("the record was written with protocol version {received}; expected {expected}")]
    ProtocolVersion { expected: u32, received: u32 },
    #[error("top-level field `{field}` carries invalid graphcal metadata")]
    FieldMetadata { field: String },
    #[error("top-level field `{field}` has a graphcal role a record cannot hold at the top level")]
    UnexpectedRole { field: String },
    #[error("the record has {count} {part:?} structs; expected exactly one")]
    PartCount { part: RecordPart, count: usize },
    #[error("top-level field `{field}` is not a struct")]
    NotStruct { field: String },
    #[error("field `{field}` is not a valid port name: {source}")]
    PortName { field: String, source: LabelError },
    #[error("port `{port}` does not carry the graphcal metadata of its role")]
    PortMetadata { port: PortName },
    #[error("parameter `{port}` declares a domain its wire type cannot have")]
    DomainKindMismatch { port: PortName },
    #[error("port `{port}` has a unit scale but carries no quantities")]
    ScaleForNonQuantity { port: PortName },
    #[error("a unit scale is recorded for `{port}`, which the record does not hold")]
    ScaleForUnknownPort { port: PortName },
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Mismatch(#[from] SchemaMismatch),
}

/// A struct a session record holds once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordPart {
    /// The varied parameter values.
    Inputs,
    /// The read output values.
    Outputs,
}

/// A decoded session record schema: the session it records and where its
/// `inputs` and `outputs` structs are.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordSchema {
    session: Arc<SessionSchema>,
    inputs: usize,
    outputs: usize,
}

impl RecordSchema {
    /// Restore the session recorded by `schema`.
    ///
    /// The `inputs` and `outputs` structs must each appear exactly once, and
    /// must be exactly the fields [`record_inputs_field`] and
    /// [`record_outputs_field`] give for the restored session. Fields without
    /// `graphcal` metadata are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`RecordSchemaError`] when the schema metadata is missing,
    /// describes another stream or protocol version, or names an unknown
    /// port; when the structs are missing, repeated, or malformed; or when a
    /// port's metadata is invalid or inconsistent with its Arrow type.
    pub fn decode(schema: &Schema) -> Result<Self, RecordSchemaError> {
        let schema_metadata = schema
            .metadata()
            .get(METADATA_KEY)
            .and_then(|value| serde_json::from_str::<SchemaMetadata>(value).ok())
            .ok_or(RecordSchemaError::SchemaMetadata)?;
        if schema_metadata.stream != StreamKind::Record {
            return Err(RecordSchemaError::Stream {
                received: schema_metadata.stream,
            });
        }
        if schema_metadata.protocol_version != PROTOCOL_VERSION {
            return Err(RecordSchemaError::ProtocolVersion {
                expected: PROTOCOL_VERSION,
                received: schema_metadata.protocol_version,
            });
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for (position, field) in schema.fields().iter().enumerate() {
            match field_role(field)? {
                None => {}
                Some(FieldMetadata::Inputs) => inputs.push(position),
                Some(FieldMetadata::Outputs) => outputs.push(position),
                Some(_) => {
                    return Err(RecordSchemaError::UnexpectedRole {
                        field: field.name().clone(),
                    });
                }
            }
        }
        let inputs = single(&inputs, RecordPart::Inputs)?;
        let outputs = single(&outputs, RecordPart::Outputs)?;

        let mut scales = schema_metadata.si_scales;
        let session_inputs = children(schema.field(inputs))?
            .iter()
            .map(|field| {
                let (port, role) = port(field)?;
                let FieldMetadata::Input { wire, domain } = role else {
                    return Err(RecordSchemaError::PortMetadata { port });
                };
                if domain.is_some_and(|domain| !domain_fits(&wire, &domain)) {
                    return Err(RecordSchemaError::DomainKindMismatch { port });
                }
                let si_scale = take_scale(&mut scales, &port, &wire)?;
                Ok(SessionInput::restored(port, wire, domain, si_scale))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let session_outputs = children(schema.field(outputs))?
            .iter()
            .map(|field| {
                let (port, role) = port(field)?;
                let FieldMetadata::Output { wire } = role else {
                    return Err(RecordSchemaError::PortMetadata { port });
                };
                let si_scale = take_scale(&mut scales, &port, &wire)?;
                Ok(SessionOutput::restored(port, wire, si_scale))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(port) = scales.into_keys().next() {
            return Err(RecordSchemaError::ScaleForUnknownPort { port });
        }
        let session = SessionSchema::restore(
            schema_metadata.interface_fingerprint,
            session_inputs,
            session_outputs,
        )?;

        // Every port is now known; the stored fields must be exactly the
        // fields the restored session gives, including types and nullability.
        let expected = Fields::from(vec![
            record_inputs_field(&session),
            record_outputs_field(&session),
        ]);
        let received = Fields::from(vec![
            schema.field(inputs).clone(),
            schema.field(outputs).clone(),
        ]);
        fields_match("", &expected, &received)?;
        Ok(Self {
            session: Arc::new(session),
            inputs,
            outputs,
        })
    }

    /// The recorded session.
    #[must_use]
    pub const fn session(&self) -> &Arc<SessionSchema> {
        &self.session
    }

    /// The position of the `inputs` struct among the schema's fields.
    #[must_use]
    pub const fn inputs_column(&self) -> usize {
        self.inputs
    }

    /// The position of the `outputs` struct among the schema's fields.
    #[must_use]
    pub const fn outputs_column(&self) -> usize {
        self.outputs
    }
}

/// The `graphcal` role of a top-level field, or `None` for an application
/// field.
fn field_role(field: &Field) -> Result<Option<FieldMetadata>, RecordSchemaError> {
    field
        .metadata()
        .get(METADATA_KEY)
        .map(|value| {
            serde_json::from_str(value).map_err(|_| RecordSchemaError::FieldMetadata {
                field: field.name().clone(),
            })
        })
        .transpose()
}

const fn single(positions: &[usize], part: RecordPart) -> Result<usize, RecordSchemaError> {
    match positions {
        [position] => Ok(*position),
        _ => Err(RecordSchemaError::PartCount {
            part,
            count: positions.len(),
        }),
    }
}

fn children(field: &Field) -> Result<&Fields, RecordSchemaError> {
    match field.data_type() {
        DataType::Struct(fields) => Ok(fields),
        _ => Err(RecordSchemaError::NotStruct {
            field: field.name().clone(),
        }),
    }
}

/// The name and `graphcal` metadata of a port field.
fn port(field: &FieldRef) -> Result<(PortName, FieldMetadata), RecordSchemaError> {
    let port =
        PortName::new(field.name().as_str()).map_err(|source| RecordSchemaError::PortName {
            field: field.name().clone(),
            source,
        })?;
    let role = field
        .metadata()
        .get(METADATA_KEY)
        .and_then(|value| serde_json::from_str::<FieldMetadata>(value).ok());
    match role {
        Some(role) => Ok((port, role)),
        None => Err(RecordSchemaError::PortMetadata { port }),
    }
}

fn take_scale(
    scales: &mut BTreeMap<PortName, SiScale>,
    port: &PortName,
    wire: &WireType,
) -> Result<Option<SiScale>, RecordSchemaError> {
    let scale = scales.remove(port);
    if scale.is_some() && wire.quantity().is_none() {
        return Err(RecordSchemaError::ScaleForNonQuantity { port: port.clone() });
    }
    Ok(scale)
}

#[cfg(test)]
mod tests {
    use arrow_schema::Field;

    use super::*;
    use crate::interface::tests::{name, sample_interface};
    use crate::session::tests::sample_session;
    use crate::unit_scale::SiScale;
    use crate::{SessionSpec, UnitSpelling};

    /// A record of `session` with an application field before its structs.
    fn record(session: &SessionSchema) -> Schema {
        Schema::new_with_metadata(
            vec![
                Field::new("row", DataType::UInt64, false),
                record_inputs_field(session),
                record_outputs_field(session),
            ],
            record_metadata(session),
        )
    }

    #[test]
    fn records_restore_their_session() {
        let session = sample_session();
        let decoded = RecordSchema::decode(&record(&session)).unwrap();
        assert_eq!(decoded.session().as_ref(), &session);
        assert_eq!(decoded.inputs_column(), 1);
        assert_eq!(decoded.outputs_column(), 2);
    }

    #[test]
    fn records_restore_chosen_unit_scales() {
        let spec = SessionSpec::new(vec![name("load"), name("mode")], vec![name("margin")])
            .with_unit(name("load"), UnitSpelling::new("km").unwrap());
        let kilo = SiScale::new(1000.0).unwrap();
        let session = sample_interface()
            .configure(&spec, |_: &PortName, _: &UnitSpelling| Ok(kilo))
            .unwrap();
        let decoded = RecordSchema::decode(&record(&session)).unwrap();
        assert_eq!(decoded.session().as_ref(), &session);
    }

    #[test]
    fn rejects_other_streams_and_missing_structs() {
        let session = sample_session();
        let request = super::super::request_schema(&session);
        assert!(matches!(
            RecordSchema::decode(&request),
            Err(RecordSchemaError::Stream {
                received: StreamKind::Request
            })
        ));
        assert!(matches!(
            RecordSchema::decode(&Schema::empty()),
            Err(RecordSchemaError::SchemaMetadata)
        ));

        let without_inputs = Schema::new_with_metadata(
            vec![record_outputs_field(&session)],
            record_metadata(&session),
        );
        assert!(matches!(
            RecordSchema::decode(&without_inputs),
            Err(RecordSchemaError::PartCount {
                part: RecordPart::Inputs,
                count: 0
            })
        ));

        let doubled = Schema::new_with_metadata(
            vec![
                record_inputs_field(&session),
                record_outputs_field(&session),
                record_outputs_field(&session).with_name("again"),
            ],
            record_metadata(&session),
        );
        assert!(matches!(
            RecordSchema::decode(&doubled),
            Err(RecordSchemaError::PartCount {
                part: RecordPart::Outputs,
                count: 2
            })
        ));
    }

    #[test]
    fn rejects_inconsistent_ports() {
        let session = sample_session();
        let schema = record(&session);
        let with_inputs = |inputs: Field| {
            Schema::new_with_metadata(
                vec![
                    Field::new("row", DataType::UInt64, false),
                    inputs,
                    record_outputs_field(&session),
                ],
                schema.metadata().clone(),
            )
        };
        let DataType::Struct(ports) = schema.field(1).data_type() else {
            panic!("inputs is a struct");
        };

        let mut duplicated = ports.iter().cloned().collect::<Vec<_>>();
        duplicated[1] = Arc::new(duplicated[0].as_ref().clone());
        let duplicated = schema
            .field(1)
            .clone()
            .with_data_type(DataType::Struct(duplicated.into()));
        assert!(matches!(
            RecordSchema::decode(&with_inputs(duplicated)),
            Err(RecordSchemaError::Session(
                SessionError::DuplicateInput { .. }
            ))
        ));

        let mut retyped = ports.iter().cloned().collect::<Vec<_>>();
        retyped[0] = Arc::new(retyped[0].as_ref().clone().with_data_type(DataType::Int64));
        let retyped = schema
            .field(1)
            .clone()
            .with_data_type(DataType::Struct(retyped.into()));
        assert!(matches!(
            RecordSchema::decode(&with_inputs(retyped)),
            Err(RecordSchemaError::Mismatch(_))
        ));

        let mut unlabeled = ports.iter().cloned().collect::<Vec<_>>();
        unlabeled[0] = Arc::new(unlabeled[0].as_ref().clone().with_metadata(HashMap::new()));
        let unlabeled = schema
            .field(1)
            .clone()
            .with_data_type(DataType::Struct(unlabeled.into()));
        assert!(matches!(
            RecordSchema::decode(&with_inputs(unlabeled)),
            Err(RecordSchemaError::PortMetadata { .. })
        ));
    }

    #[test]
    fn rejects_scales_for_unknown_ports() {
        let session = sample_session();
        let mut metadata: SchemaMetadata =
            serde_json::from_str(&record_metadata(&session)[METADATA_KEY]).unwrap();
        metadata
            .si_scales
            .insert(name("elsewhere"), SiScale::new(2.0).unwrap());
        let schema = Schema::new_with_metadata(
            vec![
                record_inputs_field(&session),
                record_outputs_field(&session),
            ],
            HashMap::from([(
                METADATA_KEY.to_owned(),
                serde_json::to_string(&metadata).unwrap(),
            )]),
        );
        assert!(matches!(
            RecordSchema::decode(&schema),
            Err(RecordSchemaError::ScaleForUnknownPort { .. })
        ));
    }
}
