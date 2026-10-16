//! Projection of a checked project (and optionally its evaluation) into
//! [`ViewModel`]s: the root module, and each include instance materialized
//! in the root's evaluation.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use graphcal_compiler::dag_call_read::DagCallRead;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::desugar::desugared_ast::Declaration;
use graphcal_compiler::ir::instance::{
    ExposedValueBody, HirInstanceRecord, InstanceProjection, ProjectionExposure,
};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::index_def::{FiniteIndex, IndexDef};
use graphcal_compiler::source_registry::ForeignSourceId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::text_position::Utf16LineIndex;
use graphcal_compiler::tir::typed::declaration_view::{
    DeclarationKind, DeclarationView, ValueDeclaration,
};
use graphcal_compiler::tir::typed::{
    CheckedDag, CheckedDeclType, CheckedTir, ResolvedIndex, ResolvedValueType,
};
use graphcal_eval::eval::public_projection::coordinate_entry_display_names;
use graphcal_eval::eval::{AssertResult, EvalResult, OutputUnavailable, Value};
use graphcal_eval::presentation_evidence::PresentationPathPart;
use thiserror::Error;

use crate::graph_ir::{GraphClusterKind, GraphIr, GraphProjectionError, project_tir};
use crate::loader::{LoadedFile, LoadedProject};
use crate::project_compiler::CheckedProject;

use super::cells::definition_cells;
use super::model::{
    ViewCall, ViewCell, ViewElement, ViewIndex, ViewIndexEntry, ViewInstance, ViewModel,
    ViewModule, ViewNode, ViewNodeKind, ViewOrigin, ViewSource, ViewSubject, ViewValue,
};
use super::target::{ViewModels, ViewTarget};
use super::value::{common_unit, view_value};
use super::wire_key::{ViewIndexId, ViewInstanceId, ViewNodeId};

