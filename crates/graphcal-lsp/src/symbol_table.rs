//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;

use graphcal_syntax::ast::{
    DeclKind, DimExpr, DomainBound, ExprKind, FnBody, IndexDeclKind, PatternBinding, TypeExpr,
    TypeExprKind, UnitExpr,
};
use graphcal_syntax::span::Span;

use graphcal_eval::builtins::{builtin_constants, builtin_functions};
use graphcal_eval::eval::format_number;
use graphcal_eval::registry::{IndexKind, Registry};
use graphcal_eval::tir::{ResolvedIndex, ResolvedTypeExpr, TIR};

/// The category of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Param,
    Node,
    Const,
    Dimension,
    Unit,
    StructType,
    Function,
    Index,
    IndexVariant,
    Field,
    LocalVar,
    BuiltinFn,
    BuiltinConst,
    Assert,
}

/// Information about a symbol definition.
#[derive(Debug, Clone)]
pub struct DefinitionInfo {
    pub name: String,
    pub category: SymbolCategory,
    /// Span of just the name token.
    pub name_span: Span,
    /// Span of the full declaration.
    pub decl_span: Span,
    /// Human-readable type/signature for hover (populated from TIR).
    pub type_description: Option<String>,
    /// Additional detail for hover.
    pub detail: Option<String>,
}

/// A reference occurrence: a name that refers to a definition.
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// Byte-offset span of this reference in the current file.
    pub span: Span,
    /// Key into `definitions` that this reference points to.
    pub target: String,
}

/// The complete symbol table for one file.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// All symbol definitions keyed by a unique ID string.
    pub definitions: HashMap<String, DefinitionInfo>,
    /// All reference occurrences sorted by span offset.
    pub references: Vec<ReferenceInfo>,
}

impl SymbolTable {
    /// Find the reference at a given byte offset, if any.
    pub fn find_reference_at(&self, offset: usize) -> Option<&ReferenceInfo> {
        // Binary search for a reference whose span contains the offset.
        let idx = self
            .references
            .binary_search_by(|r| {
                if offset < r.span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= r.span.offset() + r.span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.references[idx])
    }

    /// Find the definition whose name span contains the given byte offset, if any.
    pub fn find_definition_at(&self, offset: usize) -> Option<&DefinitionInfo> {
        self.definitions.values().find(|d| {
            offset >= d.name_span.offset() && offset < d.name_span.offset() + d.name_span.len()
        })
    }

    /// Find all references that point to the given target name.
    pub fn find_all_references(&self, target: &str) -> Vec<&ReferenceInfo> {
        self.references
            .iter()
            .filter(|r| r.target == target)
            .collect()
    }
}

/// Scope stack for tracking local variable bindings.
struct ScopeStack {
    /// Each scope maps local name -> definition key in the symbol table.
    scopes: Vec<HashMap<String, String>>,
}

impl ScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, key: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, key);
        }
    }

    fn resolve(&self, name: &str) -> Option<&str> {
        for scope in self.scopes.iter().rev() {
            if let Some(key) = scope.get(name) {
                return Some(key);
            }
        }
        None
    }
}

