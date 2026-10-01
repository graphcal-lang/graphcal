//! A checked TIR sealed together with every fact its execution needs.
//!
//! [`EvaluatedTir::evaluate`] owns a [`CheckedTir`] together with the
//! [`ConstPool`] evaluated from it, and [`EvaluatedTir::seal`] is the only
//! construction of a [`CheckedProgram`]: it adds the facts derived from those
//! constants and resolves every imported constant once. Execution then
//! selects a DAG and its facts from one value, so they can never come from
//! different checks.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::ir::imported_binding::{ImportedBinding, ImportedValueKind};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::checked_dag::CheckedDag;
use graphcal_compiler::tir::typed::dag_position::DagPosition;
use graphcal_compiler::tir::typed::model::StructFieldConstraintKey;

use crate::runtime_value::RuntimeValue;

use crate::constant_pools::{
    ConstPool, ConstPoolBuildError, ConstStep, ConstantReference, RuntimeValueMap,
};
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::runtime_presentation::{EvaluatedRuntimeValue, PendingPresentedMap};

/// Resolved domain constraints of one DAG's declarations.
pub type DomainConstraints = HashMap<ResolvedDeclName, ResolvedDomainConstraint>;

/// Resolved domain constraints of struct fields, per concrete application.
pub type StructFieldConstraints = HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>;

/// Why a checked TIR could not be sealed with its execution facts.
#[derive(Debug, Error)]
pub enum SealError {
    #[error("the constant pool does not cover DAG `{0}` of the checked TIR")]
    UncoveredDag(DagId),
    #[error("DAG `{0}` has incomplete execution checks")]
    MissingDomainConstraints(DagId),
    #[error("imported declaration `{0}` has no containing body")]
    MissingDeclaration(ResolvedDeclName),
    #[error("imported declaration `{target}` is not a checked {kind:?} value")]
    WrongImportedKind {
        target: ResolvedDeclName,
        kind: ImportedValueKind,
    },
    #[error("imported constant `{0}` has no checked value in its defining body's pool")]
    MissingConstant(ResolvedDeclName),
}

/// One imported constant of a DAG, resolved once when its program is sealed.
#[derive(Debug, Clone)]
pub struct ImportedConstant {
    name: ScopedName,
    declared_type: CheckedType,
    value: ConstantReference,
}

impl ImportedConstant {
    /// The importing DAG's lexical name for the constant.
    #[must_use]
    pub const fn name(&self) -> &ScopedName {
        &self.name
    }

    /// The constant's checked declared type.
    #[must_use]
    pub const fn declared_type(&self) -> &CheckedType {
        &self.declared_type
    }

    /// The constant in its defining DAG's pool.
    #[must_use]
    pub const fn value(&self) -> &ConstantReference {
        &self.value
    }
}

/// Execution facts of one DAG besides its constants.
#[derive(Debug)]
struct DagExecutionFacts {
    source: SourceId,
    /// Compile-time selections only; dynamic display requests have no invocation state.
    const_presentations: PendingPresentedMap,
    domain_constraints: Arc<DomainConstraints>,
    imported_constants: Vec<ImportedConstant>,
}

/// Execution facts of every DAG of a sealed program. A checked module
/// publishes them to the programs of its importers.
#[derive(Debug, Clone, Default)]
pub struct ExecutionFacts {
    consts: ConstPool,
    by_dag: Arc<HashMap<DagId, Arc<DagExecutionFacts>>>,
    struct_field_constraints: Arc<StructFieldConstraints>,
}

impl ExecutionFacts {
    /// The diagnostic source of one DAG.
    #[must_use]
    pub fn source(&self, dag_id: &DagId) -> Option<SourceId> {
        self.by_dag.get(dag_id).map(|facts| facts.source)
    }

    /// The compile-time presented value of every constant with a
    /// presentation.
    pub fn const_presentations(
        &self,
    ) -> impl Iterator<Item = (&ResolvedDeclName, &EvaluatedRuntimeValue)> {
        self.by_dag
            .values()
            .flat_map(|facts| facts.const_presentations.iter())
    }

    /// Resolved struct-field constraints of every concrete application.
    #[must_use]
    pub fn struct_field_constraints(&self) -> &StructFieldConstraints {
        &self.struct_field_constraints
    }
}