/// Why a checked project could not be projected into a view model.
#[derive(Debug, Error)]
pub enum ViewProjectionError {
    /// The dependency graph could not be projected.
    #[error(transparent)]
    Graph(#[from] GraphProjectionError),
    /// A loaded file's source id does not resolve in the project's registry.
    #[error(transparent)]
    ForeignSource(#[from] ForeignSourceId),
    /// A declaration selected for the view is neither a value nor an assertion.
    #[error("`{declaration}` is not a value or assertion declaration")]
    NotPresentable { declaration: ResolvedDeclName },
    /// Two nodes of the model would share one id.
    #[error("two view nodes share the id `{id}`")]
    DuplicateNodeId { id: ViewNodeId },
    /// A node reads a node the model does not contain.
    #[error("view node `{node}` reads `{dependency}`, which the view does not contain")]
    UnknownDependency {
        node: ViewNodeId,
        dependency: ViewNodeId,
    },
    /// An include instance's DAG, or its template's file, is not part of the
    /// checked project.
    #[error("the include instance `{instance}` is not part of the checked project")]
    MissingInstance { instance: DagId },
}

/// Project the structure of the root file of `checked`, evaluated alone.
///
/// Every node comes without its value: attach an evaluation with
/// [`ViewProjection::attach_evaluation`], or take the structure alone for a
/// library file or an unevaluated project. Source paths are relative to the
/// project root the loader read the files under.
///
/// # Errors
///
/// Returns [`ViewProjectionError`] when the checked program violates a
/// projection invariant.
pub fn project_view(
    project: &LoadedProject,
    checked: &CheckedProject,
) -> Result<ViewProjection, ViewProjectionError> {
    let ir = project_tir(checked.tir())?;
    let scope = Scope::new(project, checked, &ir)?;
    project_subject(&scope, Subject::Root)
}

/// Project every target of the root file of `checked`.
///
/// The targets are the root module, and each include instance materialized
/// in its evaluation (nested instances included), each with the values the
/// root's evaluation gives it.
///
/// # Errors
///
/// Returns [`ViewProjectionError`] when the checked program violates a
/// projection invariant.
pub fn project_views(
    project: &LoadedProject,
    checked: &CheckedProject,
) -> Result<ViewProjections, ViewProjectionError> {
    let ir = project_tir(checked.tir())?;
    let scope = Scope::new(project, checked, &ir)?;
    let module = project_subject(&scope, Subject::Root)?;
    let instances = scope
        .instances()
        .into_iter()
        .map(|record| {
            Ok((
                ViewInstanceId::of(record.instance.id().owner()),
                project_subject(&scope, Subject::Instance(record))?,
            ))
        })
        .collect::<Result<_, ViewProjectionError>>()?;
    Ok(ViewProjections { module, instances })
}

/// The projections of every target of one root, as [`project_views`] makes
/// them.
#[derive(Debug, Clone)]
pub struct ViewProjections {
    module: ViewProjection,
    instances: BTreeMap<ViewInstanceId, ViewProjection>,
}

impl ViewProjections {
    /// The projection of `target`; `None` for an instance the root's
    /// evaluation does not materialize.
    #[must_use]
    pub fn get(&self, target: &ViewTarget) -> Option<&ViewProjection> {
        match target {
            ViewTarget::Module => Some(&self.module),
            ViewTarget::Instance { instance } => self.instances.get(instance),
        }
    }

    /// Attach the root's evaluation to every target: an instance's nodes
    /// take the values the root's evaluation gives that instance.
    pub fn attach_evaluation(&mut self, evaluation: &EvalResult) {
        let values = EvaluatedValues::new(evaluation);
        self.module.attach(&values);
        for projection in self.instances.values_mut() {
            projection.attach(&values);
        }
    }

    /// The models as projected, with the values attached so far.
    #[must_use]
    pub fn into_models(self) -> ViewModels {
        ViewModels::new(
            self.module.into_model(),
            self.instances
                .into_iter()
                .map(|(id, projection)| (id, projection.into_model()))
                .collect(),
        )
    }
}

/// A projected structure whose nodes can still take their evaluated values.
#[derive(Debug, Clone)]
pub struct ViewProjection {
    model: ViewModel,
    /// Where each node's value comes from, parallel to `model.nodes`.
    bindings: Vec<ValueBinding>,
}

impl ViewProjection {
    /// The model as projected, with the values attached so far.
    #[must_use]
    pub fn into_model(self) -> ViewModel {
        self.model
    }

    /// The model as projected, with the values attached so far.
    #[must_use]
    pub const fn model(&self) -> &ViewModel {
        &self.model
    }

    /// Attach the values and assertion outcomes of `evaluation` to the
    /// nodes. A node whose declaration the evaluation does not report, or
    /// reports as failed or incomplete, keeps no value.
    ///
    /// A value that cannot be shown in its display unit is shown in SI, as
    /// `graphcal eval` shows it, and its node lists why in `presentation`,
    /// together with the presentation diagnostics the evaluation reported
    /// for its declaration.
    pub fn attach_evaluation(&mut self, evaluation: &EvalResult) {
        self.attach(&EvaluatedValues::new(evaluation));
    }

    fn attach(&mut self, values: &EvaluatedValues<'_>) {
        for (node, binding) in self.model.nodes.iter_mut().zip(&self.bindings) {
            let mut problems = match binding.kind {
                ValueBindingKind::CallOutput => Vec::new(),
                ValueBindingKind::Value | ValueBindingKind::Assertion => {
                    values.presentation(&binding.declaration)
                }
            };
            let value = match binding.kind {
                ValueBindingKind::CallOutput => None,
                ValueBindingKind::Assertion => values
                    .assertion(&binding.declaration)
                    .and_then(assertion_value),
                ValueBindingKind::Value => values
                    .value(&binding.declaration)
                    .map(|value| view_value(value, &values.result.render, &mut problems)),
            };
            node.unit = value.as_ref().and_then(common_unit);
            node.value = value;
            node.presentation = problems;
        }
    }
}

/// Where a node's value comes from in an evaluation.
#[derive(Debug, Clone)]
struct ValueBinding {
    declaration: ResolvedDeclName,
    kind: ValueBindingKind,
}

#[derive(Debug, Clone, Copy)]
enum ValueBindingKind {
    /// A value entry, looked up by identity.
    Value,
    /// An assertion outcome, looked up by identity.
    Assertion,
    /// None: an inline call's output is evaluated in place, as part of its
    /// caller, so the evaluation reports no value of its own.
    CallOutput,
}

/// What one projection presents.
#[derive(Clone, Copy)]
enum Subject<'p> {
    /// The root DAG, evaluated alone.
    Root,
    /// An include instance materialized in the root's evaluation, by the
    /// include edge it was materialized from.
    Instance(&'p HirInstanceRecord),
}

/// Project `subject` of `scope`'s root.
fn project_subject(
    scope: &Scope<'_>,
    subject: Subject<'_>,
) -> Result<ViewProjection, ViewProjectionError> {
    let tir = scope.tir;
    let (dag, presented) = match subject {
        Subject::Root => (tir.root(), scope.root().clone()),
        Subject::Instance(record) => {
            let id = record.instance.id();
            let dag = tir.dag_registry().get(id.owner()).ok_or_else(|| {
                ViewProjectionError::MissingInstance {
                    instance: id.owner().clone(),
                }
            })?;
            (dag, id.template().clone())
        }
    };
    let locals = local_declarations(dag);
    let aliases = scope.include_aliases(dag);
    let spellings = scope.external_spellings(dag, subject);
    let mut deps = dependencies(tir, scope.ir, &locals);
    // An alias reads its instance's output; that edge is the alias itself.
    for alias in aliases.keys() {
        deps.remove(alias);
    }
    let externals: BTreeSet<&ResolvedDeclName> = locals
        .iter()
        .flat_map(|id| deps.get(id).into_iter().flatten().copied())
        .filter(|dep| dep.owner() != dag.dag_id())
        .collect();
    let calls = call_outputs(tir, &locals);

    let module_name = scope.module_name(&presented);
    let cells_from = scope
        .project
        .files()
        .get(&presented.file_root())
        .map(|file| file.ast().declarations.as_slice());
    let mut builder = NodeBuilder {
        shared_definitions: shared_definitions(tir, &locals),
        scope,
        spellings: &spellings,
        presented: &presented,
        module_name: &module_name,
        cells_from,
        instance: match subject {
            Subject::Root => None,
            Subject::Instance(record) => Some(record),
        },
        axes: BTreeMap::new(),
    };
    let mut nodes = Vec::new();
    let mut bindings = Vec::new();
    for id in &locals {
        let deps = deps
            .get(id)
            .into_iter()
            .flatten()
            .map(|dep| ViewNodeId::of(dep))
            .chain(
                calls
                    .iter()
                    .filter(|call| call.caller == id)
                    .map(CallOutput::id),
            )
            .collect();
        let built = match aliases.get(id) {
            Some(alias) => builder.include_alias(id, alias)?,
            None => builder.local(id, deps)?,
        };
        if let Some((node, binding)) = built {
            nodes.push(node);
            bindings.push(binding);
        }
    }
    for id in externals {
        if let Some((node, binding)) = builder.external(id)? {
            nodes.push(node);
            bindings.push(binding);
        }
    }
    for call in &calls {
        if let Some((node, binding)) = builder.call_output(call)? {
            nodes.push(node);
            bindings.push(binding);
        }
    }
    let indexes = builder.indexes();
    check_node_references(&nodes)?;

    let (module, target) = presented_as(scope, subject, module_name)?;
    Ok(ViewProjection {
        model: ViewModel {
            version: 1,
            module,
            target,
            indexes,
            nodes,
        },
        bindings,
    })
}

/// The module a view of `subject` presents, named `module_name`, and what it
/// presents it as.
fn presented_as(
    scope: &Scope<'_>,
    subject: Subject<'_>,
    module_name: String,
) -> Result<(ViewModule, ViewSubject), ViewProjectionError> {
    match subject {
        Subject::Root => Ok((scope.root_module(), ViewSubject::Module)),
        Subject::Instance(record) => {
            let template = record.instance.id().template();
            let file = scope
                .sources
                .name_of(&template.file_root())
                .ok_or_else(|| ViewProjectionError::MissingInstance {
                    instance: record.instance.id().owner().clone(),
                })?;
            Ok((
                ViewModule {
                    id: module_name,
                    file: file.to_string(),
                    package: template.package().to_string(),
                },
                ViewSubject::Instance {
                    root: scope.root_module(),
                    instance: view_instance(record),
                },
            ))
        }
    }
}

/// The checked project and the lookups every projection shares.
struct Scope<'p> {
    project: &'p LoadedProject,
    checked: &'p CheckedProject,
    tir: &'p CheckedTir,
    ir: &'p GraphIr,
    /// Semantic provenance of each projected DAG.
    clusters: HashMap<&'p DagId, &'p GraphClusterKind>,
    sources: SourceFiles<'p>,
}

impl<'p> Scope<'p> {
    fn new(
        project: &'p LoadedProject,
        checked: &'p CheckedProject,
        ir: &'p GraphIr,
    ) -> Result<Self, ViewProjectionError> {
        Ok(Self {
            project,
            checked,
            tir: checked.tir(),
            ir,
            clusters: ir
                .clusters()
                .iter()
                .filter_map(|cluster| Some((cluster.id().dag()?, cluster.kind())))
                .collect(),
            sources: SourceFiles::new(project, checked)?,
        })
    }