/// Build a symbol table from a parsed AST file.
#[expect(
    clippy::too_many_lines,
    reason = "declaration walker needs to handle every DeclKind variant"
)]
pub fn build_from_ast(ast: &graphcal_syntax::ast::File) -> SymbolTable {
    let mut table = SymbolTable::default();
    let mut scopes = ScopeStack::new();

    // Add builtin constants as definitions.
    for name in builtin_constants().keys() {
        table.definitions.insert(
            (*name).to_string(),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
            },
        );
    }

    // Add builtin functions as definitions.
    for (name, f) in &builtin_functions() {
        table.definitions.insert(
            (*name).to_string(),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(format!("builtin, arity {}", f.arity())),
            },
        );
    }

    for decl in &ast.declarations {
        // Collect references from #[assumes(...)] attribute arguments.
        for attr in &decl.attributes {
            if attr.name.name == "assumes" {
                for arg in &attr.args {
                    if let Some(ident) = arg.as_single_ident() {
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target: ident.name.clone(),
                        });
                    }
                }
            }
        }
        match &decl.kind {
            DeclKind::Param(p) => {
                let name = p.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Param,
                        name_span: p.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                collect_type_expr_refs(&p.type_ann, &mut table);
                if let Some(ref value) = p.value {
                    collect_expr_refs(value, &mut table, &mut scopes);
                }
            }
            DeclKind::Node(n) => {
                let name = n.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Node,
                        name_span: n.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                collect_type_expr_refs(&n.type_ann, &mut table);
                collect_expr_refs(&n.value, &mut table, &mut scopes);
            }
            DeclKind::Const(c) => {
                let name = c.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Const,
                        name_span: c.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                collect_type_expr_refs(&c.type_ann, &mut table);
                collect_expr_refs(&c.value, &mut table, &mut scopes);
            }
            DeclKind::Dimension(d) => {
                let name = d.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Dimension,
                        name_span: d.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                if let Some(dim_expr) = &d.definition {
                    collect_dim_expr_refs(dim_expr, &mut table);
                }
            }
            DeclKind::Unit(u) => {
                let name = u.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Unit,
                        name_span: u.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                collect_dim_expr_refs(&u.dim_type, &mut table);
                if let Some(unit_def) = &u.definition {
                    collect_unit_expr_refs(&unit_def.unit_expr, &mut table);
                }
            }
            DeclKind::Type(t) => {
                let name = t.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::StructType,
                        name_span: t.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                // Add variants (only if more than one, i.e., tagged union, not struct sugar).
                if t.variants.len() > 1 {
                    for variant in &t.variants {
                        let vname = variant.name.value.to_string();
                        let key = format!("{name}::{vname}");
                        table.definitions.insert(
                            key,
                            DefinitionInfo {
                                name: vname,
                                category: SymbolCategory::IndexVariant,
                                name_span: variant.name.span,
                                decl_span: variant.span,
                                type_description: None,
                                detail: Some(format!("variant of {name}")),
                            },
                        );
                    }
                }
                // Walk field type annotations.
                for variant in &t.variants {
                    for field in &variant.fields {
                        collect_type_expr_refs(&field.type_ann, &mut table);
                    }
                }
            }
            DeclKind::Fn(f) => {
                let fname = f.name.value.to_string();
                table.definitions.insert(
                    fname.clone(),
                    DefinitionInfo {
                        name: fname.clone(),
                        category: SymbolCategory::Function,
                        name_span: f.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );

                // Push scope for function params.
                scopes.push();
                for param in &f.params {
                    let pname = param.name.name.clone();
                    let key = format!("{fname}::{pname}");
                    table.definitions.insert(
                        key.clone(),
                        DefinitionInfo {
                            name: pname.clone(),
                            category: SymbolCategory::LocalVar,
                            name_span: param.name.span,
                            decl_span: param.name.span,
                            type_description: None,
                            detail: Some(format!("parameter of fn {fname}")),
                        },
                    );
                    scopes.insert(pname, key);
                    collect_type_expr_refs(&param.type_ann, &mut table);
                }

                collect_type_expr_refs(&f.return_type, &mut table);

                match &f.body {
                    FnBody::Short(expr) => {
                        collect_expr_refs(expr, &mut table, &mut scopes);
                    }
                    FnBody::Block { stmts, expr } => {
                        scopes.push();
                        for stmt in stmts {
                            collect_expr_refs(&stmt.value, &mut table, &mut scopes);
                            let lname = stmt.name.name.clone();
                            let key = format!("{fname}::{lname}");
                            table.definitions.insert(
                                key.clone(),
                                DefinitionInfo {
                                    name: lname.clone(),
                                    category: SymbolCategory::LocalVar,
                                    name_span: stmt.name.span,
                                    decl_span: stmt.span,
                                    type_description: None,
                                    detail: None,
                                },
                            );
                            scopes.insert(lname, key);
                            if let Some(type_ann) = &stmt.type_ann {
                                collect_type_expr_refs(type_ann, &mut table);
                            }
                        }
                        collect_expr_refs(expr, &mut table, &mut scopes);
                        scopes.pop();
                    }
                }
                scopes.pop();
            }
            DeclKind::Index(idx) => {
                let name = idx.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Index,
                        name_span: idx.name.span,
                        decl_span: decl.span,
                        type_description: None,
                        detail: None,
                    },
                );
                if let IndexDeclKind::Named { variants } = &idx.kind {
                    for variant in variants {
                        let vname = variant.value.to_string();
                        let key = format!("{name}::{vname}");
                        table.definitions.insert(
                            key,
                            DefinitionInfo {
                                name: vname,
                                category: SymbolCategory::IndexVariant,
                                name_span: variant.span,
                                decl_span: variant.span,
                                type_description: None,
                                detail: Some(format!("label/value variant of index {name}")),
                            },
                        );
                    }
                }
            }
            DeclKind::Assert(a) => {
                let name = a.name.value.to_string();
                table.definitions.insert(
                    name.clone(),
                    DefinitionInfo {
                        name: name.clone(),
                        category: SymbolCategory::Assert,
                        name_span: a.name.span,
                        decl_span: decl.span,
                        type_description: Some("Bool".to_string()),
                        detail: Some("assert".to_string()),
                    },
                );
                // Walk assert body expressions
                match &a.body {
                    graphcal_syntax::ast::AssertBody::Expr(expr) => {
                        collect_expr_refs(expr, &mut table, &mut scopes);
                    }
                    graphcal_syntax::ast::AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => {
                        collect_expr_refs(actual, &mut table, &mut scopes);
                        collect_expr_refs(expected, &mut table, &mut scopes);
                        collect_expr_refs(tolerance, &mut table, &mut scopes);
                    }
                }
            }
            DeclKind::Import(u) => {
                // Each imported name is a reference; target resolution for cross-file
                // go-to-definition is handled separately.
                if let graphcal_syntax::ast::ImportKind::Selective(names) = &u.kind {
                    for import_item in names {
                        table.references.push(ReferenceInfo {
                            span: import_item.name.span,
                            target: import_item.name.name.clone(),
                        });
                        // If aliased, the alias also resolves to the same target.
                        if let Some(alias) = &import_item.alias {
                            table.references.push(ReferenceInfo {
                                span: alias.span,
                                target: import_item.name.name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort references by offset for binary search.
    table.references.sort_by_key(|r| r.span.offset());
    table
}

/// Collect references from an expression, tracking local scopes.
#[expect(
    clippy::too_many_lines,
    reason = "expression walker needs to handle every ExprKind variant"
)]
fn collect_expr_refs(
    expr: &graphcal_syntax::ast::Expr,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    match &expr.kind {
        ExprKind::GraphRef(name)
        | ExprKind::QualifiedGraphRef { name, .. }
        | ExprKind::ConstRef(name)
        | ExprKind::QualifiedConstRef { name, .. } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: name.value.to_string(),
            });
        }
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: name.value.to_string(),
            });
            for arg in args {
                collect_expr_refs(arg, table, scopes);
            }
        }
        ExprKind::LocalRef(ident) => {
            let target = scopes
                .resolve(&ident.name)
                .map_or_else(|| ident.name.clone(), ToString::to_string);
            table.references.push(ReferenceInfo {
                span: ident.span,
                target,
            });
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_expr_refs(lhs, table, scopes);
            collect_expr_refs(rhs, table, scopes);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_expr_refs(operand, table, scopes);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_refs(condition, table, scopes);
            collect_expr_refs(then_branch, table, scopes);
            collect_expr_refs(else_branch, table, scopes);
        }
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::Convert { expr, target } => {
            collect_expr_refs(expr, table, scopes);
            collect_unit_expr_refs(target, table);
        }
        ExprKind::DisplayTimezone { expr, .. } => {
            collect_expr_refs(expr, table, scopes);
        }
        ExprKind::AsCast { expr, target_type } => {
            collect_expr_refs(expr, table, scopes);
            collect_type_expr_refs(target_type, table);
        }
        ExprKind::Block { stmts, expr } => {
            scopes.push();
            let scope_prefix = format!("block@{}", expr.span.offset());
            for stmt in stmts {
                collect_expr_refs(&stmt.value, table, scopes);
                let lname = stmt.name.name.clone();
                let key = format!("{scope_prefix}::{lname}");
                table.definitions.insert(
                    key.clone(),
                    DefinitionInfo {
                        name: lname.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: stmt.name.span,
                        decl_span: stmt.span,
                        type_description: None,
                        detail: None,
                    },
                );
                scopes.insert(lname, key);
                if let Some(type_ann) = &stmt.type_ann {
                    collect_type_expr_refs(type_ann, table);
                }
            }
            collect_expr_refs(expr, table, scopes);
            scopes.pop();
        }
        ExprKind::FieldAccess { expr, field } => {
            collect_expr_refs(expr, table, scopes);
            // Field reference -- target is approximate without type info.
            table.references.push(ReferenceInfo {
                span: field.span,
                target: format!("field::{}", field.value),
            });
        }
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } => {
            table.references.push(ReferenceInfo {
                span: type_name.span,
                target: type_name.value.to_string(),
            });
            for type_arg in type_args {
                collect_type_expr_refs(type_arg, table);
            }
            for field in fields {
                if let Some(value) = &field.value {
                    collect_expr_refs(value, table, scopes);
                } else {
                    // Shorthand: `{ dv1 }` -- the field name is also a local reference.
                    let target = scopes
                        .resolve(field.name.value.as_str())
                        .map_or_else(|| field.name.value.to_string(), ToString::to_string);
                    table.references.push(ReferenceInfo {
                        span: field.name.span,
                        target,
                    });
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            // For TableLiteral, also add references for the index names in table[...].
            if let ExprKind::TableLiteral { indexes, .. } = &expr.kind {
                for idx in indexes {
                    table.references.push(ReferenceInfo {
                        span: idx.span,
                        target: idx.value.to_string(),
                    });
                }
            }
            for entry in entries {
                for key in &entry.keys {
                    table.references.push(ReferenceInfo {
                        span: key.index.span,
                        target: key.index.value.to_string(),
                    });
                    let variant_key = format!("{}::{}", key.index.value, key.variant.value);
                    table.references.push(ReferenceInfo {
                        span: key.variant.span,
                        target: variant_key,
                    });
                }
                collect_expr_refs(&entry.value, table, scopes);
            }
        }
        ExprKind::ForComp { bindings, body } => {
            scopes.push();
            for binding in bindings {
                table.references.push(ReferenceInfo {
                    span: binding.index.span,
                    target: binding.index.value.to_string(),
                });
                let var_name = binding.var.name.clone();
                let key = format!("for@{}::{var_name}", binding.var.span.offset());
                table.definitions.insert(
                    key.clone(),
                    DefinitionInfo {
                        name: var_name.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: binding.var.span,
                        decl_span: binding.var.span,
                        type_description: None,
                        detail: Some(format!("loop variable over {}", binding.index.value)),
                    },
                );
                scopes.insert(var_name, key);
            }
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::IndexAccess { expr, args } => {
            collect_expr_refs(expr, table, scopes);
            for arg in args {
                match arg {
                    graphcal_syntax::ast::IndexArg::Variant { index, variant } => {
                        table.references.push(ReferenceInfo {
                            span: index.span,
                            target: index.value.to_string(),
                        });
                        let variant_key = format!("{}::{}", index.value, variant.value);
                        table.references.push(ReferenceInfo {
                            span: variant.span,
                            target: variant_key,
                        });
                    }
                    graphcal_syntax::ast::IndexArg::Var(ident) => {
                        let target = scopes
                            .resolve(&ident.name)
                            .map_or_else(|| ident.name.clone(), ToString::to_string);
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target,
                        });
                    }
                }
            }
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            collect_expr_refs(source, table, scopes);
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let acc_key = format!("scan@{}::acc", expr.span.offset());
            let val_key = format!("scan@{}::val", expr.span.offset());
            table.definitions.insert(
                acc_key.clone(),
                DefinitionInfo {
                    name: acc_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: acc_name.span,
                    decl_span: acc_name.span,
                    type_description: None,
                    detail: Some("scan accumulator".to_string()),
                },
            );
            table.definitions.insert(
                val_key.clone(),
                DefinitionInfo {
                    name: val_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: val_name.span,
                    decl_span: val_name.span,
                    type_description: None,
                    detail: Some("scan value".to_string()),
                },
            );
            scopes.insert(acc_name.name.clone(), acc_key);
            scopes.insert(val_name.name.clone(), val_key);
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let prev_key = format!("unfold@{}::prev", expr.span.offset());
            let curr_key = format!("unfold@{}::curr", expr.span.offset());
            table.definitions.insert(
                prev_key.clone(),
                DefinitionInfo {
                    name: prev_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: prev_name.span,
                    decl_span: prev_name.span,
                    type_description: None,
                    detail: Some("unfold previous step".to_string()),
                },
            );
            table.definitions.insert(
                curr_key.clone(),
                DefinitionInfo {
                    name: curr_name.name.clone(),
                    category: SymbolCategory::LocalVar,
                    name_span: curr_name.span,
                    decl_span: curr_name.span,
                    type_description: None,
                    detail: Some("unfold current step".to_string()),
                },
            );
            scopes.insert(prev_name.name.clone(), prev_key);
            scopes.insert(curr_name.name.clone(), curr_key);
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::VariantLiteral { index, variant } => {
            // Reference to the index name
            table.references.push(ReferenceInfo {
                span: index.span,
                target: index.value.to_string(),
            });
            // Reference to the qualified variant: Index::Variant
            table.references.push(ReferenceInfo {
                span: variant.span,
                target: format!("{}::{}", index.value, variant.value),
            });
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_expr_refs(scrutinee, table, scopes);
            for arm in arms {
                let variant_name = arm.pattern.variant_name.value.to_string();

                // If the pattern has a qualified index (e.g., Maneuver::Departure),
                // add a reference for the index name too.
                if let Some(qi) = &arm.pattern.qualified_index {
                    table.references.push(ReferenceInfo {
                        span: qi.span,
                        target: qi.value.to_string(),
                    });
                    // Reference the qualified variant: Index::Variant
                    table.references.push(ReferenceInfo {
                        span: arm.pattern.variant_name.span,
                        target: format!("{}::{}", qi.value, variant_name),
                    });
                } else {
                    // Try to resolve variant as Type::Variant (tagged union).
                    table.references.push(ReferenceInfo {
                        span: arm.pattern.variant_name.span,
                        target: variant_name.clone(),
                    });
                }

                scopes.push();
                for binding in &arm.pattern.bindings {
                    match binding {
                        PatternBinding::Bind { field, var } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: format!("field::{}", field.value),
                            });
                            let var_key = format!("match@{}::{}", arm.span.offset(), var.name);
                            table.definitions.insert(
                                var_key.clone(),
                                DefinitionInfo {
                                    name: var.name.clone(),
                                    category: SymbolCategory::LocalVar,
                                    name_span: var.span,
                                    decl_span: var.span,
                                    type_description: None,
                                    detail: Some(format!("bound from {variant_name}")),
                                },
                            );
                            scopes.insert(var.name.clone(), var_key);
                        }
                        PatternBinding::Wildcard { field, .. } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: format!("field::{}", field.value),
                            });
                        }
                    }
                }
                collect_expr_refs(&arm.body, table, scopes);
                scopes.pop();
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_) => {}
    }
}

