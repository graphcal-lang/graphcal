//! Projection of a checked project (and optionally its evaluation) into a
//! [`ViewModel`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::ir::instance::{ExposedValueBody, InstanceProjection, ProjectionExposure};
use graphcal_compiler::ir::prelude_definitions::PreludeDefinitionError;
use graphcal_compiler::resolved_name::{ResolvedDeclName, ResolvedIndexName};
use graphcal_compiler::source_registry::ForeignSourceId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::text_position::Utf16LineIndex;
use graphcal_compiler::tir::typed::declaration_view::{
    DeclarationKind, DeclarationView, ValueDeclaration,
};
use graphcal_compiler::tir::typed::{
    CheckedDag, CheckedDeclType, CheckedTir, ResolvedDeclType, ResolvedDim, ResolvedIndex,
    ResolvedValueType,
};
use graphcal_eval::eval::{
    AssertResult, DisplayProjectionError, EvalResult, OutputUnavailable, Value,
};
use thiserror::Error;

use crate::graph_ir::{GraphClusterKind, GraphIr, GraphProjectionError, project_tir};
use crate::loader::{LoadedFile, LoadedProject};
use crate::project_compiler::CheckedProject;

use super::model::{
    ViewIndex, ViewModel, ViewModule, ViewNode, ViewNodeKind, ViewSource, ViewValue,
};
use super::value::{CoherentUnits, common_unit, view_value};

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
}

/// Project the structure of the root file of `checked`.
///
/// Every node comes without its value: attach an evaluation with
/// [`ViewProjection::attach_evaluation`], or take the structure alone for a
/// library file or an unevaluated project. `root_dir` is the project root
/// that source paths are made relative to; without it, paths are the
/// loader's diagnostic names.
///
/// # Errors
///
/// Returns [`ViewProjectionError`] when the checked program violates a
/// projection invariant.
pub fn project_view(
    project: &LoadedProject,
    checked: &CheckedProject,
    root_dir: Option<&Path>,
) -> Result<ViewProjection, ViewProjectionError> {
    let tir = checked.tir();
    let ir = project_tir(tir)?;
    let scope = Scope::new(project, checked, &ir, root_dir)?;
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
        scope: &scope,
        spellings: &spellings,
        axes: BTreeSet::new(),
    };
    let mut nodes = Vec::new();
    let mut bindings = Vec::new();
    for id in &locals {
        let deps = deps.get(id).map_or_else(Vec::new, |deps| {
            deps.iter().map(|dep| builder.render_id(dep)).collect()
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

    let root = tir.root_dag_id();
    Ok(ViewProjection {
        model: ViewModel {
            version: 0,
            module: ViewModule {
                id: scope.module_name(root),
                file: scope.sources.name_of(project.root_file()).to_string(),
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
        root_dir: Option<&Path>,
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
            sources: SourceFiles::new(project, checked, root_dir)?,
            units: CoherentUnits::from_prelude()?,
        })
    }

    fn root(&self) -> &'p DagId {
        self.tir.root_dag_id()
    }

    /// Whether `owner` is the root file or an inline DAG declared in it.
    fn is_local(&self, owner: &DagId) -> bool {
        matches!(
            self.clusters.get(owner),
            Some(GraphClusterKind::SourceModule)
        ) && owner.file_root() == self.root().file_root()
    }

    /// The module a declaration is presented under: the template of an
    /// include instance, otherwise its owner.
    fn module_of(&self, owner: &DagId) -> DagId {
        match self.clusters.get(owner) {
            Some(GraphClusterKind::Instance { template }) => template.clone(),
            _ => owner.clone(),
        }
    }

    /// The module id as a module path spells it (`power_project.eps`): the
    /// path importers write, or for a module no file imports (the root), its
    /// identity below the root package's source directory. A concrete
    /// instance keeps its canonical identity.
    fn module_name(&self, module: &DagId) -> String {
        if let Some(path) = self.checked.module_paths().module(module) {
            return path.to_string();
        }
        let Some(key) = module.module_path_key() else {
            return module.to_string();
        };
        let source_dir_depth = self
            .project
            .root_source_dir()
            .map_or(0, |dir| dir.components().count());
        let segments: Vec<&str> = key
            .segments()
            .iter()
            .map(graphcal_compiler::syntax::names::NameAtom::as_str)
            .skip(source_dir_depth)
            .collect();
        if segments.is_empty() {
            module.to_string()
        } else {
            segments.join(".")
        }
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

    /// The root's value and assertion declarations, with those of its inline
    /// DAGs, in source order. (An include alias is declared where the
    /// `include` is written, not where the DAG lists it.)
    fn local_declarations(&self) -> Vec<ResolvedDeclName> {
        let mut declarations: Vec<(usize, ResolvedDeclName)> = self
            .clusters
            .keys()
            .filter(|owner| self.is_local(owner))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|owner| self.tir.dag_registry().get(owner))
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
                        instance: instance_label(record, template),
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
                    id: name.to_string(),
                    instance: None,
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
                        id: record
                            .instance
                            .exposed_name(projection.projection)
                            .to_string(),
                        instance: Some(instance_label(record, template)),
                    },
                );
            }
        }
        spellings
    }
}