    fn root(&self) -> &'p DagId {
        self.tir.root_dag_id()
    }

    /// The root module, as a view names it.
    fn root_module(&self) -> ViewModule {
        let root = self.root();
        ViewModule {
            id: self.module_name(root),
            file: self.sources.root_name.clone(),
            package: root.package().to_string(),
        }
    }

    /// Every include instance the root's evaluation materializes: the
    /// root's includes, then (depth first) the includes of each instance.
    /// An inline `dag` template is not evaluated, so its includes are not.
    fn instances(&self) -> Vec<&'p HirInstanceRecord> {
        let mut instances = Vec::new();
        let mut pending = vec![self.tir.root()];
        while let Some(dag) = pending.pop() {
            for record in dag.semantic_instances() {
                instances.push(record);
                if let Some(instance) = self.tir.dag_registry().get(record.instance.id().owner()) {
                    pending.push(instance);
                }
            }
        }
        instances
    }

    /// The module a declaration is presented under: the template of an
    /// include instance, otherwise its owner.
    fn module_of(&self, owner: &DagId) -> DagId {
        match self.clusters.get(owner) {
            Some(GraphClusterKind::Instance { template }) => template.clone(),
            _ => owner.clone(),
        }
    }

    /// The module path of `module` (`power_project.eps`), as the loader
    /// recorded it for its file; a module whose file path is no module path
    /// keeps its identity.
    fn module_name(&self, module: &DagId) -> String {
        self.checked
            .module_paths()
            .module(module)
            .map_or_else(|| module.to_string(), |path| path.to_string())
    }

    /// A declaration of another module as no binding of the subject spells
    /// it: its name under its module's path (`pipeline.lib::x`).
    fn qualified_name(&self, declaration: &ResolvedDeclName) -> String {
        self.checked
            .module_paths()
            .name(declaration.owner(), declaration.leaf())
            .map_or_else(|| declaration.to_string(), |name| name.to_string())
    }

    /// Whether `module` declares `name` as an explicit `pub` output.
    fn is_public(&self, module: &DagId, name: &DeclName) -> bool {
        self.project.module(module).is_some_and(|module| {
            module
                .interface()
                .value_outputs()
                .iter()
                .any(|output| output.name() == name)
        })
    }

    /// Selective include items `dag` declares as local aliases
    /// (`include lib()::{ x }`): the alias stands for the instance's output,
    /// so the view presents it as that module's declaration.
    fn include_aliases(&self, dag: &'p CheckedDag) -> HashMap<ResolvedDeclName, IncludeAlias> {
        let mut aliases = HashMap::new();
        for record in dag.semantic_instances() {
            let Some(instance) = self.tir.dag_registry().semantic_instance(record) else {
                continue;
            };
            let template = record.instance.id().template();
            for projection in instance.output_projections() {
                let ProjectionExposure::Selected(name) = projection.projection.exposure() else {
                    continue;
                };
                if projection.projection.body() != ExposedValueBody::LocalAlias {
                    continue;
                }
                let Some(alias) = dag
                    .declarations()
                    .find(|declaration| declaration.name() == name)
                else {
                    continue;
                };
                aliases.insert(
                    alias.identity().clone(),
                    IncludeAlias {
                        target: projection.target,
                        template: template.clone(),
                        instance: view_instance(record),
                    },
                );
            }
        }
        aliases
    }

    /// How `subject` spells each declaration of another module it reads: an
    /// imported value by its import binding (`num`), an include member by
    /// its instance scope (`e::p_budget`). An instance also reads, through
    /// the include's arguments, what the module including it reads: that
    /// module's own declarations, and the values it spells.
    fn external_spellings(
        &self,
        dag: &'p CheckedDag,
        subject: Subject<'p>,
    ) -> HashMap<ResolvedDeclName, ExternalName> {
        let mut spellings = HashMap::new();
        if let Subject::Instance(record) = subject
            && let Some(includer) = self.tir.dag_registry().get(record.instance.id().parent())
        {
            for declaration in includer.declarations() {
                spellings.insert(
                    declaration.identity().clone(),
                    ExternalName {
                        label: declaration.name().to_string(),
                        origin: ViewOrigin::Includer,
                    },
                );
            }
            self.spellings_of(includer, &mut spellings);
        }
        self.spellings_of(dag, &mut spellings);
        spellings
    }

    /// The values `dag` reads through its imports and includes, as it
    /// spells them.
    fn spellings_of(
        &self,
        dag: &'p CheckedDag,
        spellings: &mut HashMap<ResolvedDeclName, ExternalName>,
    ) {
        for (name, binding) in dag.imported_bindings() {
            spellings.insert(
                binding.target().clone(),
                ExternalName {
                    label: name.to_string(),
                    origin: ViewOrigin::Imported,
                },
            );
        }
        for record in dag.semantic_instances() {
            let Some(instance) = self.tir.dag_registry().semantic_instance(record) else {
                continue;
            };
            for projection in instance.output_projections() {
                spellings.insert(
                    projection.target,
                    ExternalName {
                        label: record
                            .instance
                            .exposed_name(projection.projection)
                            .to_string(),
                        origin: ViewOrigin::Included {
                            instance: view_instance(record),
                        },
                    },
                );
            }
        }
    }
}