/// Collect references from a type expression.
fn collect_type_expr_refs(type_expr: &graphcal_syntax::ast::TypeExpr, table: &mut SymbolTable) {
    match &type_expr.kind {
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
        TypeExprKind::DimExpr(dim_expr) => {
            collect_dim_expr_refs(dim_expr, table);
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_refs(base, table);
            for idx in indexes {
                table.references.push(ReferenceInfo {
                    span: idx.span,
                    target: idx.name.clone(),
                });
            }
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: name.name.clone(),
            });
            for arg in type_args {
                collect_type_expr_refs(arg, table);
            }
        }
    }
    // Collect references from domain constraint bound expressions (e.g., unit names in `100 kg`).
    for bound in &type_expr.constraints {
        collect_constraint_expr_refs(&bound.value, table);
    }
}

/// Collect references from a constraint bound expression (limited walk for unit names).
fn collect_constraint_expr_refs(expr: &graphcal_syntax::ast::Expr, table: &mut SymbolTable) {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_constraint_expr_refs(operand, table);
        }
        _ => {}
    }
}

/// Collect references from a dimension expression.
fn collect_dim_expr_refs(dim_expr: &DimExpr, table: &mut SymbolTable) {
    for item in &dim_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.term.span,
            target: item.term.name.name.clone(),
        });
    }
}

