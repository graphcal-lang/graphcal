//! Include assembly and typed substitution for unfrozen per-DAG IR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::{Expr, ExprKind, TypeExpr};
use crate::graphcal_error::GraphcalError;
use crate::hir::expr::LocalDecl;
use crate::ir::instance::identity::{instance_declaration, projection_alias};
use crate::ir::instance::{
    InstanceAssertionProjection, InstancePlotProjection, InstanceRecord, InstanceValueProjection,
};
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{UnitName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;
use crate::syntax::type_name::ConstructorName;
use crate::syntax::visitor::ExprVisitor;

use super::{
    entry::{self, Decl, InScope},
    model::{IncludeAliasDeclaration, UnfrozenIR, UnfrozenSemanticInstance},
};

/// V005 obligations carried atomically from one include-site preflight into
/// its semantic instance edge.
#[derive(Debug, Clone, Default)]
pub struct IncludeOverrideReconciliations(
    HashMap<ResolvedDeclName, Vec<crate::ir::override_reconciliation::OverrideReconciliation>>,
);

/// Complete importer-side data needed to record one semantic include edge.
#[derive(Debug, Clone)]
pub struct SemanticInstanceInput {
    pub instance: InstanceRecord,
    pub value_bindings: HashMap<ResolvedDeclName, Expr>,
    pub runtime_unit_names: HashSet<UnitName>,
    pub output_projections: Vec<InstanceValueProjection>,
    pub assertion_projections: Vec<InstanceAssertionProjection>,
    pub plot_projections: Vec<InstancePlotProjection>,
    pub override_reconciliations: IncludeOverrideReconciliations,
}

impl UnfrozenIR {
    /// Typed Static ports authored directly by this reusable DAG template.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::source_interface::StaticPort] {
        &self.static_ports
    }

    /// Resolve an exposed plot alias to its canonical template declaration.
    #[must_use]
    pub fn plot_projection_target(&self, name: &DeclName) -> Option<LocalDecl> {
        self.decls
            .iter()
            .find_map(|decl| match decl {
                Decl::Plot(entry) if entry.name() == name => Some(LocalDecl::new(entry.identity())),
                _ => None,
            })
            .or_else(|| {
                self.semantic_instances.iter().find_map(|instance| {
                    instance
                        .plot_projections
                        .iter()
                        .find(|projection| &projection.alias == name)
                        .map(|projection| {
                            LocalDecl::new(instance_declaration(
                                instance.instance.id(),
                                projection.target.leaf().clone(),
                            ))
                        })
                })
            })
    }

    /// Names of assertions authored by this reusable DAG template.
    #[must_use]
    pub fn assertion_names(&self) -> Vec<DeclName> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Assert(entry) => Some(entry.name().clone()),
                _ => None,
            })
            .collect()
    }

    /// Local names of the value declarations in this IR, in source order,
    /// including selective include aliases.
    pub fn value_names(&self) -> impl Iterator<Item = &DeclName> {
        self.decls
            .iter()
            .filter(|decl| matches!(decl.category(), DeclCategory::Value(_)))
            .map(Decl::name)
    }

    fn params(&self) -> impl Iterator<Item = &entry::ParamEntry<entry::Syntax>> {
        self.decls.iter().filter_map(|decl| match decl {
            Decl::Param(entry) => Some(entry),
            _ => None,
        })
    }

    /// Record one include as a semantic edge rather than merging its syntax tree.
    pub fn record_semantic_instance(
        &mut self,
        input: SemanticInstanceInput,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
        if self
            .semantic_instances
            .iter()
            .any(|existing| existing.instance.id().owner() == input.instance.id().owner())
        {
            return Err(GraphcalError::InternalError {
                message: format!(
                    "duplicate semantic instance identity `{}`",
                    input.instance.id().owner()
                ),
                src: src.clone(),
                span: span.into(),
            });
        }
        self.semantic_instances.push(UnfrozenSemanticInstance {
            instance: input.instance,
            value_bindings: input.value_bindings,
            runtime_unit_names: input.runtime_unit_names,
            output_projections: input.output_projections,
            assertion_projections: input.assertion_projections,
            plot_projections: input.plot_projections,
            override_reconciliations: input.override_reconciliations.0,
        });
        Ok(())
    }

    /// Return the effective local declaration exposed by an include selector.
    ///
    /// This observes aliases created by earlier include/import composition, not
    /// only declarations authored directly in the producer's source AST.
    #[must_use]
    pub fn include_alias_declaration(&self, name: &DeclName) -> Option<IncludeAliasDeclaration> {
        self.decls.iter().find_map(|decl| {
            let (type_ann, is_const) = match decl {
                Decl::Const(entry) if entry.name() == name => (&entry.type_ann, true),
                Decl::Param(entry) if entry.name() == name => (&entry.type_ann, false),
                Decl::Node(entry) if entry.name() == name => (&entry.type_ann, false),
                _ => return None,
            };
            Some(IncludeAliasDeclaration {
                type_ann: type_ann.syntax.clone(),
                is_const,
            })
        })
    }

    /// Publish a synthesized Term alias as part of this module's external surface.
    pub fn export_term_alias(&mut self, name: DeclName) {
        self.external_surface.insert_explicit_export(name);
    }

    /// Expose public runtime units through one semantic instance namespace.
    ///
    /// Only a source-visible alias makes `alias.unit` spellable; an anonymous
    /// selective include exposes its units solely through projection aliases
    /// (see [`Self::add_dynamic_unit_projection_alias`]).
    pub fn add_semantic_dynamic_unit_bindings<'a>(
        &mut self,
        units: impl IntoIterator<Item = &'a UnitName>,
        instance: &crate::dag_id::InstanceId,
    ) {
        let Some(alias) = instance.scope().alias() else {
            return;
        };
        self.unit_bindings.extend(units.into_iter().map(|unit| {
            (
                UnitRef::qualified(
                    crate::syntax::non_empty::NonEmpty::singleton(alias.atom().clone()),
                    unit.clone(),
                ),
                instance_declaration(instance, unit.clone()),
            )
        }));
    }

    /// Add a local alias for a dynamic unit exposed by a recorded semantic
    /// instance. Units the instance does not expose are ignored.
    pub fn add_dynamic_unit_projection_alias(
        &mut self,
        instance_owner: &crate::dag_id::DagId,
        source: &UnitName,
        alias: UnitName,
    ) {
        let exposed = self
            .semantic_instances
            .iter()
            .find(|record| {
                record.instance.id().owner() == instance_owner
                    && record.runtime_unit_names.contains(source)
            })
            .map(|record| instance_declaration(record.instance.id(), source.clone()));
        if let Some(target) = exposed {
            self.unit_bindings.insert(UnitRef::local(alias), target);
        }
    }

    /// Add a const alias: a synthetic const declaration that references another const.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_const_alias(
        &mut self,
        name: DeclName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.decls.push(Decl::Const(entry::ConstEntry {
            identity: projection_alias(&body_resolution_owner, name),
            type_ann: InScope::new(type_ann, type_resolution_owner),
            expr: InScope::new(expr, body_resolution_owner),
            span,
        }));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: DeclName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.decls.push(Decl::Node(entry::NodeEntry {
            identity: projection_alias(&body_resolution_owner, name),
            type_ann: InScope::new(type_ann, type_resolution_owner),
            definition: InScope::new(
                crate::node_definition::NodeDefinition::Formula(expr),
                body_resolution_owner,
            ),
            span,
        }));
    }

    /// Record include-site nominal override obligations before substitution.
    ///
    /// Field and generic-argument ownership is checked later from canonical
    /// inferred HIR. Explicit index labels and constructors need a narrow
    /// registry-backed preflight because substitution can make HIR lowering
    /// reject them before TIR exists.
    pub fn include_override_reconciliations(
        &self,
        bindings: &HashMap<DeclName, Expr>,
        substitution: &crate::ir::static_substitution::StaticSubstitution,
        resolver: &crate::resolve::ModuleResolver,
        dependency_owner: &crate::dag_id::DagId,
        importer_src: &NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Result<IncludeOverrideReconciliations, GraphcalError> {
        self.params()
            .filter(|param| !bindings.contains_key(param.name()))
            .map(|param| {
                let mut reconciliations = param.override_reconciliations.clone();
                if let Some(default) = &param.default
                    && (!substitution.indexes.is_empty() || !substitution.types.is_empty())
                {
                    NominalOverridePreflight {
                        substitution,
                        resolver,
                        dependency_owner,
                        orphan_decl: param.name(),
                        importer_src,
                        include_span,
                    }
                    .visit_expr(&default.syntax)?;
                    reconciliations.push(
                        crate::ir::override_reconciliation::OverrideReconciliation::new(
                            param.identity(),
                            substitution,
                            importer_src.clone(),
                            include_span,
                        ),
                    );
                }
                Ok((param.identity(), reconciliations))
            })
            .filter_map(|result| match result {
                Ok((_, reconciliations)) if reconciliations.is_empty() => None,
                result => Some(result),
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map(IncludeOverrideReconciliations)
    }
}

/// Narrow pre-HIR guard for nominal syntax of the producer's parameter
/// defaults.
///
/// HIR lowering validates explicit labels and constructor patterns against
/// the substituted definitions. An incompatible replacement could therefore
/// fail before TIR ownership is available. Field and generic-argument
/// dependencies remain deferred to canonical inference.
struct NominalOverridePreflight<'a> {
    substitution: &'a crate::ir::static_substitution::StaticSubstitution,
    resolver: &'a crate::resolve::ModuleResolver,
    dependency_owner: &'a crate::dag_id::DagId,
    orphan_decl: &'a DeclName,
    importer_src: &'a NamedSource<Arc<String>>,
    include_span: Span,
}

impl NominalOverridePreflight<'_> {
    fn check_label(&self, index: &IndexName, detail: String) -> Result<(), GraphcalError> {
        let Ok(symbol) = self.resolver.resolve_index_path(
            self.dependency_owner,
            &crate::syntax::names::NamePath::local(index.atom().clone()),
        ) else {
            return Ok(());
        };
        if !self.substitution.indexes.contains_key(symbol.resolved()) {
            return Ok(());
        }
        Err(GraphcalError::IncludeMustReconcileOverride {
            overridden: index.to_string(),
            overridden_kind: "index".to_string(),
            orphan_decl: self.orphan_decl.to_string(),
            detail,
            src: self.importer_src.clone(),
            span: self.include_span.into(),
        })
    }

    fn check_constructor(
        &self,
        constructor: &ConstructorName,
        detail: String,
    ) -> Result<(), GraphcalError> {
        let Ok(symbol) = self.resolver.resolve_constructor_path(
            self.dependency_owner,
            &crate::syntax::names::NamePath::local(constructor.atom().clone()),
        ) else {
            return Ok(());
        };
        let owning_type = symbol.kind().owner_type();
        let owning_identity = symbol.owner_type_identity();
        if !self.substitution.types.contains_key(&owning_identity) {
            return Ok(());
        }
        Err(GraphcalError::IncludeMustReconcileOverride {
            overridden: owning_type.to_string(),
            overridden_kind: "type".to_string(),
            orphan_decl: self.orphan_decl.to_string(),
            detail,
            src: self.importer_src.clone(),
            span: self.include_span.into(),
        })
    }
}

