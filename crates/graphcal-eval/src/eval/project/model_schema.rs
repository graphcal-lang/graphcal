//! Transport-independent graph schema for concrete Graphcal values.
//!
//! Algebraic values are represented as typed references into an immutable
//! definition arena. Recursive and mutually recursive types therefore remain
//! finite without weakening ordinary evaluation or encoding semantic identity
//! in display strings.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::declared_type::{
    DeclaredGenericArg, DeclaredType, IndexTypeRef, StructTypeRef,
};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::types::{IndexDef, IndexKind};
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use miette::NamedSource;

/// Concrete fixed-axis schema retained by the generic model interface.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelIndexSchema {
    identity: IndexTypeRef,
    kind: ModelIndexKind,
}

impl ModelIndexSchema {
    /// Owner-qualified or structural index identity.
    #[must_use]
    pub const fn identity(&self) -> &IndexTypeRef {
        &self.identity
    }

    /// Closed concrete axis representation.
    #[must_use]
    pub const fn kind(&self) -> &ModelIndexKind {
        &self.kind
    }
}

/// Concrete index families supported by recursive Graphcal values.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelIndexKind {
    /// Declared label axis.
    Named {
        /// Labels in declaration order.
        variants: Vec<IndexVariantName>,
    },
    /// Fixed physical-coordinate axis.
    Coordinate {
        /// Coordinates in SI representation.
        coordinates_si: Vec<f64>,
        /// Shared physical dimension.
        dimension: graphcal_compiler::dimension::Dimension,
        /// Optional source display-unit label.
        display_label: Option<String>,
        /// Display-unit scale to SI.
        display_scale: f64,
    },
    /// Structural `Fin(N)` axis.
    Finite {
        /// Exact positive cardinality.
        cardinality: usize,
    },
}

/// Stable semantic identity of one concrete algebraic type instantiation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelTypeId {
    identity: StructTypeRef,
    generic_args: Vec<DeclaredGenericArg>,
}

impl ModelTypeId {
    const fn new(identity: StructTypeRef, generic_args: Vec<DeclaredGenericArg>) -> Self {
        Self {
            identity,
            generic_args,
        }
    }

    /// Canonical owner-qualified nominal identity.
    #[must_use]
    pub const fn identity(&self) -> &StructTypeRef {
        &self.identity
    }

    /// Concrete sorted generic arguments.
    #[must_use]
    pub fn generic_args(&self) -> &[DeclaredGenericArg] {
        &self.generic_args
    }
}

/// One algebraic constructor field whose value may refer to another arena type.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelFieldSchema {
    name: FieldName,
    value: ModelValueSchema,
}

impl ModelFieldSchema {
    /// Source field name.
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    /// Concrete field schema, possibly an algebraic arena reference.
    #[must_use]
    pub const fn value(&self) -> &ModelValueSchema {
        &self.value
    }
}

/// One constructor in a concrete algebraic definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConstructorSchema {
    name: ConstructorName,
    fields: Vec<ModelFieldSchema>,
}

impl ModelConstructorSchema {
    /// Source constructor name.
    #[must_use]
    pub const fn name(&self) -> &ConstructorName {
        &self.name
    }

    /// Constructor fields in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[ModelFieldSchema] {
        &self.fields
    }
}

/// Canonical boundary unit attached to a quantity schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUnitSchema {
    label: String,
    scale_to_si: f64,
}

impl ModelUnitSchema {
    /// Canonical source-like unit label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Multiplicative scale from this unit to SI.
    #[must_use]
    pub const fn scale_to_si(&self) -> f64 {
        self.scale_to_si
    }
}

/// One real or complex physical quantity family.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuantitySchema {
    dimension: graphcal_compiler::dimension::Dimension,
    canonical_unit: Option<ModelUnitSchema>,
}

impl ModelQuantitySchema {
    /// Concrete physical dimension.
    #[must_use]
    pub const fn dimension(&self) -> &graphcal_compiler::dimension::Dimension {
        &self.dimension
    }

    /// Canonical SI boundary unit for dimensioned quantities.
    #[must_use]
    pub const fn canonical_unit(&self) -> Option<&ModelUnitSchema> {
        self.canonical_unit.as_ref()
    }
}

/// Transport-independent schema node for one concrete Graphcal value.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelValueSchema {
    /// Real physical quantity.
    Quantity(ModelQuantitySchema),
    /// Complex physical quantity.
    Complex(ModelQuantitySchema),
    /// Boolean value.
    Bool,
    /// Signed integer value.
    Int,
    /// Physical instant in one declared time scale.
    Datetime(TimeScale),
    /// One key from a fixed index.
    Key(ModelIndexSchema),
    /// Typed reference to an algebraic definition in [`ModelSchemaGraph`].
    Algebraic(ModelTypeId),
    /// Fixed-axis map layer.
    Indexed {
        /// Element schema after selecting one key on this axis.
        element: Box<Self>,
        /// Concrete outer axis.
        axis: ModelIndexSchema,
    },
}

