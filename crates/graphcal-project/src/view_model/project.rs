//! Projection of a checked project (and optionally its evaluation) into a
//! [`ViewModel`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::ir::instance::{ExposedValueBody, InstanceProjection, ProjectionExposure};
use graphcal_compiler::ir::prelude_definitions::PreludeDefinitionError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::index_def::{FiniteIndex, IndexDef};
use graphcal_compiler::source_registry::ForeignSourceId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::text_position::Utf16LineIndex;
use graphcal_compiler::tir::typed::declaration_view::{
    DeclarationKind, DeclarationView, ValueDeclaration,
};
use graphcal_compiler::tir::typed::{
    CheckedDag, CheckedDeclType, CheckedTir, ResolvedDeclType, ResolvedDim, ResolvedIndex,
    ResolvedValueType,
};
use graphcal_eval::eval::public_projection::coordinate_entry_display_names;
use graphcal_eval::eval::{
    AssertResult, DisplayProjectionError, EvalResult, OutputUnavailable, Value,
};
use thiserror::Error;

use crate::graph_ir::{GraphClusterKind, GraphIr, GraphProjectionError, project_tir};
use crate::loader::{LoadedFile, LoadedProject};
use crate::project_compiler::CheckedProject;

use super::cells::definition_cells;
use super::model::{
    ViewCell, ViewElement, ViewIndex, ViewIndexEntry, ViewInstance, ViewModel, ViewModule,
    ViewNode, ViewNodeKind, ViewOrigin, ViewSource, ViewValue,
};
use super::value::{CoherentUnits, common_unit, view_value};
use super::wire_key::{ViewIndexId, ViewNodeId};

/// Why a checked project could not be projected into a view model.
#[derive(Debug, Error)]
pub enum ViewProjectionError {
    /// The dependency graph could not be projected.
    #[error(transparent)]
    Graph(#[from] GraphProjectionError),
    /// The built-in prelude failed to construct (a compiler bug).
    #[error(transparent)]
    Prelude(#[from] PreludeDefinitionError),
    /// A loaded file's source id does not resolve in the project's registry.
    #[error(transparent)]
    ForeignSource(#[from] ForeignSourceId),
    /// A declaration selected for the view is neither a value nor an assertion.
    #[error("`{declaration}` is not a value or assertion declaration")]
    NotPresentable { declaration: ResolvedDeclName },
    /// A value could not be shown in its display unit.
    #[error("value of `{declaration}` cannot be displayed: {error}")]
    Display {
        declaration: ResolvedDeclName,
        #[source]
        error: DisplayProjectionError,
    },
    /// Two nodes of the model would share one id.
    #[error("two view nodes share the id `{id}`")]
    DuplicateNodeId { id: ViewNodeId },
    /// A node reads a node the model does not contain.
    #[error("view node `{node}` reads `{dependency}`, which the view does not contain")]
    UnknownDependency {
        node: ViewNodeId,
        dependency: ViewNodeId,
    },
}

/// Project the structure of the root file of `checked`.
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
    let tir = checked.tir();
    let ir = project_tir(tir)?;
    let scope = Scope::new(project, checked, &ir)?;
    let locals = scope.local_declarations();
    let aliases = scope.include_aliases();
    let spellings = scope.external_spellings();
    let mut deps = dependencies(tir, &ir, &locals);
    // An alias reads its instance's output; that edge is the alias itself.
    for alias in aliases.keys() {
        deps.remove(alias);
    }
    let externals: BTreeSet<&ResolvedDeclName> = locals
        .iter()
        .flat_map(|id| deps.get(id).into_iter().flatten().copied())
        .filter(|dep| !scope.is_local(dep.owner()))
        .collect();

    let mut builder = NodeBuilder {
        shared_definitions: scope.shared_definitions(&locals),
        scope: &scope,
        spellings: &spellings,
        axes: BTreeMap::new(),
    };
    let mut nodes = Vec::new();
    let mut bindings = Vec::new();
    for id in &locals {
        let deps = deps.get(id).map_or_else(Vec::new, |deps| {
            deps.iter().map(|dep| ViewNodeId::of(dep)).collect()
        });
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
    let indexes = builder.indexes();
    check_node_references(&nodes)?;

    let root = tir.root_dag_id();
    Ok(ViewProjection {
        model: ViewModel {
            version: 0,
            module: ViewModule {
                id: scope.module_name(root),
                file: scope.sources.root_name.clone(),
                package: root.package().to_string(),
            },
            indexes,
            nodes,
        },
        bindings,
        units: scope.units,
    })
}

/// A projected structure whose nodes can still take their evaluated values.
#[derive(Debug, Clone)]
pub struct ViewProjection {
    model: ViewModel,
    /// Where each node's value comes from, parallel to `model.nodes`.
    bindings: Vec<ValueBinding>,
    units: CoherentUnits,
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
    /// # Errors
    ///
    /// Returns [`ViewProjectionError::Display`] when a value cannot be shown
    /// in its display unit; the nodes before it keep their values.
    pub fn attach_evaluation(
        &mut self,
        evaluation: &EvalResult,
    ) -> Result<(), ViewProjectionError> {
        let values = EvaluatedValues::new(evaluation);
        for (node, binding) in self.model.nodes.iter_mut().zip(&self.bindings) {
            let value = match binding.kind {
                ValueBindingKind::Assertion => values
                    .assertion(&binding.declaration)
                    .and_then(assertion_value),
                ValueBindingKind::Value => values
                    .value(&binding.declaration)
                    .map(|value| view_value(value, &evaluation.render, &self.units))
                    .transpose()
                    .map_err(|error| ViewProjectionError::Display {
                        declaration: binding.declaration.clone(),
                        error,
                    })?,
            };
            if let Some(value) = value {
                node.unit = common_unit(&value);
                node.value = Some(value);
            }
        }
        Ok(())
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
    /// A root-owned assertion outcome, looked up by name.
    Assertion,
}

/// The checked project and the lookups every node shares.
struct Scope<'p> {
    project: &'p LoadedProject,
    checked: &'p CheckedProject,
    tir: &'p CheckedTir,
    /// Semantic provenance of each projected DAG.
    clusters: HashMap<&'p DagId, &'p GraphClusterKind>,
    sources: SourceFiles<'p>,
    units: CoherentUnits,
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
            clusters: ir
                .clusters()
                .iter()
                .map(|cluster| (cluster.dag_id(), cluster.kind()))
                .collect(),
            sources: SourceFiles::new(project, checked)?,
            units: CoherentUnits::from_prelude()?,
        })
    }