impl ExprVisitor<crate::syntax::phase::Desugared> for NominalOverridePreflight<'_> {
    type Error = GraphcalError;

    fn visit_unresolved_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        let ExprKind::UnresolvedRef(reference) = &expr.kind else {
            return Ok(());
        };
        match reference {
            crate::syntax::ast::UnresolvedRef::IndexLabel { index, label, .. } => {
                let name = IndexName::classify(index.leaf().name.atom().clone());
                self.check_label(&name, format!("`{index}#{}`", label.value))
            }
            crate::syntax::ast::UnresolvedRef::Path(path) => {
                if let Some(name) = path.as_bare() {
                    let constructor = ConstructorName::classify(name.name.atom().clone());
                    self.check_constructor(&constructor, format!("constructor `{constructor}`"))?;
                }
                Ok(())
            }
        }
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::IndexAccess { args, .. } = &expr.kind {
            for arg in args {
                if let crate::desugar::desugared_ast::IndexArg::Variant { index, variant } = arg {
                    let name = IndexName::classify(index.value.leaf().clone());
                    self.check_label(&name, format!("`{}#{}`", index.value, variant.value))?;
                }
            }
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr,
        entries: &[crate::desugar::desugared_ast::MapEntry],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            for key in &entry.keys {
                if let crate::syntax::ast::MapEntryKey::Named { index, .. } = key {
                    let index_name = IndexName::classify(index.value.leaf().clone());
                    self.check_label(&index_name, format!("`{key}`"))?;
                }
            }
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_match(
        &mut self,
        _expr: &Expr,
        scrutinee: &Expr,
        arms: &[crate::desugar::desugared_ast::MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            match &arm.pattern {
                crate::desugar::desugared_ast::MatchPattern::IndexLabel {
                    index, variant, ..
                } => {
                    let name = IndexName::classify(index.value.leaf().clone());
                    self.check_label(&name, format!("`{}#{}`", index.value, variant.value))?;
                }
                crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                    if let Some(name) = path.as_bare() {
                        let constructor = ConstructorName::classify(name.name.atom().clone());
                        self.check_constructor(
                            &constructor,
                            format!("match constructor `{constructor}`"),
                        )?;
                    }
                }
                crate::desugar::desugared_ast::MatchPattern::Constructor { name, .. } => {
                    self.check_constructor(
                        &name.value,
                        format!("match constructor `{}`", name.value),
                    )?;
                }
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }

    fn visit_constructor_call(
        &mut self,
        expr: &Expr,
        fields: &[crate::desugar::desugared_ast::FieldInit],
    ) -> Result<(), Self::Error> {
        if let ExprKind::ConstructorCall { callee, .. } = &expr.kind
            && let Some(name) = callee.as_bare()
        {
            let constructor = ConstructorName::classify(name.name.atom().clone());
            self.check_constructor(&constructor, format!("constructor `{constructor}(...)`"))?;
        }
        fields
            .iter()
            .try_for_each(|field| self.visit_expr(&field.value))
    }
}