/// `dag`'s value and assertion declarations in source order. (An include
/// alias is declared where the `include` is written, not where the DAG lists
/// it.)
fn local_declarations(dag: &CheckedDag) -> Vec<ResolvedDeclName> {
    let mut declarations: Vec<(usize, ResolvedDeclName)> = dag
        .declarations()
        .filter_map(|declaration| {
            let span = match declaration.kind() {
                DeclarationKind::Value(value) => value.span,
                DeclarationKind::Assert { span, .. } => span,
                DeclarationKind::Plot { .. }
                | DeclarationKind::Figure { .. }
                | DeclarationKind::Layer { .. } => return None,
            };
            Some((span.offset(), declaration.identity().clone()))
        })
        .collect();
    declarations.sort_by_key(|(offset, _)| *offset);
    declarations
        .into_iter()
        .map(|(_, declaration)| declaration)
        .collect()
}

/// The definition ranges more than one of `locals` is defined by: the
/// columns of a multi-declaration table share their table's.
fn shared_definitions(tir: &CheckedTir, locals: &[ResolvedDeclName]) -> HashSet<Span> {
    let mut seen = HashSet::new();
    let mut shared = HashSet::new();
    for id in locals {
        let Some((_, declaration)) = declaration_of(tir, id) else {
            continue;
        };
        if let DeclarationKind::Value(ValueDeclaration {
            definition: Some(definition),
            ..
        }) = declaration.kind()
            && !seen.insert(definition)
        {
            shared.insert(definition);
        }
    }
    shared
}

/// An include instance as the view shows it: its key, its name (the alias of
/// `include lib() as l`, or the template's file name when the include is
/// anonymous), and whether the include binds anything.
fn view_instance(record: &HirInstanceRecord) -> ViewInstance {
    let id = record.instance.id();
    let substitution = record.instance.substitution();
    ViewInstance {
        id: ViewInstanceId::of(id.owner()),
        name: id
            .scope()
            .alias()
            .map_or_else(|| id.template().leaf().to_string(), ToString::to_string),
        bound: !record.value_bindings.is_empty()
            || !substitution.indexes.is_empty()
            || !substitution.types.is_empty()
            || !substitution.dimensions.is_empty(),
    }
}

/// A declaration standing for an include instance's output.
struct IncludeAlias {
    /// The instance declaration the alias reads.
    target: ResolvedDeclName,
    /// The template the instance was materialized from.
    template: DagId,
    /// The instance as the view shows it.
    instance: ViewInstance,
}

/// How the subject spells a declaration of another module, and through what.
struct ExternalName {
    label: String,
    origin: ViewOrigin,
}