    fn root(&self) -> &'p DagId {
        self.tir.root_dag_id()
    }

    /// Whether `owner` is the root DAG. An inline `dag` declared in the
    /// root file is a template: its declarations hold no values of their
    /// own, and the root reads them only through an instance.
    fn is_local(&self, owner: &DagId) -> bool {
        owner == self.root()
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

    /// A declaration of another module as no binding of the root spells it:
    /// its name under its module's path (`pipeline.lib::x`).
    fn qualified_name(&self, declaration: &ResolvedDeclName) -> String {
        self.checked
            .module_paths()
            .name(declaration.owner(), declaration.leaf())
            .map_or_else(|| declaration.to_string(), |name| name.to_string())
    }

    /// The definition ranges more than one of `locals` is defined by: the
    /// columns of a multi-declaration table share their table's.
    fn shared_definitions(&self, locals: &[ResolvedDeclName]) -> HashSet<Span> {
        let mut seen = HashSet::new();
        let mut shared = HashSet::new();
        for id in locals {
            let Some((_, declaration)) = declaration_of(self.tir, id) else {
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

    /// The root's value and assertion declarations in source order. (An
    /// include alias is declared where the `include` is written, not where
    /// the DAG lists it.)
    fn local_declarations(&self) -> Vec<ResolvedDeclName> {
        let mut declarations: Vec<(usize, ResolvedDeclName)> = self
            .tir
            .dag_registry()
            .get(self.root())
            .into_iter()
            .flat_map(|dag| {
                dag.declarations().filter_map(|declaration| {
                    let span = match declaration.kind() {
                        DeclarationKind::Value(value) => value.span,
                        DeclarationKind::Assert { span, .. } => span,
                        DeclarationKind::Plot { .. }
                        | DeclarationKind::Figure { .. }
                        | DeclarationKind::Layer { .. } => return None,
                    };
                    Some((span.offset(), declaration.identity().clone()))
                })
            })
            .collect();
        declarations.sort_by_key(|(offset, _)| *offset);
        declarations
            .into_iter()
            .map(|(_, declaration)| declaration)
            .collect()
    }

    /// Selective include items the root declares as local aliases
    /// (`include lib()::{ x }`): the alias stands for the instance's output,
    /// so the view presents it as that module's declaration.
    fn include_aliases(&self) -> HashMap<ResolvedDeclName, IncludeAlias> {
        let mut aliases = HashMap::new();
        for record in self.tir.root().semantic_instances() {
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
                let Some(alias) = self
                    .tir
                    .root()
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
                        instance: view_instance(record, template),
                    },
                );
            }
        }
        aliases
    }

    /// How the root spells each declaration of another module it reads:
    /// an imported value by its import binding (`num`), an include member
    /// by its instance scope (`e::p_budget`).
    fn external_spellings(&self) -> HashMap<ResolvedDeclName, ExternalName> {
        let mut spellings = HashMap::new();
        for (name, binding) in self.tir.root().imported_bindings() {
            spellings.insert(
                binding.target().clone(),
                ExternalName {
                    label: name.to_string(),
                    origin: ViewOrigin::Imported,
                },
            );
        }
        for record in self.tir.root().semantic_instances() {
            let Some(instance) = self.tir.dag_registry().semantic_instance(record) else {
                continue;
            };
            let template = record.instance.id().template();
            for projection in instance.output_projections() {
                spellings.insert(
                    projection.target,
                    ExternalName {
                        label: record
                            .instance
                            .exposed_name(projection.projection)
                            .to_string(),
                        origin: ViewOrigin::Included {
                            instance: view_instance(record, template),
                        },
                    },
                );
            }
        }
        spellings
    }
}