/// Facts one check derived from the constants of the DAGs it scheduled.
pub struct ScheduledChecks {
    /// The checked file's source, the diagnostic source of every scheduled DAG.
    pub source: SourceId,
    /// Compile-time presentations of the evaluated constants.
    pub const_presentations: PendingPresentedMap,
    /// Domain constraints of every scheduled DAG.
    pub domain_constraints: HashMap<DagId, DomainConstraints>,
    /// Struct-field constraints resolved by this check.
    pub struct_field_constraints: StructFieldConstraints,
}

/// A checked TIR sealed with its execution facts.
pub struct CheckedProgram {
    tir: CheckedTir,
    facts: ExecutionFacts,
    /// The constants and facts of each DAG of the TIR, by registry position.
    by_position: Vec<(Arc<RuntimeValueMap>, Arc<DagExecutionFacts>)>,
}

impl std::fmt::Debug for CheckedProgram {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckedProgram")
            .field("root", self.tir.root_dag_id())
            .field("dags", &self.tir.dag_registry().len())
            .finish_non_exhaustive()
    }
}

/// One DAG of a sealed program together with its execution facts.
#[derive(Debug, Clone, Copy)]
pub struct SealedDag<'a> {
    dag: &'a CheckedDag,
    const_values: &'a Arc<RuntimeValueMap>,
    facts: &'a DagExecutionFacts,
}

impl<'a> SealedDag<'a> {
    #[must_use]
    pub const fn dag(self) -> &'a CheckedDag {
        self.dag
    }

    /// The source the DAG's diagnostics point into.
    #[must_use]
    pub const fn source(self) -> SourceId {
        self.facts.source
    }

    /// The DAG's evaluated constants.
    #[must_use]
    pub const fn const_values(self) -> &'a Arc<RuntimeValueMap> {
        self.const_values
    }

    /// Resolved domain constraints of the DAG's declarations.
    #[must_use]
    pub const fn domain_constraints(self) -> &'a Arc<DomainConstraints> {
        &self.facts.domain_constraints
    }

    /// The DAG's imported constants.
    #[must_use]
    pub fn imported_constants(self) -> &'a [ImportedConstant] {
        &self.facts.imported_constants
    }
}

/// A checked TIR together with the constant pool evaluated from it and the
/// facts of the checked modules it includes: the only input a
/// [`CheckedProgram`] is sealed from.
pub struct EvaluatedTir {
    tir: CheckedTir,
    consts: ConstPool,
    inherited: ExecutionFacts,
}

impl EvaluatedTir {
    /// Evaluate the constants `tir` schedules with `evaluate`, as
    /// [`ConstPool::build`] does; the DAGs of checked modules share their
    /// pools from `inherited`.
    ///
    /// # Errors
    ///
    /// Returns the first evaluation error, or a structural mismatch between
    /// `tir` and `inherited`.
    pub fn evaluate<E>(
        tir: CheckedTir,
        inherited: &ExecutionFacts,
        evaluate: impl FnMut(ConstStep<'_>) -> Result<RuntimeValue, E>,
    ) -> Result<Self, ConstPoolBuildError<E>> {
        let consts = ConstPool::build(&tir, &inherited.consts, evaluate)?;
        Ok(Self {
            tir,
            consts,
            inherited: inherited.clone(),
        })
    }

    /// The checked TIR.
    #[must_use]
    pub const fn tir(&self) -> &CheckedTir {
        &self.tir
    }

    /// The evaluated constants of every DAG of the TIR.
    #[must_use]
    pub const fn consts(&self) -> &ConstPool {
        &self.consts
    }

    /// Facts of the checked modules this TIR includes.
    #[must_use]
    pub const fn inherited(&self) -> &ExecutionFacts {
        &self.inherited
    }

    /// Seal the TIR with its constants and `checks`, the facts derived from
    /// the constants of the DAGs it scheduled. Inherited DAGs keep the facts
    /// their module published; every imported constant is resolved here, once.
    ///
    /// # Errors
    ///
    /// Returns [`SealError`] when `checks` does not cover a scheduled DAG, or
    /// an imported value binding does not resolve.
    pub fn seal(self, checks: ScheduledChecks) -> Result<CheckedProgram, SealError> {
        let Self {
            tir,
            consts,
            inherited,
        } = self;
        let ScheduledChecks {
            source,
            const_presentations,
            mut domain_constraints,
            struct_field_constraints,
        } = checks;
        let mut by_dag = HashMap::new();
        let mut by_position = Vec::with_capacity(tir.dag_registry().len());
        for (_, dag) in tir.dag_registry().positioned() {
            let dag_id = dag.dag_id();
            let const_values = consts
                .for_dag(dag_id)
                .ok_or_else(|| SealError::UncoveredDag(dag_id.clone()))?;
            let facts = if let Some(facts) = inherited.by_dag.get(dag_id) {
                Arc::clone(facts)
            } else {
                Arc::new(DagExecutionFacts {
                    source,
                    const_presentations: const_values
                        .keys()
                        .filter_map(|key| {
                            const_presentations
                                .get(key)
                                .map(|evidence| (key.clone(), evidence.clone()))
                        })
                        .collect(),
                    domain_constraints: Arc::new(
                        domain_constraints
                            .remove(dag_id)
                            .ok_or_else(|| SealError::MissingDomainConstraints(dag_id.clone()))?,
                    ),
                    imported_constants: resolve_imported_constants(&tir, &consts, dag)?,
                })
            };
            by_position.push((Arc::clone(const_values), Arc::clone(&facts)));
            by_dag.insert(dag_id.clone(), facts);
        }
        let mut all_field_constraints = inherited.struct_field_constraints.as_ref().clone();
        all_field_constraints.extend(struct_field_constraints);
        Ok(CheckedProgram {
            tir,
            facts: ExecutionFacts {
                consts,
                by_dag: Arc::new(by_dag),
                struct_field_constraints: Arc::new(all_field_constraints),
            },
            by_position,
        })
    }
}

impl CheckedProgram {
    /// The checked TIR.
    #[must_use]
    pub const fn tir(&self) -> &CheckedTir {
        &self.tir
    }