/// The output one inline DAG call of a subject's declaration reads: the
/// `ordinal`-th call of `caller`, in pre-order.
struct CallOutput<'a> {
    caller: &'a ResolvedDeclName,
    ordinal: usize,
    read: &'a DagCallRead,
}

impl CallOutput<'_> {
    fn id(&self) -> ViewNodeId {
        ViewNodeId::of_call_output(self.caller, self.ordinal, &self.read.output)
    }
}

/// The outputs the inline DAG calls of `locals` read, in declaration and
/// then call order.
fn call_outputs<'a>(tir: &'a CheckedTir, locals: &'a [ResolvedDeclName]) -> Vec<CallOutput<'a>> {
    locals
        .iter()
        .filter_map(|caller| {
            let dag = tir.dag_registry().get(caller.owner())?;
            Some((caller, dag.dependencies().call_reads.get(caller)?))
        })
        .flat_map(|(caller, reads)| {
            reads
                .iter()
                .enumerate()
                .map(move |(ordinal, read)| CallOutput {
                    caller,
                    ordinal,
                    read,
                })
        })
        .collect()
}

/// What each local declaration reads: dataflow edges for values, the
/// assertion dependency map for assertions.
fn dependencies<'a>(
    tir: &'a CheckedTir,
    ir: &'a GraphIr,
    locals: &'a [ResolvedDeclName],
) -> BTreeMap<&'a ResolvedDeclName, Vec<&'a ResolvedDeclName>> {
    let mut deps = BTreeMap::<&ResolvedDeclName, Vec<&ResolvedDeclName>>::new();
    for edge in ir.edges() {
        if let (Some(to), Some(from)) = (edge.to().declaration(), edge.from().declaration()) {
            deps.entry(to).or_default().push(from);
        }
    }
    for id in locals {
        if let Some(dag) = tir.dag_registry().get(id.owner())
            && let Some(assert_deps) = dag.dependencies().assert_deps.get(id)
        {
            deps.insert(id, assert_deps.iter().collect());
        }
    }
    deps
}

/// Builds the nodes of one subject, collecting the index axes they use.
struct NodeBuilder<'a> {
    scope: &'a Scope<'a>,
    /// Definition ranges written once for several of the subject's
    /// declarations (the columns of a multi-declaration table): none of them
    /// owns the text.
    shared_definitions: HashSet<Span>,
    spellings: &'a HashMap<ResolvedDeclName, ExternalName>,
    /// The module the subject's own declarations are presented under: the
    /// root, or an instance's template.
    presented: &'a DagId,
    module_name: &'a str,
    /// The declarations of the presented module's file, whose table cells
    /// the subject's own nodes edit.
    cells_from: Option<&'a [Declaration]>,
    /// The include edge of an instance subject, whose arguments define its
    /// bound params.
    instance: Option<&'a HirInstanceRecord>,
    axes: BTreeMap<ViewIndexId, ResolvedIndex>,
}

/// A node and where its value comes from.
type BuiltNode = (ViewNode, ValueBinding);

impl<'a> NodeBuilder<'a> {
    /// A declaration of the subject itself.
    fn local(
        &mut self,
        id: &ResolvedDeclName,
        deps: Vec<ViewNodeId>,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        let Some((dag, declaration)) = declaration_of(self.scope.tir, id) else {
            return Ok(None);
        };
        let site = self
            .argument_of(id, &declaration)
            .unwrap_or(Definition::Declared);
        self.node(NodeInput {
            dag,
            declaration,
            label: id.as_str().to_string(),
            module: self.module_name.to_string(),
            origin: ViewOrigin::Local,
            is_public: self.scope.is_public(self.presented, id.leaf()),
            deps,
            file: self.scope.sources.file_of(&self.presented.file_root()),
            definition: site,
            value_of: id,
        })
        .map(Some)
    }

