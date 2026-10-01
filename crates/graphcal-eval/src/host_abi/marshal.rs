//! Marshalling of runtime values across the host-function ABI.
//!
//! Arguments are encoded in the order of the checked call node, each at the
//! ABI kind its node carries ([`ExternArgKind`]), into validated
//! [`HostArgument`]s: scalars become one [`HostScalar`], and indexed arguments
//! are flattened through [`DenseArray`] into row-major host arrays, recording
//! the typed axis each index variable binds. The expression kernel reads every
//! argument at its kind through [`ArgumentReader`], so no argument can arrive
//! at another kind, count, or rank than its checked parameter. A host result is validated by
//! [`decode_result`] and rebuilt over those recorded axes, so an indexed result
//! can only range over axes that some argument supplied.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::convert::Infallible;
use std::fmt;

use graphcal_compiler::extern_struct_result::ExternStructResult;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::function_signature::{IndexBinder, ResultKind, ScalarValueKind};
use graphcal_compiler::syntax::function_name::FnParamName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::texpr::ExternArgKind;

use super::argument::{HostArgument, HostArgumentArray, HostArrayElements};
use super::{
    HostInt, HostResultDecodeError, HostScalar, HostScalarError, ValidatedHostArrayValues,
    ValidatedHostFieldValue, ValidatedHostResult, decode_result,
};
use crate::host_fns::HostFnValue;
use crate::invariant::{Failure, Invariant};
use crate::runtime_value::dense_array::{DenseArray, DenseArrayError};
use crate::runtime_value::{IndexAxis, IndexedValue, RuntimeValue, StructFieldsError, StructValue};

/// The declared result kind of an extern function.
type ExternResultKind = ResultKind<ExternStructResult>;

/// Encoded arguments of one extern call, with the typed axes their indexed
/// arguments bound.
#[derive(Debug)]
pub struct HostArguments<'s> {
    result: &'s ExternResultKind,
    values: Vec<HostArgument>,
    bound: HashMap<IndexBinder, IndexAxis>,
}

/// Reads the value of one plugin-call argument at the ABI kind its checked
/// node carries.
///
/// The expression kernel implements it: a node read at another kind than
/// its checked type is the kernel's own invariant, checked where it reads
/// every operand.
pub trait ArgumentReader<N> {
    /// Why reading a value failed.
    type Error;

    /// The quantity `node` evaluates to.
    fn quantity(&self, node: N) -> Result<FiniteQuantity, Self::Error>;
    /// The Boolean `node` evaluates to.
    fn bool(&self, node: N) -> Result<bool, Self::Error>;
    /// The integer `node` evaluates to.
    fn int(&self, node: N) -> Result<i64, Self::Error>;
    /// The indexed value `node` evaluates to.
    fn indexed(&self, node: N) -> Result<IndexedValue<RuntimeValue>, Self::Error>;
}

/// Why the arguments of an extern call could not be encoded.
#[derive(Debug)]
pub enum EncodeError<E> {
    /// Producing the argument value failed.
    Value(E),
    /// The argument at `position` cannot cross the ABI.
    Argument {
        position: usize,
        failure: Failure<ArgumentError>,
    },
}

/// A user-facing reason one argument cannot cross the ABI.
#[derive(Debug)]
pub enum ArgumentError {
    /// An `Int` argument cannot be represented exactly as binary64.
    InexactInt {
        parameter: FnParamName,
        error: HostScalarError,
    },
    /// An `Int` array element cannot be represented exactly as binary64.
    InexactIntElement {
        parameter: FnParamName,
        index: usize,
        error: HostScalarError,
    },
}

/// A user-facing reason a host result cannot become a runtime value.
#[derive(Debug)]
pub enum ResultError {
    /// The raw result does not inhabit the declared result kind.
    Decode(HostResultDecodeError),
    /// An array result has other extents than the axes it ranges over.
    Shape {
        returned: Vec<usize>,
        expected: Vec<usize>,
    },
    /// A record result does not match its declared constructor.
    MalformedRecord(StructFieldsError),
}