/// One immutable definition stored in a [`ModelSchemaGraph`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModelAlgebraicTypeSchema {
    id: ModelTypeId,
    constructors: Vec<ModelConstructorSchema>,
}

impl ModelAlgebraicTypeSchema {
    /// Concrete nominal type identity represented by this definition.
    #[must_use]
    pub const fn id(&self) -> &ModelTypeId {
        &self.id
    }

    /// Constructors in source order.
    #[must_use]
    pub fn constructors(&self) -> &[ModelConstructorSchema] {
        &self.constructors
    }
}

/// Immutable arena of concrete algebraic model definitions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelSchemaGraph {
    definitions: Vec<ModelAlgebraicTypeSchema>,
    definition_indices: HashMap<ModelTypeId, usize>,
}

impl ModelSchemaGraph {
    /// Look up a definition by its full semantic type identity.
    #[must_use]
    pub fn definition(&self, id: &ModelTypeId) -> Option<&ModelAlgebraicTypeSchema> {
        self.definition_indices
            .get(id)
            .and_then(|index| self.definitions.get(*index))
    }

    /// Every retained definition in deterministic discovery order.
    #[must_use]
    pub fn definitions(&self) -> &[ModelAlgebraicTypeSchema] {
        &self.definitions
    }

    /// Whether resolving definitions reachable from `schema` encounters a
    /// direct or mutual algebraic cycle.
    #[must_use]
    pub fn contains_recursive_type(&self, schema: &ModelValueSchema) -> bool {
        let mut roots = Vec::new();
        collect_schema_type_refs(schema, &mut roots);
        roots
            .iter()
            .any(|root| self.reachable_type_graph_has_cycle(root))
    }

    fn insert(&mut self, definition: ModelAlgebraicTypeSchema) {
        if self.definition_indices.contains_key(&definition.id) {
            return;
        }
        let index = self.definitions.len();
        self.definition_indices.insert(definition.id.clone(), index);
        self.definitions.push(definition);
    }

    fn reachable_type_graph_has_cycle(&self, root: &ModelTypeId) -> bool {
        enum Frame<'a> {
            Enter(&'a ModelTypeId),
            Exit(&'a ModelTypeId),
        }

        let mut active = HashSet::new();
        let mut complete = HashSet::new();
        let mut stack = vec![Frame::Enter(root)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Enter(id) => {
                    if active.contains(id) {
                        return true;
                    }
                    if complete.contains(id) {
                        continue;
                    }
                    active.insert(id);
                    stack.push(Frame::Exit(id));
                    if let Some(definition) = self.definition(id) {
                        let mut referenced = Vec::new();
                        definition
                            .constructors
                            .iter()
                            .flat_map(|constructor| &constructor.fields)
                            .for_each(|field| {
                                collect_schema_type_refs(&field.value, &mut referenced);
                            });
                        stack.extend(referenced.into_iter().rev().map(Frame::Enter));
                    }
                }
                Frame::Exit(id) => {
                    active.remove(id);
                    complete.insert(id);
                }
            }
        }
        false
    }
}

fn collect_schema_type_refs<'schema>(
    schema: &'schema ModelValueSchema,
    output: &mut Vec<&'schema ModelTypeId>,
) {
    match schema {
        ModelValueSchema::Algebraic(id) => output.push(id),
        ModelValueSchema::Indexed { element, .. } => collect_schema_type_refs(element, output),
        ModelValueSchema::Quantity(_)
        | ModelValueSchema::Complex(_)
        | ModelValueSchema::Bool
        | ModelValueSchema::Int
        | ModelValueSchema::Datetime(_)
        | ModelValueSchema::Key(_) => {}
    }
}

pub(super) struct ModelSchemaGraphBuilder<'a> {
    tir: &'a graphcal_compiler::tir::typed::TIR,
    source: &'a NamedSource<Arc<String>>,
    graph: ModelSchemaGraph,
    building: HashSet<ModelTypeId>,
}

impl<'a> ModelSchemaGraphBuilder<'a> {
    pub(super) fn new(
        tir: &'a graphcal_compiler::tir::typed::TIR,
        source: &'a NamedSource<Arc<String>>,
    ) -> Self {
        Self {
            tir,
            source,
            graph: ModelSchemaGraph::default(),
            building: HashSet::new(),
        }
    }

    pub(super) fn finish(self) -> ModelSchemaGraph {
        self.graph
    }