    /// Where an instance's param is defined when the include binds it: the
    /// argument, in the file of the module that includes the instance.
    fn argument_of(
        &self,
        id: &ResolvedDeclName,
        declaration: &DeclarationView<'_>,
    ) -> Option<Definition<'a>> {
        let record = self.instance?;
        let DeclarationKind::Value(ValueDeclaration {
            category: ValueDeclCategory::Param,
            ..
        }) = declaration.kind()
        else {
            return None;
        };
        let (_, argument) = record
            .value_bindings
            .iter()
            .find(|(port, _)| port.leaf() == id.leaf())?;
        let includer = self.scope.module_of(record.instance.id().parent());
        let file = self.scope.sources.file_of(&includer.file_root())?;
        Some(Definition::Argument {
            file,
            span: argument.span,
        })
    }

    /// An include alias, presented as the template's declaration but valued
    /// by the alias itself (its entry carries the exposed value).
    fn include_alias(
        &mut self,
        id: &ResolvedDeclName,
        alias: &IncludeAlias,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        let Some(template) = self.scope.tir.dag_registry().get(&alias.template) else {
            return Ok(None);
        };
        let Some(origin) = template
            .declarations()
            .find(|declaration| declaration.name() == alias.target.leaf())
        else {
            return Ok(None);
        };
        self.node(NodeInput {
            dag: template,
            declaration: origin,
            label: id.as_str().to_string(),
            module: self.scope.module_name(&alias.template),
            origin: ViewOrigin::Included {
                instance: alias.instance.clone(),
            },
            is_public: self.scope.is_public(&alias.template, alias.target.leaf()),
            deps: Vec::new(),
            file: self.scope.sources.file_of(&alias.template.file_root()),
            definition: Definition::Declared,
            value_of: id,
        })
        .map(Some)
    }

    /// A declaration of another module the subject reads.
    fn external(
        &mut self,
        id: &ResolvedDeclName,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        // An include instance's declarations keep the template's spans, so
        // the instance's own declaration locates the source in the template.
        let Some((dag, declaration)) = declaration_of(self.scope.tir, id) else {
            return Ok(None);
        };
        let module = self.scope.module_of(id.owner());
        let (label, origin) = self.spellings.get(id).map_or_else(
            || (self.scope.qualified_name(id), ViewOrigin::Imported),
            |name| (name.label.clone(), name.origin.clone()),
        );
        self.node(NodeInput {
            dag,
            declaration,
            label,
            module: self.scope.module_name(&module),
            origin,
            is_public: self.scope.is_public(&module, id.leaf()),
            deps: Vec::new(),
            file: self.scope.sources.file_of(&module.file_root()),
            definition: Definition::Declared,
            value_of: id,
        })
        .map(Some)
    }

    /// The output an inline DAG call reads, presented as the called DAG's
    /// declaration read through the call, under a key of its own.
    fn call_output(
        &mut self,
        call: &CallOutput<'_>,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        let Some((dag, declaration)) = declaration_of(self.scope.tir, &call.read.output) else {
            return Ok(None);
        };
        let target = &call.read.target;
        let (node, binding) = self.node(NodeInput {
            dag,
            declaration,
            label: format!("{}(…)::{}", target.leaf(), call.read.output.as_str()),
            module: self.scope.module_name(target),
            origin: ViewOrigin::Called {
                instance: ViewCall {
                    name: format!("{}(…)", target.leaf()),
                    bound: call.read.binds,
                },
            },
            is_public: self.scope.is_public(target, call.read.output.leaf()),
            deps: Vec::new(),
            file: self.scope.sources.file_of(&target.file_root()),
            definition: Definition::Declared,
            value_of: &call.read.output,
        })?;
        let node = ViewNode {
            id: call.id(),
            ..node
        };
        let binding = ValueBinding {
            kind: ValueBindingKind::CallOutput,
            ..binding
        };
        Ok(Some((node, binding)))
    }

    fn node(&mut self, input: NodeInput<'_>) -> Result<BuiltNode, ViewProjectionError> {
        let identity = input.declaration.identity();
        let (kind, span, definition, doc, annotation) = match input.declaration.kind() {
            DeclarationKind::Value(ValueDeclaration {
                category,
                span,
                definition,
                doc,
                annotation,
                ..
            }) => (kind_of(category), span, definition, doc, Some(annotation)),
            DeclarationKind::Assert { span, body, doc } => {
                (ViewNodeKind::Assert, span, Some(body), doc, None)
            }
            DeclarationKind::Plot { .. }
            | DeclarationKind::Figure { .. }
            | DeclarationKind::Layer { .. } => {
                return Err(ViewProjectionError::NotPresentable {
                    declaration: identity.clone(),
                });
            }
        };

        let decl_type = input
            .dag
            .value_decl_type(identity)
            .map(CheckedDeclType::resolved);
        let axes = decl_type.map_or_else(Vec::new, |ty| {
            ty.indexes()
                .iter()
                .map(|axis| {
                    let id = ViewIndexId::of(axis);
                    self.axes.entry(id.clone()).or_insert_with(|| axis.clone());
                    id
                })
                .collect()
        });
        // An assertion is a `Bool` check; it declares no type of its own. A
        // value's element type is shown as its declaration spells it.
        let dimension = match kind {
            ViewNodeKind::Assert => Some("Bool".to_string()),
            _ => annotation
                .zip(input.file)
                .and_then(|(annotation, file)| slice(file.text, annotation.element_span()))
                .or_else(|| decl_type.map(|ty| ty.element().format(self.scope.tir.registry()))),
        };

        let (expr, source) = self.definition_of(&input, span, definition);
        let element = match kind {
            ViewNodeKind::Assert => ViewElement::Bool,
            _ => decl_type.map_or(ViewElement::Other, |ty| element_of(ty.element())),
        };

        let binding = ValueBinding {
            declaration: input.value_of.clone(),
            kind: match kind {
                ViewNodeKind::Assert => ValueBindingKind::Assertion,
                _ => ValueBindingKind::Value,
            },
        };
        let node = ViewNode {
            id: ViewNodeId::of(input.value_of),
            label: input.label,
            name: input.declaration.name().as_str().to_string(),
            module: input.module,
            kind,
            origin: input.origin,
            element,
            axes,
            dimension,
            unit: None,
            // A parameter is an input even when its module exposes it.
            is_public: input.is_public && kind != ViewNodeKind::Param,
            deps: input.deps,
            expr,
            source,
            doc: doc.map(str::to_string),
            value: None,
            presentation: Vec::new(),
        };
        Ok((node, binding))
    }

    /// The text that defines a node's value and where it is: the source
    /// range after `=` (parentheses included) of the declaration spanning
    /// `span` and defined at `definition`, or the include's argument for a
    /// bound param of an instance.
    fn definition_of(
        &self,
        input: &NodeInput<'_>,
        span: Span,
        definition: Option<Span>,
    ) -> (Option<String>, Option<ViewSource>) {
        let (file, line_at, definition, owns_definition, cells) = match input.definition {
            Definition::Declared => {
                // Only the subject's own declarations are edited from its sheet.
                let cells = match (input.origin == ViewOrigin::Local, self.cells_from) {
                    (true, Some(declarations)) => {
                        definition_cells(declarations, input.declaration.name())
                    }
                    _ => Vec::new(),
                };
                let owned = definition.filter(|span| !self.shared_definitions.contains(span));
                (input.file, span, definition, owned, cells)
            }
            Definition::Argument { file, span } => {
                (Some(file), span, Some(span), Some(span), Vec::new())
            }
        };
        let expr = file.and_then(|file| definition.and_then(|span| slice(file.text, span)));
        let source = file.map(|file| ViewSource {
            file: Some(file.name.to_string()),
            line: file.lines.position(line_at.offset()).line.saturating_add(1),
            expr_range: owns_definition
                .map(|span| file.lines.range(span.offset(), span.len()).into()),
            cells: cells
                .into_iter()
                .map(|cell| ViewCell {
                    keys: cell.keys.iter().map(ToString::to_string).collect(),
                    range: file.lines.range(cell.span.offset(), cell.span.len()).into(),
                })
                .collect(),
        });
        (expr, source)
    }

    /// Every index used as an axis, with its entries.
    fn indexes(&self) -> Vec<ViewIndex> {
        self.axes
            .iter()
            .map(|(id, axis)| ViewIndex {
                id: id.clone(),
                name: axis.to_string(),
                module: match axis {
                    ResolvedIndex::Concrete(name, _) => {
                        Some(self.scope.module_name(&self.scope.module_of(name.owner())))
                    }
                    ResolvedIndex::Finite(..) | ResolvedIndex::GenericParam(..) => None,
                },
                entries: match axis {
                    ResolvedIndex::Concrete(name, _) => self
                        .scope
                        .tir
                        .declared_index_def(name)
                        .map_or_else(Vec::new, axis_entries),
                    ResolvedIndex::Finite(form, _) => form
                        .constant_value()
                        .and_then(|size| FiniteIndex::try_from_u64(size).ok())
                        .map_or_else(Vec::new, |finite| axis_entries(&IndexDef::finite(finite))),
                    ResolvedIndex::GenericParam(..) => Vec::new(),
                },
            })
            .collect()
    }
}