/// Collect references from a unit expression.
fn collect_unit_expr_refs(unit_expr: &UnitExpr, table: &mut SymbolTable) {
    for item in &unit_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.name.span,
            target: item.name.value.to_string(),
        });
    }
}

/// Format a domain bound expression as a human-readable string.
///
/// Handles the common cases: number literals, unit-annotated literals, and negated forms.
fn format_bound_expr(expr: &graphcal_syntax::ast::Expr) -> String {
    match &expr.kind {
        ExprKind::Number(v) => format_number(*v),
        ExprKind::Integer(v) => v.to_string(),
        ExprKind::UnitLiteral { value, unit } => {
            let num = format_number(*value);
            let unit_str = format_unit_expr_inline(unit);
            format!("{num} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_syntax::ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_bound_expr(operand))
        }
        // For complex expressions, fall back to "..."
        _ => "...".to_string(),
    }
}

/// Format a `UnitExpr` as a human-readable label (inline version for LSP).
fn format_unit_expr_inline(expr: &UnitExpr) -> String {
    use graphcal_syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.value.to_string();
        if let Some(pow) = item.power
            && pow != 1
        {
            part = format!("{part}^{pow}");
        }
        match item.op {
            MulDivOp::Mul => numerator.push(part),
            MulDivOp::Div => denominator.push(part),
        }
    }

    if denominator.is_empty() {
        numerator.join(" * ")
    } else if numerator.len() == 1 && denominator.len() == 1 {
        format!("{}/{}", numerator[0], denominator[0])
    } else {
        let num = numerator.join(" * ");
        let den = denominator.join(" * ");
        format!("{num} / ({den})")
    }
}