    pub(super) fn value_schema(
        &mut self,
        declared_type: &DeclaredType,
    ) -> Result<ModelValueSchema, GraphcalError> {
        match declared_type {
            DeclaredType::Quantity(dimension) => Ok(ModelValueSchema::Quantity(
                model_quantity_schema(dimension, self.tir),
            )),
            DeclaredType::Complex(dimension) => Ok(ModelValueSchema::Complex(
                model_quantity_schema(dimension, self.tir),
            )),
            DeclaredType::Bool => Ok(ModelValueSchema::Bool),
            DeclaredType::Int => Ok(ModelValueSchema::Int),
            DeclaredType::Datetime(scale) => Ok(ModelValueSchema::Datetime(*scale)),
            DeclaredType::Key(index) => {
                model_index_schema(index, self.tir, self.source).map(ModelValueSchema::Key)
            }
            DeclaredType::Indexed { element, index } => Ok(ModelValueSchema::Indexed {
                element: Box::new(self.value_schema(element)?),
                axis: model_index_schema(index, self.tir, self.source)?,
            }),
            DeclaredType::Struct(identity, generic_args) => {
                let id = ModelTypeId::new(identity.clone(), generic_args.clone());
                self.ensure_algebraic_definition(&id)?;
                Ok(ModelValueSchema::Algebraic(id))
            }
            DeclaredType::IndexArg(index) => Err(GraphcalError::internal_error(
                format!(
                    "unresolved index argument `{index}` cannot be a concrete model value port"
                ),
                self.source,
                DiagnosticAnchor::WholeFile,
            )),
        }
    }

    #[expect(
        clippy::needless_collect,
        reason = "materializing TIR-backed field specs releases immutable borrows before recursively mutating the schema arena"
    )]
    fn ensure_algebraic_definition(&mut self, id: &ModelTypeId) -> Result<(), GraphcalError> {
        if self.graph.definition(id).is_some() || !self.building.insert(id.clone()) {
            return Ok(());
        }

        let result = (|| {
            let model_type = graphcal_compiler::tir::dim_check::ConcreteModelType::try_new(
                self.tir,
                &id.identity,
                &id.generic_args,
                self.source,
            )
            .map_err(|error| error.into_graphcal_error(self.source))?;
            let constructor_specs = model_type
                .constructors(self.source)?
                .into_iter()
                .map(|constructor| {
                    (
                        constructor.name().clone(),
                        constructor
                            .fields()
                            .iter()
                            .map(|field| (field.name().clone(), field.declared_type().clone()))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();

            constructor_specs
                .into_iter()
                .map(|(name, fields)| {
                    let fields = fields
                        .into_iter()
                        .map(|(name, declared_type)| {
                            Ok(ModelFieldSchema {
                                name,
                                value: self.value_schema(&declared_type)?,
                            })
                        })
                        .collect::<Result<Vec<_>, GraphcalError>>()?;
                    Ok(ModelConstructorSchema { name, fields })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()
        })();
        self.building.remove(id);
        let constructors = result?;
        self.graph.insert(ModelAlgebraicTypeSchema {
            id: id.clone(),
            constructors,
        });
        Ok(())
    }
}

fn model_quantity_schema(
    dimension: &graphcal_compiler::dimension::Dimension,
    tir: &graphcal_compiler::tir::typed::TIR,
) -> ModelQuantitySchema {
    let canonical_unit = (!dimension.is_dimensionless())
        .then(|| {
            crate::eval::types::default_unit_label(
                dimension,
                tir.registry().dimensions.base_dim_symbols(),
            )
        })
        .flatten()
        .map(|label| ModelUnitSchema {
            label,
            scale_to_si: 1.0,
        });
    ModelQuantitySchema {
        dimension: dimension.clone(),
        canonical_unit,
    }
}

fn model_index_schema(
    index: &IndexTypeRef,
    tir: &graphcal_compiler::tir::typed::TIR,
    source: &NamedSource<Arc<String>>,
) -> Result<ModelIndexSchema, GraphcalError> {
    if let Some(finite) = index.finite_index() {
        return Ok(ModelIndexSchema {
            identity: index.clone(),
            kind: ModelIndexKind::Finite {
                cardinality: finite.cardinality().get(),
            },
        });
    }
    let definition = index_def_for_ref(index, tir).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("concrete model index definition is unavailable for `{index}`"),
            source,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let kind = match &definition.kind {
        IndexKind::Named { variants } => ModelIndexKind::Named {
            variants: variants.clone(),
        },
        IndexKind::Coordinate(data) => ModelIndexKind::Coordinate {
            coordinates_si: (0..data.cardinality())
                .map(|position| data.coordinate_value(position))
                .collect(),
            dimension: data.dimension.clone(),
            display_label: data.display_label.clone(),
            display_scale: data.display_scale,
        },
        IndexKind::Finite { cardinality } => ModelIndexKind::Finite {
            cardinality: cardinality.get(),
        },
        IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. } => {
            return Err(GraphcalError::internal_error(
                format!("required model index `{index}` was not concretely bound"),
                source,
                DiagnosticAnchor::WholeFile,
            ));
        }
    };
    Ok(ModelIndexSchema {
        identity: index.clone(),
        kind,
    })
}

pub(super) fn index_def_for_ref<'tir>(
    index: &IndexTypeRef,
    tir: &'tir graphcal_compiler::tir::typed::TIR,
) -> Option<&'tir IndexDef> {
    index.finite_index().map_or_else(
        || tir.declared_index_def(index.declared_resolved()?),
        |finite| tir.registry().indexes.get_finite_index(finite),
    )
}