/// The entries of an index, each under the key its values are keyed by and
/// shown as `graphcal eval` shows it.
fn axis_entries(def: &IndexDef) -> Vec<ViewIndexEntry> {
    let keys = def.entry_keys();
    let coordinates = def.coordinate_data().and_then(|data| {
        NonEmpty::try_from_vec(keys.clone())
            .ok()
            .map(|keys| coordinate_entry_display_names(data, &keys))
    });
    keys.iter()
        .map(|key: &IndexEntryKey| ViewIndexEntry {
            key: key.to_string(),
            display: coordinates
                .as_ref()
                .and_then(|names| names.get(key).cloned())
                .unwrap_or_else(|| key.to_string()),
        })
        .collect()
}

/// What one node is built from.
struct NodeInput<'a> {
    /// The DAG whose declaration is presented.
    dag: &'a CheckedDag,
    declaration: DeclarationView<'a>,
    label: String,
    module: String,
    origin: ViewOrigin,
    is_public: bool,
    deps: Vec<ViewNodeId>,
    /// The declaration's file.
    file: Option<SourceFile<'a>>,
    /// Where the text defining the value is.
    definition: Definition<'a>,
    /// The identity whose evaluated value the node shows; it is the node's
    /// key too.
    value_of: &'a ResolvedDeclName,
}

/// Where the text defining a node's value is.
#[derive(Clone, Copy)]
enum Definition<'a> {
    /// In the declaration, after `=`.
    Declared,
    /// In the include that binds an instance's param: its argument.
    Argument { file: SourceFile<'a>, span: Span },
}

/// Every node id is unique, and every dependency names a node of the model:
/// the view joins nodes by id, so a clash or a dangling id would wire the
/// wrong declarations together.
pub(super) fn check_node_references(nodes: &[ViewNode]) -> Result<(), ViewProjectionError> {
    let mut ids = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if !ids.insert(&node.id) {
            return Err(ViewProjectionError::DuplicateNodeId {
                id: node.id.clone(),
            });
        }
    }
    for node in nodes {
        if let Some(dependency) = node.deps.iter().find(|dep| !ids.contains(dep)) {
            return Err(ViewProjectionError::UnknownDependency {
                node: node.id.clone(),
                dependency: dependency.clone(),
            });
        }
    }
    Ok(())
}

fn declaration_of<'t>(
    tir: &'t CheckedTir,
    id: &ResolvedDeclName,
) -> Option<(&'t CheckedDag, DeclarationView<'t>)> {
    let dag = tir.dag_registry().get(id.owner())?;
    let declaration = dag.declaration(id)?;
    Some((dag, declaration))
}

/// The element type family of a declared value type.
const fn element_of(element: &ResolvedValueType) -> ViewElement {
    match element {
        ResolvedValueType::Bool => ViewElement::Bool,
        ResolvedValueType::Int => ViewElement::Int,
        ResolvedValueType::Quantity(_) => ViewElement::Quantity,
        ResolvedValueType::Complex { .. } => ViewElement::Complex,
        ResolvedValueType::Datetime(_) => ViewElement::Datetime,
        ResolvedValueType::Key { .. } => ViewElement::Key,
        ResolvedValueType::Struct { .. } => ViewElement::Struct,
        ResolvedValueType::GenericTypeParam(..) => ViewElement::Other,
    }
}

