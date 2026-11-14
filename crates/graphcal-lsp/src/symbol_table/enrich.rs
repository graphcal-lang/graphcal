//! TIR enrichment: hover types and details from a checked program.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir;
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::display::formatting_registry::FormattingRegistry;
use graphcal_compiler::display::unit_label::format_unit_terms_with_config;
use graphcal_compiler::semantic::index_def::{ConcreteIndexKind, IndexKind, RequiredIndexKind};
use graphcal_compiler::semantic::unit_scale::UnitScale;
use graphcal_compiler::tir::typed::{CheckedTir, ResolvedDeclType, ResolvedDomainBound};
use graphcal_eval::eval::format_number;

use crate::symbol_identity::{FieldId, SymbolId};

use super::index::{DefinitionInfo, SymbolCategory, SymbolTable};

/// Enrich a symbol table with type information from a TIR.
///
/// `dag_id` selects which DAG's resolved declaration types to use — the
/// analyzed file's own id for the root table, the dependency's id for an
/// imported file's table. Using the root unconditionally left every
/// imported declaration without a type (`(unknown type)` hover, #831):
/// the type information lives on the dep's merged [`DagTIR`], not the
/// root's.
#[expect(
    clippy::too_many_lines,
    reason = "linear match over all symbol categories"
)]
pub fn enrich_from_tir(table: &mut SymbolTable, tir: &CheckedTir, dag_id: &DagId) {
    let registry = tir.registry();

    // Enrich param/node/const declarations with resolved types + constraints:
    // the file's own, and those of the inline `dag` templates it writes.
    let file_dags = tir
        .dag_registry()
        .get(dag_id)
        .into_iter()
        .chain(tir.inline_dag_templates(dag_id));
    for dag in file_dags {
        for (identity, annotation) in dag.value_decl_types() {
            let resolved_type = annotation.checked().resolved();
            let type_desc = dag.domain_bounds(&identity);
            let key = SymbolId::Declaration(identity);
            if let Some(def) = table.definitions.get_mut(&key) {
                let type_desc = type_desc.map_or_else(
                    || resolved_type.format(registry),
                    |constraints| {
                        format_type_with_constraints(
                            resolved_type,
                            constraints.as_slice(),
                            registry,
                        )
                    },
                );
                def.type_description = Some(type_desc);
            }
        }
    }

    // Enrich dimension/unit/index/struct definitions from registry.
    // Collect keys and categories first to avoid cloning the entire HashMap.
    let definition_keys: Vec<(SymbolId, SymbolCategory)> = table
        .definitions
        .iter()
        .map(|(k, d)| (k.clone(), d.category))
        .collect();
    for (key, category) in &definition_keys {
        match (key, category) {
            (SymbolId::Dimension(resolved), SymbolCategory::Dimension) => {
                let name = resolved.as_str();
                if let Some(dim) = tir.dimension(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    def_mut.type_description = Some(format!(
                        "dim {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            (SymbolId::Unit(resolved), SymbolCategory::Unit) => {
                if let Some(unit_info) = tir.unit_info(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let (timing, scale_str) = match &unit_info.scale {
                        UnitScale::Const(s) => ("const", format!("{s}")),
                        UnitScale::Runtime(s) => ("runtime", format!("{s}")),
                        UnitScale::Dynamic { .. } => ("runtime", "dynamic".to_string()),
                    };
                    def_mut.type_description = Some(format!(
                        "{}, {timing}, scale = {scale_str}",
                        registry.dimensions.format_dimension(&unit_info.dimension),
                    ));
                }
            }
            (SymbolId::Index(resolved), SymbolCategory::Index) => {
                if let Some(idx_def) = tir.declared_index_def(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    match &idx_def.kind {
                        IndexKind::Concrete(ConcreteIndexKind::Named { variants }) => {
                            let vs: Vec<&str> = variants
                                .iter()
                                .map(
                                    graphcal_compiler::syntax::index_name::IndexVariantName::as_str,
                                )
                                .collect();
                            def_mut.type_description = Some(format!("{{ {} }}", vs.join(", ")));
                        }
                        IndexKind::Concrete(ConcreteIndexKind::Coordinate(data)) => {
                            def_mut.type_description = Some(match data.spacing() {
                                graphcal_compiler::semantic::index_def::CoordinateSpacing::Step {
                                    step,
                                } => {
                                    format!("range({}, {}, step: {step})", data.start(), data.end())
                                }
                                graphcal_compiler::semantic::index_def::CoordinateSpacing::Linspace => {
                                    format!(
                                        "linspace({}, {}, points: {})",
                                        data.start(),
                                        data.end(),
                                        data.cardinality()
                                    )
                                }
                            });
                        }
                        IndexKind::Required(RequiredIndexKind::Named) => {
                            def_mut.type_description = Some("(required)".to_string());
                        }
                        IndexKind::Required(RequiredIndexKind::Coordinate { dimension }) => {
                            def_mut.type_description = Some(format!(
                                "(required, dim: {})",
                                registry.dimensions.format_dimension(dimension)
                            ));
                        }
                        IndexKind::Concrete(ConcreteIndexKind::Finite { index }) => {
                            def_mut.type_description = Some(index.to_string());
                        }
                    }
                }
            }
            (SymbolId::StructType(resolved), SymbolCategory::StructType) => {
                if let Some(type_def) = tir.struct_type_def(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let desc = type_def.union_members().map_or_else(
                        || format!("{} (required)", type_def.name()),
                        |members| match type_def.record_fields() {
                            Some([]) => type_def.name().to_string(),
                            Some(fields) => {
                                let field_descs: Vec<String> = fields
                                    .iter()
                                    .map(|field| field.name().to_string())
                                    .collect();
                                format!("{} {{ {} }}", type_def.name(), field_descs.join(", "))
                            }
                            None => {
                                // Union type: show members separated by |
                                let member_descs: Vec<String> = members
                                    .iter()
                                    .map(|member| member.name().to_string())
                                    .collect();
                                member_descs.join(" | ")
                            }
                        },
                    );
                    def_mut.type_description = Some(desc);
                }
            }
            _ => {}
        }
    }

    // Register field definitions from struct types under their struct-qualified
    // keys so that pattern bindings (`@s match { Variant(field: v) => …}`)
    // resolve to the field of the right struct/variant, even when two structs
    // share a field name.
    for (identity, type_def) in tir
        .nominal_type_defs()
        .filter(|(identity, _)| identity.owner() == dag_id)
    {
        let Some(members) = type_def.union_members() else {
            continue;
        };
        for member in members {
            for field in member.fields() {
                let constructor = identity.constructor(member.name());
                let field_key = SymbolId::Field(FieldId::new(constructor, field.name().clone()));
                if !table.definitions.contains_key(&field_key) {
                    table.insert_definition(
                        field_key,
                        DefinitionInfo {
                            doc: None,
                            name: field.name().to_string(),
                            category: SymbolCategory::Field,
                            name_span: Span::new(0, 0),
                            decl_span: Span::new(0, 0),
                            type_description: None,
                            detail: Some(format!("field of {}.{}", type_def.name(), member.name())),
                            visibility: None,
                        },
                    );
                }
            }
        }
    }
}

/// Format a domain bound expression as a human-readable string.
///
/// Handles the common cases: number literals, quantity literals, and negated forms.
fn format_bound_expr(expr: &hir::expr::Expr) -> String {
    match expr.kind() {
        hir::expr::ExprKind::Number(value) => format_number(*value),
        hir::expr::ExprKind::Integer(value) => value.to_string(),
        hir::expr::ExprKind::QuantityLiteral { value, unit } => {
            let number = format_number(*value);
            let unit = format_unit_terms_with_config(
                unit.terms
                    .iter()
                    .map(|item| (item.op, item.name.value.to_string(), item.power)),
                true,
            );
            format!("{number} {unit}")
        }
        hir::expr::ExprKind::UnaryOp {
            op: graphcal_compiler::syntax::ast::UnaryOp::Neg,
            operand,
        } => format!("-{}", format_bound_expr(operand)),
        // For complex expressions, fall back to "..."
        _ => "...".to_string(),
    }
}

/// Format a constraint clause from domain bounds.
///
/// Returns a string like `(min: 100 kg, max: 2000 kg)` or an empty string if no constraints.
fn format_constraints(constraints: &[ResolvedDomainBound]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints
        .iter()
        .map(|b| format!("{}: {}", b.kind, format_bound_expr(&b.value)))
        .collect();
    format!("({})", parts.join(", "))
}

/// Format a resolved type expression with domain constraints.
///
/// For indexed types like `Velocity[Maneuver]`, inserts the constraint clause
/// between the base type and the index suffix: `Velocity(min: 0 m/s)[Maneuver]`.
fn format_type_with_constraints(
    resolved: &ResolvedDeclType,
    constraints: &[ResolvedDomainBound],
    registry: &FormattingRegistry,
) -> String {
    let constraint_str = format_constraints(constraints);
    match resolved {
        ResolvedDeclType::Indexed { element, indexes } => {
            let base_str = element.format(registry);
            let idx_strs: Vec<String> = indexes.iter().map(ToString::to_string).collect();
            format!("{base_str}{constraint_str}[{}]", idx_strs.join(", "))
        }
        ResolvedDeclType::Value(value_type) => {
            let type_str = value_type.format(registry);
            format!("{type_str}{constraint_str}")
        }
    }
}