/// An include instance as the view shows it: named by its alias
/// (`include lib() as l`), or the template's file name when the include is
/// anonymous, and whether the include binds anything.
fn view_instance(
    record: &graphcal_compiler::ir::instance::HirInstanceRecord,
    template: &DagId,
) -> ViewInstance {
    let substitution = record.instance.substitution();
    ViewInstance {
        name: record
            .instance
            .id()
            .scope()
            .alias()
            .map_or_else(|| template.leaf().to_string(), ToString::to_string),
        bound: !record.value_bindings.is_empty()
            || !substitution.indexes.is_empty()
            || !substitution.types.is_empty()
            || !substitution.dimensions.is_empty(),
    }
}

/// A root declaration standing for an include instance's output.
struct IncludeAlias {
    /// The instance declaration the alias reads.
    target: ResolvedDeclName,
    /// The template the instance was materialized from.
    template: DagId,
    /// The instance as the view shows it.
    instance: ViewInstance,
}

/// How the root spells a declaration of another module, and through what.
struct ExternalName {
    label: String,
    origin: ViewOrigin,
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
        deps.entry(edge.to()).or_default().push(edge.from());
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

/// Builds nodes, collecting the index axes they use.
struct NodeBuilder<'a> {
    scope: &'a Scope<'a>,
    /// Definition ranges written once for several root declarations (the
    /// columns of a multi-declaration table): none of them owns the text.
    shared_definitions: HashSet<Span>,
    spellings: &'a HashMap<ResolvedDeclName, ExternalName>,
    axes: BTreeMap<ViewIndexId, ResolvedIndex>,
}

/// A node and where its value comes from.
type BuiltNode = (ViewNode, ValueBinding);