const fn kind_of(category: ValueDeclCategory) -> ViewNodeKind {
    match category {
        ValueDeclCategory::Const => ViewNodeKind::Const,
        ValueDeclCategory::Param => ViewNodeKind::Param,
        ValueDeclCategory::Node => ViewNodeKind::Node,
    }
}

/// The evaluation's values and assertion outcomes, keyed by identity.
struct EvaluatedValues<'e> {
    result: &'e EvalResult,
    by_identity: HashMap<&'e ResolvedDeclName, &'e Result<Value, OutputUnavailable>>,
    assertions: HashMap<&'e ResolvedDeclName, &'e AssertResult>,
}

impl<'e> EvaluatedValues<'e> {
    fn new(result: &'e EvalResult) -> Self {
        Self {
            result,
            by_identity: result
                .entries
                .iter()
                .chain(&result.nested_instance_entries)
                .map(|entry| (&entry.identity, &entry.outcome))
                .collect(),
            assertions: result
                .assertions
                .iter()
                .map(|assertion| (&assertion.identity, &assertion.outcome))
                .collect(),
        }
    }

    fn value(&self, id: &ResolvedDeclName) -> Option<&'e Value> {
        self.by_identity.get(id)?.as_ref().ok()
    }

    /// The display failures the evaluation reported for the value of `id`
    /// (plot channels aside), each with the path of the leaf it concerns.
    fn presentation(&self, id: &ResolvedDeclName) -> Vec<String> {
        self.result
            .presentation_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.declaration == *id && diagnostic.channel.is_none())
            .map(|diagnostic| {
                let path: String = diagnostic
                    .detail
                    .path
                    .iter()
                    .map(|part| match part {
                        PresentationPathPart::Field(field) => format!(".{field}"),
                        PresentationPathPart::Index(index) => format!("[{index}]"),
                    })
                    .collect();
                let failure = &diagnostic.detail.failure;
                if path.is_empty() {
                    format!("{failure} (SI value retained)")
                } else {
                    format!("{path}: {failure} (SI value retained)")
                }
            })
            .collect()
    }

    /// The outcome of an assertion the root reports: its own, or one an
    /// include exposes.
    fn assertion(&self, id: &ResolvedDeclName) -> Option<&'e AssertResult> {
        self.assertions.get(id).copied()
    }
}

/// The loaded files, by the module they define, with their view names.
struct SourceFiles<'p> {
    project: &'p LoadedProject,
    names: HashMap<&'p DagId, String>,
    /// The root file's view name.
    root_name: String,
    /// Each file's line index: one scan per file, not one per declaration.
    lines: HashMap<&'p DagId, Utf16LineIndex<'p>>,
}

impl<'p> SourceFiles<'p> {
    fn new(project: &'p LoadedProject, checked: &CheckedProject) -> Result<Self, ForeignSourceId> {
        let name_of = |file: &LoadedFile| -> Result<String, ForeignSourceId> {
            // The name is a project-relative path with `/` on every
            // platform: it is the wire format the view and the editors key
            // their files by. A file outside the project root (a dependency)
            // keeps the loader's diagnostic name.
            let relative = project
                .project_root()
                .and_then(|root| file.path().strip_prefix(root).ok())
                .map(|path| {
                    path.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                });
            match relative {
                Some(relative) => Ok(relative),
                None => Ok(checked
                    .sources()
                    .named_source(file.source_id())?
                    .name()
                    .to_string()),
            }
        };
        let mut names = HashMap::new();
        let mut lines = HashMap::new();
        for file in project.files().iter() {
            names.insert(file.dag_id(), name_of(file)?);
            lines.insert(file.dag_id(), Utf16LineIndex::new(file.source().as_str()));
        }
        Ok(Self {
            project,
            names,
            root_name: name_of(project.root_file())?,
            lines,
        })
    }

    /// The view name of the file defining `module`.
    fn name_of(&self, module: &DagId) -> Option<&str> {
        self.names.get(module).map(String::as_str)
    }

    fn file_of(&self, module: &DagId) -> Option<SourceFile<'_>> {
        let file = self.project.files().get(module)?;
        Some(SourceFile {
            name: self.names.get(file.dag_id())?.as_str(),
            text: file.source().as_str(),
            lines: self.lines.get(file.dag_id())?,
        })
    }
}

/// One source file: its view name, text, and line index.
#[derive(Clone, Copy)]
struct SourceFile<'p> {
    name: &'p str,
    text: &'p str,
    lines: &'p Utf16LineIndex<'p>,
}

fn assertion_value(outcome: &AssertResult) -> Option<ViewValue> {
    match outcome {
        AssertResult::Pass => Some(ViewValue::Bool {
            display: "PASS".to_string(),
            value: true,
        }),
        AssertResult::Fail { .. } => Some(ViewValue::Bool {
            display: "FAIL".to_string(),
            value: false,
        }),
        AssertResult::Blocked { .. } | AssertResult::Error { .. } => None,
    }
}

fn slice(text: &str, span: Span) -> Option<String> {
    text.get(span.offset()..span.offset().checked_add(span.len())?)
        .map(str::to_string)
}