/// Format a constraint clause from domain bounds.
///
/// Returns a string like `(min: 100 kg, max: 2000 kg)` or an empty string if no constraints.
fn format_constraints(constraints: &[DomainBound]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints
        .iter()
        .map(|b| format!("{}: {}", b.name.name, format_bound_expr(&b.value)))
        .collect();
    format!("({})", parts.join(", "))
}

/// Format a resolved type expression with domain constraints.
///
/// For indexed types like `Velocity[Maneuver]`, inserts the constraint clause
/// between the base type and the index suffix: `Velocity(min: 0 m/s)[Maneuver]`.
fn format_type_with_constraints(
    resolved: &ResolvedTypeExpr,
    constraints: &[DomainBound],
    registry: &Registry,
) -> String {
    let constraint_str = format_constraints(constraints);
    if let ResolvedTypeExpr::Indexed { base, indexes } = resolved {
        let base_str = base.format(registry);
        let idx_strs: Vec<String> = indexes
            .iter()
            .map(|i| match i {
                ResolvedIndex::Concrete(name, _) => name.to_string(),
                ResolvedIndex::GenericParam(name, _) => name.to_string(),
            })
            .collect();
        format!("{base_str}{constraint_str}[{}]", idx_strs.join(", "))
    } else {
        let type_str = resolved.format(registry);
        format!("{type_str}{constraint_str}")
    }
}