    /// Execution facts of every DAG.
    #[must_use]
    pub const fn facts(&self) -> &ExecutionFacts {
        &self.facts
    }

    /// Select one DAG with its execution facts.
    #[must_use]
    pub fn dag(&self, dag_id: &DagId) -> Option<SealedDag<'_>> {
        self.tir
            .dag_registry()
            .get_positioned(dag_id)
            .map(|(position, dag)| self.sealed(position, dag))
    }

    /// Every DAG of the program with its execution facts, in registry
    /// position order.
    pub fn positioned(&self) -> impl Iterator<Item = (DagPosition, SealedDag<'_>)> {
        self.tir
            .dag_registry()
            .positioned()
            .map(|(position, dag)| (position, self.sealed(position, dag)))
    }

    /// The DAG at `position` of this program's registry, with the facts
    /// sealing recorded for that position.
    fn sealed<'a>(&'a self, position: DagPosition, dag: &'a CheckedDag) -> SealedDag<'a> {
        let (const_values, facts) = &self.by_position[position.index()];
        SealedDag {
            dag,
            const_values,
            facts,
        }
    }

    /// Split a checked module's program for publication: its TIR's bodies
    /// for importers' TIRs, and its facts for importers' programs.
    #[must_use]
    pub fn into_parts(self) -> (CheckedTir, ExecutionFacts) {
        (self.tir, self.facts)
    }
}

/// Resolve the imported constants of one DAG in `consts`.
fn resolve_imported_constants(
    tir: &CheckedTir,
    consts: &ConstPool,
    dag: &CheckedDag,
) -> Result<Vec<ImportedConstant>, SealError> {
    dag.imported_bindings()
        .iter()
        .filter_map(|(name, binding)| {
            resolve_imported_constant(tir, consts, name, binding).transpose()
        })
        .collect()
}

/// Resolve one imported value binding: its constant in `consts`, or `None`
/// for a runtime import of a runtime declaration.
pub fn resolve_imported_constant(
    tir: &CheckedTir,
    consts: &ConstPool,
    name: &ScopedName,
    binding: &ImportedBinding,
) -> Result<Option<ImportedConstant>, SealError> {
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::ImportedSourceResolution);
    let target = binding.target();
    let defining = tir
        .dag_containing_declaration(target)
        .ok_or_else(|| SealError::MissingDeclaration(target.clone()))?;
    match (binding.kind(), defining.is_constant(target)) {
        (ImportedValueKind::Runtime, false) => Ok(None),
        (ImportedValueKind::Constant, true) => consts
            .reference(target)
            .map(|value| {
                Some(ImportedConstant {
                    name: name.clone(),
                    declared_type: binding.declared_type().clone(),
                    value,
                })
            })
            .ok_or_else(|| SealError::MissingConstant(target.clone())),
        (kind, _) => Err(SealError::WrongImportedKind {
            target: target.clone(),
            kind,
        }),
    }
}