impl ArgumentError {
    /// Describe this failure for a call of `function`.
    pub fn describe(&self, function: &dyn fmt::Display) -> String {
        match self {
            Self::InexactInt { parameter, error } => format!(
                "extern function `{function}` received an invalid Int for parameter `{parameter}`: {error}"
            ),
            Self::InexactIntElement {
                parameter,
                index,
                error,
            } => format!(
                "extern function `{function}` parameter `{parameter}` has an invalid Int at flat array index {index}: {error}"
            ),
        }
    }
}

impl ResultError {
    /// Describe this failure for a call of `function`.
    pub fn describe(&self, function: &dyn fmt::Display) -> String {
        match self {
            Self::Decode(error) => {
                format!("extern function `{function}` returned an invalid ABI result: {error}")
            }
            Self::Shape { returned, expected } => format!(
                "extern function `{function}` returned shape {returned:?}, expected {expected:?}"
            ),
            Self::MalformedRecord(error) => {
                format!("extern function `{function}` returned a malformed record: {error}")
            }
        }
    }
}

impl<'s> HostArguments<'s> {
    /// Encode the arguments of a checked call of a function whose declared
    /// result kind is `result`: `arguments` yields each argument's ABI kind
    /// with the node
    /// `reader` evaluates, in call order. Values are read lazily, so an
    /// earlier argument's failure is reported before a later one is evaluated.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] for a failed argument value or an argument
    /// that cannot cross the ABI.
    pub fn encode<'k, N, R: ArgumentReader<N>>(
        result: &'s ExternResultKind,
        arguments: impl IntoIterator<Item = (&'k ExternArgKind, N)>,
        reader: &R,
    ) -> Result<Self, EncodeError<R::Error>> {
        let arguments = arguments.into_iter();
        let mut encoded = Self {
            result,
            values: Vec::with_capacity(arguments.size_hint().0),
            bound: HashMap::new(),
        };
        for (position, (kind, node)) in arguments.enumerate() {
            let argument = |failure| EncodeError::Argument { position, failure };
            let value = match kind {
                ExternArgKind::Quantity { .. } => HostArgument::Scalar(HostScalar::Quantity(
                    reader.quantity(node).map_err(EncodeError::Value)?,
                )),
                ExternArgKind::Bool { .. } => HostArgument::Scalar(HostScalar::Bool(
                    reader.bool(node).map_err(EncodeError::Value)?,
                )),
                ExternArgKind::Int { param } => {
                    let value = reader.int(node).map_err(EncodeError::Value)?;
                    HostInt::try_new(value)
                        .map(|value| HostArgument::Scalar(HostScalar::Int(value)))
                        .map_err(|error| {
                            argument(Failure::Error(ArgumentError::InexactInt {
                                parameter: param.clone(),
                                error,
                            }))
                        })?
                }
                ExternArgKind::Indexed {
                    param,
                    element,
                    indexes,
                } => {
                    let indexed = reader.indexed(node).map_err(EncodeError::Value)?;
                    let (axes, array) = encode_array(param, element, &indexed).map_err(argument)?;
                    encoded
                        .bind_axes(param, indexes, &axes)
                        .map_err(|invariant| argument(invariant.into()))?;
                    HostArgument::Array(array)
                }
            };
            encoded.values.push(value);
        }
        Ok(encoded)
    }

    /// The encoded arguments, in parameter order.
    pub fn values(&self) -> &[HostArgument] {
        &self.values
    }