/// The name the view shows for an include instance: its alias
/// (`include lib() as l`), or the template's file name when the include
/// is anonymous.
fn instance_label(
    record: &graphcal_compiler::ir::instance::HirInstanceRecord,
    template: &DagId,
) -> String {
    record
        .instance
        .id()
        .scope()
        .alias()
        .map_or_else(|| template.leaf().to_string(), ToString::to_string)
}

/// A root declaration standing for an include instance's output.
struct IncludeAlias {
    /// The instance declaration the alias reads.
    target: ResolvedDeclName,
    /// The template the instance was materialized from.
    template: DagId,
    /// The instance's name in the view.
    instance: String,
}

/// How the root spells a declaration of another module.
struct ExternalName {
    id: String,
    /// The include instance it is read through, if any.
    instance: Option<String>,
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
    spellings: &'a HashMap<ResolvedDeclName, ExternalName>,
    axes: BTreeSet<ResolvedIndexName>,
}

/// A node and where its value comes from.
type BuiltNode = (ViewNode, ValueBinding);

impl NodeBuilder<'_> {
    /// The id of a dependency as the root spells it.
    fn render_id(&self, id: &ResolvedDeclName) -> String {
        if self.scope.is_local(id.owner()) {
            return id.as_str().to_string();
        }
        self.spellings.get(id).map_or_else(
            || {
                format!(
                    "{}::{}",
                    self.scope.module_of(id.owner()).leaf(),
                    id.as_str()
                )
            },
            |name| name.id.clone(),
        )
    }

    /// A declaration of the root file.
    fn local(
        &mut self,
        id: &ResolvedDeclName,
        deps: Vec<String>,
    ) -> Result<Option<BuiltNode>, ViewProjectionError> {
        let Some((dag, declaration)) = declaration_of(self.scope.tir, id) else {
            return Ok(None);
        };
        self.node(NodeInput {
            dag,
            declaration,
            id: id.as_str().to_string(),
            module: self.scope.module_name(self.scope.root()),
            kind: None,
            instance: None,
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
            id: id.as_str().to_string(),
            module: self.scope.module_name(&alias.template),
            kind: Some(ViewNodeKind::External),
            instance: Some(alias.instance.clone()),
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
        self.node(NodeInput {
            dag,
            declaration,
            id: self.render_id(id),
            module: self.scope.module_name(&module),
            kind: Some(ViewNodeKind::External),
            instance: self
                .spellings
                .get(id)
                .and_then(|name| name.instance.clone()),
            is_public: self.scope.is_public(&module, id.leaf()),
            deps: Vec::new(),
            file: self.scope.sources.file_of(&module.file_root()),
            value_of: id,
        })
        .map(Some)
    }

    fn node(&mut self, input: NodeInput<'_>) -> Result<BuiltNode, ViewProjectionError> {
        let identity = input.declaration.identity();
        let (declared_kind, span, definition, doc) = match input.declaration.kind() {
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
                    if let ResolvedIndex::Concrete(name, _) = axis {
                        self.axes.insert(name.clone());
                    }
                    axis.to_string()
                })
                .collect()
        });
        // An assertion is a `Bool` check; it declares no type of its own.
        let dimension = match declared_kind {
            ViewNodeKind::Assert => Some("Bool".to_string()),
            _ => decl_type.map(|ty| ty.element().format(self.scope.tir.registry())),
        };
        let unit = declared_unit(decl_type, &self.scope.units);

        // The defining expression ends where the declaration ends: HIR spans
        // of parenthesized operands omit their parentheses, so the
        // declaration span, not the expression's, bounds the text.
        let definition = input
            .file
            .and_then(|file| definition.map(|start| definition_span(file.text, start, span)));
        let expr = input
            .file
            .and_then(|file| definition.and_then(|span| slice(file.text, span)));
        let source = input.file.map(|file| ViewSource {
            file: Some(file.name.to_string()),
            line: file.lines.position(span.offset()).line.saturating_add(1),
            expr_range: definition.map(|span| file.lines.range(span.offset(), span.len()).into()),
        });

        let binding = ValueBinding {
            declaration: input.value_of.clone(),
            kind: match declared_kind {
                ViewNodeKind::Assert => ValueBindingKind::Assertion,
                _ => ValueBindingKind::Value,
            },
        };
        let node = ViewNode {
            id: input.id,
            name: input.declaration.name().as_str().to_string(),
            module: input.module,
            kind: input.kind.unwrap_or(declared_kind),
            axes,
            dimension,
            unit,
            // A parameter is an input even when its module exposes it.
            is_public: input.is_public && declared_kind != ViewNodeKind::Param,
            deps: input.deps,
            expr,
            reduce: None,
            instance: input.instance,
            source,
            doc: doc.map(str::to_string),
            value: None,
        };
        Ok((node, binding))
    }

    /// Every index used as an axis, with its labels.
    fn indexes(&self) -> Vec<ViewIndex> {
        self.axes
            .iter()
            .map(|name| ViewIndex {
                name: name.as_str().to_string(),
                module: self.scope.module_name(name.owner()),
                labels: self
                    .scope
                    .tir
                    .declared_index_def(name)
                    .map_or_else(Vec::new, |def| {
                        def.entry_keys().iter().map(ToString::to_string).collect()
                    }),
            })
            .collect()
    }
}