/// Extract domain constraints from a `TypeExpr`, looking through `Indexed` wrappers.
fn extract_constraints(type_expr: &TypeExpr) -> &[DomainBound] {
    if !type_expr.constraints.is_empty() {
        return &type_expr.constraints;
    }
    // For indexed types, the constraints are on the base type
    if let TypeExprKind::Indexed { base, .. } = &type_expr.kind {
        return &base.constraints;
    }
    &[]
}

/// Enrich a symbol table with type information from a TIR.
#[expect(
    clippy::too_many_lines,
    reason = "linear match over all symbol categories"
)]
pub fn enrich_from_tir(table: &mut SymbolTable, tir: &TIR) {
    let registry = &tir.registry;

    // Build a map from declaration name to its AST TypeExpr constraints.
    let mut decl_constraints: HashMap<&str, &[DomainBound]> = HashMap::new();
    for (name, type_ann, _, _) in &tir.params {
        let constraints = extract_constraints(type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(name, constraints);
        }
    }
    for (name, type_ann, _, _) in &tir.nodes {
        let constraints = extract_constraints(type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(name, constraints);
        }
    }
    for (name, type_ann, _, _) in &tir.consts {
        let constraints = extract_constraints(type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(name, constraints);
        }
    }

    // Enrich param/node/const declarations with resolved types + constraints.
    for (name, resolved_type) in &tir.resolved_decl_types {
        if let Some(def) = table.definitions.get_mut(name) {
            let type_desc = decl_constraints.get(name.as_str()).map_or_else(
                || resolved_type.format(registry),
                |constraints| format_type_with_constraints(resolved_type, constraints, registry),
            );
            def.type_description = Some(type_desc);
        }
    }

    // Enrich function definitions with signatures.
    for (fn_name, sig) in &tir.resolved_fn_sigs {
        if let Some(def) = table.definitions.get_mut(fn_name.as_str()) {
            let params_str: Vec<String> = sig
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.resolved_type.format(registry)))
                .collect();

            let generics =
                if sig.generic_dim_params.is_empty() && sig.generic_index_params.is_empty() {
                    String::new()
                } else {
                    let all: Vec<String> = sig
                        .generic_dim_params
                        .iter()
                        .map(|p| format!("{p}: Dim"))
                        .chain(
                            sig.generic_index_params
                                .iter()
                                .map(|p| format!("{p}: Index")),
                        )
                        .collect();
                    format!("<{}>", all.join(", "))
                };

            let ret = sig.return_type.format(registry);
            def.type_description = Some(format!(
                "fn {}{generics}({}) -> {ret}",
                fn_name,
                params_str.join(", ")
            ));
        }
    }

    // Enrich dimension definitions.
    for (name, def) in &table.definitions.clone() {
        match def.category {
            SymbolCategory::Dimension => {
                if let Some(dim) = registry.dimensions.get_dimension(name)
                    && let Some(def_mut) = table.definitions.get_mut(name)
                {
                    def_mut.type_description = Some(format!(
                        "dimension {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            SymbolCategory::Unit => {
                if let Some(unit_info) = registry.units.get_unit(name)
                    && let Some(def_mut) = table.definitions.get_mut(name)
                {
                    def_mut.type_description = Some(format!(
                        "{}, scale = {}",
                        registry.dimensions.format_dimension(&unit_info.dimension),
                        unit_info.scale
                    ));
                }
            }
            SymbolCategory::Index => {
                if let Some(idx_def) = registry.indexes.get_index(name)
                    && let Some(def_mut) = table.definitions.get_mut(name)
                {
                    match &idx_def.kind {
                        IndexKind::Named { variants } => {
                            let vs: Vec<&str> = variants
                                .iter()
                                .map(graphcal_syntax::names::VariantName::as_str)
                                .collect();
                            def_mut.type_description = Some(format!("{{ {} }}", vs.join(", ")));
                        }
                        IndexKind::Range {
                            start, end, step, ..
                        } => {
                            def_mut.type_description =
                                Some(format!("range({start}, {end}, step: {step})"));
                        }
                    }
                }
            }
            SymbolCategory::StructType => {
                if let Some(type_def) = registry.types.get_type(name)
                    && let Some(def_mut) = table.definitions.get_mut(name)
                {
                    let variants_desc: Vec<String> = type_def
                        .variants
                        .iter()
                        .map(|v| {
                            if v.fields.is_empty() {
                                v.name.to_string()
                            } else {
                                let fields: Vec<String> =
                                    v.fields.iter().map(|f| f.name.to_string()).collect();
                                format!("{} {{ {} }}", v.name, fields.join(", "))
                            }
                        })
                        .collect();
                    def_mut.type_description = Some(variants_desc.join(" | "));
                }
            }
            _ => {}
        }
    }

    // Register field definitions from struct types so that `field::name`
    // references resolve to a definition with hover info.
    for type_def in registry.types.all_types() {
        for variant in &type_def.variants {
            for field in &variant.fields {
                let field_key = format!("field::{}", field.name);
                table
                    .definitions
                    .entry(field_key)
                    .or_insert_with(|| DefinitionInfo {
                        name: field.name.to_string(),
                        category: SymbolCategory::Field,
                        name_span: Span::new(0, 0),
                        decl_span: Span::new(0, 0),
                        type_description: None,
                        detail: Some(format!("field of {}", type_def.name)),
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn build_symbol_table_basic() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        assert!(table.definitions.contains_key("x"));
        assert!(table.definitions.contains_key("y"));
        assert_eq!(table.definitions["x"].category, SymbolCategory::Param);
        assert_eq!(table.definitions["y"].category, SymbolCategory::Node);

        // @x is a reference
        assert!(
            table.references.iter().any(|r| r.target == "x"),
            "expected @x reference"
        );
    }

    #[test]
    fn build_symbol_table_with_function() {
        let source = "fn double<D: Dim>(x: D) -> D = x + x;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        assert!(table.definitions.contains_key("double"));
        assert_eq!(
            table.definitions["double"].category,
            SymbolCategory::Function
        );
        assert!(table.definitions.contains_key("double::x"));
        assert_eq!(
            table.definitions["double::x"].category,
            SymbolCategory::LocalVar
        );
    }

    #[test]
    fn find_reference_at_offset() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
        let file = graphcal_syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let table = build_from_ast(&file);

        // Find the @x reference -- it should be near the end of the source
        let at_x_offset = source.find("@x").unwrap() + 1; // offset of 'x' in '@x'
        let reference = table.find_reference_at(at_x_offset);
        assert!(reference.is_some(), "expected to find reference at @x");
        assert_eq!(reference.unwrap().target, "x");
    }
}