    /// Record the axes an array argument binds to its index variables.
    fn bind_axes(
        &mut self,
        parameter: &FnParamName,
        indexes: &NonEmpty<IndexBinder>,
        axes: &NonEmpty<IndexAxis>,
    ) -> Result<(), Invariant> {
        if axes.len() != indexes.len() {
            return Err(rank_invariant(parameter, axes.len(), indexes.len()));
        }
        for (index, axis) in indexes.iter().zip(axes) {
            match self.bound.entry(index.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(axis.clone());
                }
                Entry::Occupied(previous) if !previous.get().matches(axis) => {
                    return Err(Invariant::violated(format_args!(
                        "received inconsistent typed axes for index variable `{index}` after dimension checking"
                    )));
                }
                Entry::Occupied(_) => {}
            }
        }
        Ok(())
    }

    /// Validate `raw` against the declared result kind and rebuild it as a
    /// runtime value over the axes the arguments bound.
    ///
    /// # Errors
    ///
    /// Returns [`ResultError`] for a result the plugin should not have
    /// returned, or an [`Invariant`] when the signature and arguments
    /// disagree after dimension checking.
    pub fn decode(&self, raw: &HostFnValue) -> Result<RuntimeValue, Failure<ResultError>> {
        let result = self.result;
        let decoded = decode_result(result, raw)
            .map_err(|error| Failure::Error(ResultError::Decode(error)))?;
        match decoded {
            ValidatedHostResult::Quantity { value, .. } => Ok(RuntimeValue::Quantity(value)),
            ValidatedHostResult::Int(value) => Ok(RuntimeValue::Int(value)),
            ValidatedHostResult::Bool(value) => Ok(RuntimeValue::Bool(value)),
            ValidatedHostResult::Array(array) => {
                let axes = array.indexes().try_map_ref(|index| {
                    self.bound.get(index).cloned().ok_or_else(|| {
                        Invariant::violated(format_args!(
                            "result index variable `{index}` was not bound by any argument"
                        ))
                    })
                })?;
                let expected = axes.iter().map(IndexAxis::len).collect::<Vec<_>>();
                if array.shape() != expected {
                    return Err(Failure::Error(ResultError::Shape {
                        returned: array.shape().to_vec(),
                        expected,
                    }));
                }
                let indexed = match array.values() {
                    ValidatedHostArrayValues::Quantity { values, .. } => {
                        rebuild_array(axes, values, |value| RuntimeValue::Quantity(*value))
                    }
                    ValidatedHostArrayValues::Bool(values) => {
                        rebuild_array(axes, values, |value| RuntimeValue::Bool(*value))
                    }
                    ValidatedHostArrayValues::Int(values) => {
                        rebuild_array(axes, values, |value| RuntimeValue::Int(*value))
                    }
                }?;
                Ok(RuntimeValue::Indexed(indexed))
            }
            ValidatedHostResult::Struct(fields) => {
                let ResultKind::Struct(record) = result else {
                    return Err(
                        Invariant::violated("decoded a struct for a non-struct result").into(),
                    );
                };
                let fields = fields.iter().map(|field| {
                    let value = match field.value() {
                        ValidatedHostFieldValue::Bool(value) => RuntimeValue::Bool(*value),
                        ValidatedHostFieldValue::Int(value) => RuntimeValue::Int(*value),
                        ValidatedHostFieldValue::Quantity(value) => RuntimeValue::Quantity(*value),
                    };
                    (field.name().clone(), value)
                });
                StructValue::try_from_record(record, fields)
                    .map(RuntimeValue::Struct)
                    .map_err(|error| Failure::Error(ResultError::MalformedRecord(error)))
            }
        }
    }
}