impl NodeBuilder<'_> {
    /// A declaration of the root file.
    fn local(
        &mut self,
        id: &ResolvedDeclName,
        deps: Vec<ViewNodeId>,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        let Some((dag, declaration)) = declaration_of(self.scope.tir, id) else {
            return Ok(None);
        };
        self.node(NodeInput {
            dag,
            declaration,
            label: id.as_str().to_string(),
            module: self.scope.module_name(self.scope.root()),
            origin: ViewOrigin::Local,
            is_public: self.scope.is_public(id.owner(), id.leaf()),
            deps,
            file: self.scope.sources.file_of(&id.owner().file_root()),
            value_of: id,
        })
        .map(Some)
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
            value_of: id,
        })
        .map(Some)
    }

    /// A declaration of another module the root reads.
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
            value_of: id,
        })
        .map(Some)
    }

    fn node(&mut self, input: NodeInput<'_>) -> Result<BuiltNode, ViewProjectionError> {
        let identity = input.declaration.identity();
        let (kind, span, definition, doc) = match input.declaration.kind() {
            DeclarationKind::Value(ValueDeclaration {
                category,
                span,
                definition,
                doc,
                ..
            }) => (kind_of(category), span, definition, doc),
            DeclarationKind::Assert { span, body, doc } => {
                (ViewNodeKind::Assert, span, Some(body), doc)
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
        // An assertion is a `Bool` check; it declares no type of its own.
        let dimension = match kind {
            ViewNodeKind::Assert => Some("Bool".to_string()),
            _ => decl_type.map(|ty| ty.element().format(self.scope.tir.registry())),
        };
        let unit = declared_unit(decl_type, &self.scope.units);

        // The definition is the source range after `=`, parentheses included.
        let expr = input
            .file
            .and_then(|file| definition.and_then(|span| slice(file.text, span)));
        // Only the module's own declarations are edited from its sheet.
        let local = input.origin == ViewOrigin::Local;
        let cells = if local {
            definition_cells(
                &self.scope.project.root_file().ast().declarations,
                input.declaration.name(),
            )
        } else {
            Vec::new()
        };
        let source = input.file.map(|file| ViewSource {
            file: Some(file.name.to_string()),
            line: file.lines.position(span.offset()).line.saturating_add(1),
            expr_range: definition
                .filter(|span| !self.shared_definitions.contains(span))
                .map(|span| file.lines.range(span.offset(), span.len()).into()),
            cells: cells
                .into_iter()
                .map(|cell| ViewCell {
                    keys: cell.keys.iter().map(ToString::to_string).collect(),
                    range: file.lines.range(cell.span.offset(), cell.span.len()).into(),
                })
                .collect(),
        });
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
            unit,
            // A parameter is an input even when its module exposes it.
            is_public: input.is_public && kind != ViewNodeKind::Param,
            deps: input.deps,
            expr,
            reduce: None,
            source,
            doc: doc.map(str::to_string),
            value: None,
        };
        Ok((node, binding))
    }

    /// Every index used as an axis, with its entries.
    fn indexes(&self) -> Vec<ViewIndex> {
        self.axes
            .iter()
            .map(|(id, axis)| ViewIndex {
                id: id.clone(),
                name: axis.to_string(),
                module: match axis {
                    ResolvedIndex::Concrete(name, _) => Some(self.scope.module_name(name.owner())),
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
    file: Option<SourceFile<'a>>,
    /// The identity whose evaluated value the node shows; it is the node's
    /// key too.
    value_of: &'a ResolvedDeclName,
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
}

impl<'e> EvaluatedValues<'e> {
    fn new(result: &'e EvalResult) -> Self {
        let by_identity = result
            .entries
            .iter()
            .filter_map(|(name, outcome, _)| {
                result
                    .identities
                    .get(name)
                    .map(|identity| (identity, outcome))
            })
            .collect();
        Self {
            result,
            by_identity,
        }
    }

    fn value(&self, id: &ResolvedDeclName) -> Option<&'e Value> {
        self.by_identity.get(id)?.as_ref().ok()
    }

    /// The outcome of a root-owned assertion.
    fn assertion(&self, id: &ResolvedDeclName) -> Option<&'e AssertResult> {
        let name = ScopedName::local(id.leaf().clone());
        self.result
            .assertions
            .iter()
            .find(|(assertion, _, _)| *assertion == name)
            .map(|(_, outcome, _)| outcome)
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

/// The unit an unevaluated quantity declaration would be shown in.
fn declared_unit(decl_type: Option<&ResolvedDeclType>, units: &CoherentUnits) -> Option<String> {
    match decl_type?.element() {
        ResolvedValueType::Quantity(ResolvedDim::Concrete(dimension)) => {
            units.of(dimension).map(str::to_string)
        }
        _ => None,
    }
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