/// What one node is built from.
struct NodeInput<'a> {
    /// The DAG whose declaration is presented.
    dag: &'a CheckedDag,
    declaration: DeclarationView<'a>,
    id: String,
    module: String,
    /// Overrides the kind derived from the declaration (for externals).
    kind: Option<ViewNodeKind>,
    instance: Option<String>,
    is_public: bool,
    deps: Vec<String>,
    file: Option<SourceFile<'a>>,
    /// The identity whose evaluated value the node shows.
    value_of: &'a ResolvedDeclName,
}

fn declaration_of<'t>(
    tir: &'t CheckedTir,
    id: &ResolvedDeclName,
) -> Option<(&'t CheckedDag, DeclarationView<'t>)> {
    let dag = tir.dag_registry().get(id.owner())?;
    let declaration = dag.declaration(id)?;
    Some((dag, declaration))
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
    /// Each file's line index: one scan per file, not one per declaration.
    lines: HashMap<&'p DagId, Utf16LineIndex<'p>>,
}

impl<'p> SourceFiles<'p> {
    fn new(
        project: &'p LoadedProject,
        checked: &CheckedProject,
        root_dir: Option<&Path>,
    ) -> Result<Self, ForeignSourceId> {
        let mut names = HashMap::new();
        let mut lines = HashMap::new();
        for file in project.files().iter() {
            // The name is a project-relative path with `/` on every platform: it is
            // the wire format the view and the editors key their files by.
            let relative = root_dir
                .and_then(|root| file.path().strip_prefix(root).ok())
                .map(|path| {
                    path.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                });
            let name = match relative {
                Some(relative) => relative,
                None => checked
                    .sources()
                    .named_source(file.source_id())?
                    .name()
                    .to_string(),
            };
            names.insert(file.dag_id(), name);
            lines.insert(file.dag_id(), Utf16LineIndex::new(file.source().as_str()));
        }
        Ok(Self {
            project,
            names,
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

    fn name_of(&self, file: &LoadedFile) -> &str {
        self.names.get(file.dag_id()).map_or("", String::as_str)
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

/// The span from the start of `definition` to the end of `declaration`,
/// less the terminating `;` and trailing whitespace.
fn definition_span(text: &str, definition: Span, declaration: Span) -> Span {
    let start = definition.offset().min(text.len());
    let end = declaration
        .offset()
        .saturating_add(declaration.len())
        .max(definition.offset().saturating_add(definition.len()))
        .clamp(start, text.len());
    let trimmed = text[start..end].trim_end();
    let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed).trim_end();
    Span::new(start, trimmed.len())
}

fn slice(text: &str, span: Span) -> Option<String> {
    text.get(span.offset()..span.offset().checked_add(span.len())?)
        .map(str::to_string)
}