/// Flatten an indexed argument into its typed axes and validated
/// row-major elements.
///
/// A checked argument of this parameter is a rectangular array of `element`s,
/// so a ragged array or another element is an invariant violation.
fn encode_array(
    parameter: &FnParamName,
    element: &ScalarValueKind,
    indexed: &IndexedValue<RuntimeValue>,
) -> Result<(NonEmpty<IndexAxis>, HostArgumentArray), Failure<ArgumentError>> {
    let invariant = |error: &DenseArrayError<&'static str>| -> Failure<ArgumentError> {
        Invariant::violated(match error {
            DenseArrayError::Ragged => format!(
                "array argument `{parameter}` is ragged after dimension checking"
            ),
            DenseArrayError::Element(expected) => format!(
                "array argument `{parameter}` has an element other than {expected} after dimension checking"
            ),
        })
        .into()
    };
    match element {
        ScalarValueKind::Quantity(_) => DenseArray::try_from_indexed(indexed, |leaf| match leaf {
            RuntimeValue::Quantity(value) => Ok(*value),
            _ => Err("a quantity"),
        })
        .map(|array| {
            (
                array.axes().clone(),
                HostArgumentArray::from_dense(array, HostArrayElements::Quantity),
            )
        })
        .map_err(|error| invariant(&error)),
        ScalarValueKind::Bool => DenseArray::try_from_indexed(indexed, |leaf| match leaf {
            RuntimeValue::Bool(value) => Ok(*value),
            _ => Err("a Bool"),
        })
        .map(|array| {
            (
                array.axes().clone(),
                HostArgumentArray::from_dense(array, HostArrayElements::Bool),
            )
        })
        .map_err(|error| invariant(&error)),
        ScalarValueKind::Int => {
            let integers = DenseArray::try_from_indexed(indexed, |leaf| match leaf {
                RuntimeValue::Int(value) => Ok(*value),
                _ => Err("an Int"),
            })
            .map_err(|error| invariant(&error))?;
            let mut positions = 0_usize..;
            let integers = integers.try_map(|value| {
                let index = positions.next().unwrap_or(usize::MAX);
                HostInt::try_new(value).map_err(|error| {
                    Failure::Error(ArgumentError::InexactIntElement {
                        parameter: parameter.clone(),
                        index,
                        error,
                    })
                })
            })?;
            Ok((
                integers.axes().clone(),
                HostArgumentArray::from_dense(integers, HostArrayElements::Int),
            ))
        }
    }
}

fn rank_invariant(parameter: &FnParamName, actual: usize, expected: usize) -> Invariant {
    Invariant::violated(format_args!(
        "parameter `{parameter}` received rank {actual}, expected rank {expected} after dimension checking"
    ))
}

/// Rebuild a validated row-major result over `axes`.
fn rebuild_array<T: Clone>(
    axes: NonEmpty<IndexAxis>,
    values: &[T],
    leaf: impl Fn(&T) -> RuntimeValue,
) -> Result<IndexedValue<RuntimeValue>, Invariant> {
    let dense = DenseArray::try_new(axes, values.to_vec()).map_err(Invariant::violated)?;
    dense
        .try_to_indexed(|value| Ok::<_, Infallible>(leaf(value)))
        .map_err(|never| match never {})
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::function_signature::{DimMonomial, FunctionParam, NamedResultKind};
    use graphcal_compiler::syntax::dimension::DimVarName;
    use graphcal_compiler::syntax::index_name::IndexVarName;
    use graphcal_compiler::tir::texpr::ExternSignature;

    use super::*;
    use crate::host_fns::HostArray;
    use graphcal_compiler::function_signature::ParamKind;

    type NamedKind = ParamKind<DimVarName, IndexVarName>;

    fn index_var(name: &str) -> IndexVarName {
        IndexVarName::expect_valid(name)
    }

    fn quantity() -> ScalarValueKind<DimVarName> {
        ScalarValueKind::Quantity(DimMonomial::fixed(Dimension::dimensionless()))
    }

    fn indexed(element: ScalarValueKind<DimVarName>, indexes: &[&str]) -> NamedKind {
        let indexes = indexes.iter().map(|name| index_var(name)).collect();
        ParamKind::Indexed {
            element,
            indexes: NonEmpty::try_from_vec(indexes).unwrap(),
        }
    }

    fn signature(
        index_vars: &[&str],
        params: Vec<NamedKind>,
        result: NamedResultKind<ExternStructResult>,
    ) -> ExternSignature {
        let params = params
            .into_iter()
            .enumerate()
            .map(|(position, kind)| FunctionParam {
                name: FnParamName::expect_valid(format!("p{position}")),
                kind,
            })
            .collect();
        ExternSignature::try_from_parts(
            Vec::new(),
            index_vars.iter().map(|name| index_var(name)).collect(),
            params,
            result,
        )
        .unwrap()
    }

    fn axis(name: &str, variants: &[&str]) -> IndexAxis {
        IndexAxis::named_for_test(DagId::root_in_package("marshal", "main"), name, variants)
    }

    fn rows() -> IndexAxis {
        axis("Row", &["A", "B"])
    }

    fn columns() -> IndexAxis {
        axis("Column", &["X", "Y", "Z"])
    }

    fn vector(axis: IndexAxis, values: Vec<RuntimeValue>) -> RuntimeValue {
        RuntimeValue::Indexed(IndexedValue::for_test(axis, values))
    }

    fn quantities(values: &[f64]) -> Vec<RuntimeValue> {
        values
            .iter()
            .map(|value| RuntimeValue::quantity(*value).unwrap())
            .collect()
    }

    /// Reads already evaluated values; another kind than requested is the
    /// reader's (the kernel's) failure.
    struct ValueReader;

    impl ArgumentReader<RuntimeValue> for ValueReader {
        type Error = &'static str;

        fn quantity(&self, node: RuntimeValue) -> Result<FiniteQuantity, &'static str> {
            node.expect_quantity("test").map_err(|_| "not a quantity")
        }

        fn bool(&self, node: RuntimeValue) -> Result<bool, &'static str> {
            match node {
                RuntimeValue::Bool(value) => Ok(value),
                _ => Err("not a Bool"),
            }
        }

        fn int(&self, node: RuntimeValue) -> Result<i64, &'static str> {
            match node {
                RuntimeValue::Int(value) => Ok(value),
                _ => Err("not an Int"),
            }
        }

        fn indexed(&self, node: RuntimeValue) -> Result<IndexedValue<RuntimeValue>, &'static str> {
            match node {
                RuntimeValue::Indexed(value) => Ok(value),
                _ => Err("not indexed"),
            }
        }
    }

    /// Encode `values` as the arguments of a checked call of `signature`.
    fn encode(
        signature: &ExternSignature,
        values: Vec<RuntimeValue>,
    ) -> Result<HostArguments<'_>, EncodeError<&'static str>> {
        let kinds = signature
            .params()
            .iter()
            .map(ExternArgKind::for_param)
            .collect::<Vec<_>>();
        HostArguments::encode(signature.result(), kinds.iter().zip(values), &ValueReader)
    }

    fn argument_failure<E: fmt::Debug>(error: EncodeError<E>) -> (usize, Failure<ArgumentError>) {
        match error {
            EncodeError::Argument { position, failure } => (position, failure),
            EncodeError::Value(error) => panic!("expected an argument failure, got {error:?}"),
        }
    }

    fn quantity_leaves(value: &RuntimeValue) -> Vec<f64> {
        let RuntimeValue::Indexed(indexed) = value else {
            panic!("expected an indexed result");
        };
        DenseArray::try_from_indexed(indexed, |leaf| leaf.expect_quantity("test"))
            .unwrap()
            .data()
            .iter()
            .map(|value| value.get())
            .collect()
    }

    #[test]
    fn matrices_cross_row_major_and_results_rebuild_over_bound_axes() {
        let signature = signature(
            &["I", "J"],
            vec![indexed(quantity(), &["I", "J"])],
            indexed(quantity(), &["J", "I"]).into(),
        );
        let matrix = vector(
            rows(),
            vec![
                vector(columns(), quantities(&[1.0, 2.0, 3.0])),
                vector(columns(), quantities(&[4.0, 5.0, 6.0])),
            ],
        );
        let arguments = encode(&signature, vec![matrix]).unwrap();
        let [HostArgument::Array(array)] = arguments.values() else {
            panic!("expected one array argument");
        };
        assert_eq!(array.shape(), [2, 3]);
        assert_eq!(array.elements().abi_slots(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        let transposed =
            HostArray::try_new(vec![3, 2], vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]).unwrap();
        let result = arguments.decode(&HostFnValue::Array(transposed)).unwrap();
        let RuntimeValue::Indexed(outer) = &result else {
            panic!("expected an indexed result");
        };
        assert!(outer.axis().matches(&columns()));
        let RuntimeValue::Indexed(inner) = outer.values().first() else {
            panic!("expected a nested indexed result");
        };
        assert!(inner.axis().matches(&rows()));
        assert_eq!(quantity_leaves(&result), [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);

        let wrong_shape = HostArray::try_new(vec![2, 3], vec![0.0; 6]).unwrap();
        assert!(matches!(
            arguments.decode(&HostFnValue::Array(wrong_shape)),
            Err(Failure::Error(ResultError::Shape { returned, expected }))
                if returned == [2, 3] && expected == [3, 2]
        ));
        assert!(matches!(
            arguments.decode(&HostFnValue::F64(1.0)),
            Err(Failure::Error(ResultError::Decode(
                HostResultDecodeError::ExpectedArray
            )))
        ));
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "the ABI encodes these values exactly")]
    fn scalars_and_arrays_encode_with_the_abi_policy() {
        let signature = signature(
            &["I", "J"],
            vec![
                ParamKind::Scalar(quantity()),
                ParamKind::Scalar(ScalarValueKind::Int),
                ParamKind::Scalar(ScalarValueKind::Bool),
                indexed(ScalarValueKind::Int, &["I"]),
                indexed(ScalarValueKind::Bool, &["J"]),
            ],
            ParamKind::Scalar(ScalarValueKind::Int).into(),
        );
        let arguments = encode(
            &signature,
            vec![
                RuntimeValue::quantity(2.5).unwrap(),
                RuntimeValue::Int(-3),
                RuntimeValue::Bool(true),
                vector(rows(), vec![RuntimeValue::Int(7), RuntimeValue::Int(8)]),
                vector(
                    columns(),
                    vec![
                        RuntimeValue::Bool(false),
                        RuntimeValue::Bool(true),
                        RuntimeValue::Bool(false),
                    ],
                ),
            ],
        )
        .unwrap();
        let [
            HostArgument::Scalar(quantity @ HostScalar::Quantity(_)),
            HostArgument::Scalar(int @ HostScalar::Int(_)),
            HostArgument::Scalar(flag @ HostScalar::Bool(true)),
            HostArgument::Array(ints),
            HostArgument::Array(flags),
        ] = arguments.values()
        else {
            panic!("unexpected encoded arguments");
        };
        assert_eq!(
            [quantity.abi_slot(), int.abi_slot(), flag.abi_slot()],
            [2.5, -3.0, 1.0]
        );
        assert!(matches!(ints.elements(), HostArrayElements::Int(values) if values.len() == 2));
        assert_eq!(ints.elements().abi_slots(), [7.0, 8.0]);
        assert_eq!(
            flags.elements(),
            &HostArrayElements::Bool(vec![false, true, false])
        );
        assert!(matches!(
            arguments.decode(&HostFnValue::F64(4.0)),
            Ok(RuntimeValue::Int(4))
        ));
    }

    #[test]
    fn user_facing_argument_failures_name_their_position() {
        let signature = signature(
            &["I"],
            vec![
                ParamKind::Scalar(ScalarValueKind::Int),
                indexed(ScalarValueKind::Int, &["I"]),
            ],
            indexed(ScalarValueKind::Int, &["I"]).into(),
        );
        assert!(matches!(
            encode(
                &signature,
                vec![
                    RuntimeValue::Bool(true),
                    vector(rows(), vec![RuntimeValue::Int(1), RuntimeValue::Int(2)]),
                ]
            ),
            Err(EncodeError::Value("not an Int"))
        ));

        let inexact = (1_i64 << 53) + 1;
        let (position, failure) = argument_failure(
            encode(
                &signature,
                vec![
                    RuntimeValue::Int(inexact),
                    vector(rows(), vec![RuntimeValue::Int(1), RuntimeValue::Int(2)]),
                ],
            )
            .unwrap_err(),
        );
        assert_eq!(position, 0);
        let Failure::Error(error) = failure else {
            panic!("expected a user-facing failure");
        };
        assert_eq!(
            error.describe(&"lib::f"),
            format!(
                "extern function `lib::f` received an invalid Int for parameter `p0`: integer {inexact} cannot be represented exactly as binary64"
            )
        );

        let (position, failure) = argument_failure(
            encode(
                &signature,
                vec![
                    RuntimeValue::Int(1),
                    vector(
                        rows(),
                        vec![RuntimeValue::Int(1), RuntimeValue::Int(inexact)],
                    ),
                ],
            )
            .unwrap_err(),
        );
        assert_eq!(position, 1);
        assert!(matches!(
            failure,
            Failure::Error(ArgumentError::InexactIntElement { index: 1, .. })
        ));
    }

    #[test]
    fn checker_guaranteed_facts_are_invariants() {
        let signature = signature(
            &["I"],
            vec![
                indexed(quantity(), &["I"]),
                indexed(quantity(), &["I"]),
                indexed(ScalarValueKind::Int, &["I"]),
            ],
            indexed(quantity(), &["I"]).into(),
        );
        let row_values = || vector(rows(), quantities(&[1.0, 2.0]));
        let row_ints = || vector(rows(), vec![RuntimeValue::Int(1), RuntimeValue::Int(2)]);
        let invariant = |values| {
            let (position, failure) = argument_failure(encode(&signature, values).unwrap_err());
            let Failure::Invariant(invariant) = failure else {
                panic!("expected an invariant");
            };
            (position, invariant.to_string())
        };

        assert_eq!(
            invariant(vec![
                vector(rows(), vec![row_values(), row_values()]),
                row_values(),
                row_ints(),
            ]),
            (
                0,
                "parameter `p0` received rank 2, expected rank 1 after dimension checking"
                    .to_string()
            )
        );
        assert_eq!(
            invariant(vec![
                row_values(),
                vector(columns(), quantities(&[1.0, 2.0, 3.0])),
                row_ints(),
            ]),
            (
                1,
                "received inconsistent typed axes for index variable `I` after dimension checking"
                    .to_string()
            )
        );
        assert_eq!(
            invariant(vec![
                row_values(),
                row_values(),
                vector(rows(), vec![RuntimeValue::Int(1), RuntimeValue::Bool(true)]),
            ]),
            (
                2,
                "array argument `p2` has an element other than an Int after dimension checking"
                    .to_string()
            )
        );
        let ragged = vector(
            rows(),
            vec![
                vector(columns(), vec![RuntimeValue::Int(1); 3]),
                RuntimeValue::Int(2),
            ],
        );
        assert_eq!(
            invariant(vec![row_values(), row_values(), ragged]),
            (
                2,
                "array argument `p2` is ragged after dimension checking".to_string()
            )
        );

        let arguments = encode(&signature, vec![row_values(), row_values(), row_ints()]).unwrap();
        let result = arguments
            .decode(&HostFnValue::Array(
                HostArray::vector(vec![3.0, 4.0]).unwrap(),
            ))
            .unwrap();
        assert_eq!(quantity_leaves(&result), [3.0, 4.0]);
    }
}
