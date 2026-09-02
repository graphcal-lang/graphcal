//! Prepared-parameter binding compilation and row construction.

use super::{
    AstExprKind, BuiltinFnName, CheckedEntryInterface, CompileError, ConstRef, DeclName,
    DiagnosticAnchor, EvalContext, Expr, ExprLoweringContext, FunctionRef, GenericScope,
    GraphcalError, HirExprKind, HirLocalValueMap, ModelSchemaGraph, ModelSchemaGraphBuilder,
    ModelValueSchema, ParameterBindingBuilder, ParameterPort, ParameterPosition, ParameterValue,
    PreludeTypeScope, PreparedProject, RuntimeParameterBinding, RuntimeParameterBindings,
    RuntimeValue, RuntimeValueMap, Span, UnaryOp, builtin_functions, eval_hir_expr,
    parameter_domain,
};

impl PreparedProject {
    /// Begin constructing one plan-scoped parameter row.
    #[must_use]
    pub fn binding_builder(&self) -> ParameterBindingBuilder<'_> {
        ParameterBindingBuilder {
            project: self,
            slots: vec![None; self.parameter_ports.len()],
        }
    }

    /// Compile and validate one closed recursive value for a plan position.
    pub fn compile_parameter_value(
        &self,
        position: ParameterPosition,
        expr: &Expr,
    ) -> Result<ParameterValue, CompileError> {
        let port = self.port_at(position)?;
        let normalized =
            normalize_binding_literal(expr.clone(), &port.value_schema, &self.schema_graph)
                .map_err(|message| {
                    CompileError::Eval(GraphcalError::EvalError {
                        message: format!("invalid binding for `{}`: {message}", port.name),
                        src: self.source.clone(),
                        span: expr.span.into(),
                    })
                })?;
        let hir = self.lower_closed_binding(port, &normalized)?;
        let value = self.evaluate_closed_binding(&hir)?;
        let presentation = graphcal_compiler::tir::dim_check::checked_expression_presentation(
            &self.tir,
            port.runtime_key.as_resolved(),
            &hir,
            &self.source,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .map_err(CompileError::Eval)?;
        Ok(ParameterValue {
            plan_id: self.plan_id,
            position,
            binding: RuntimeParameterBinding {
                value,
                presentation,
            },
        })
    }

    /// Resolve an entry parameter name and compile one closed recursive value.
    pub fn compile_named_parameter_value(
        &self,
        name: &DeclName,
        expr: &Expr,
    ) -> Result<ParameterValue, CompileError> {
        let index = self.parameter_index(name)?;
        self.compile_parameter_value(self.parameter_ports[index].position, expr)
    }
}

impl ParameterBindingBuilder<'_> {
    /// Validate required slots and finish this row.
    pub fn finish(self) -> Result<ParameterBindingRow, CompileError> {
        if let Some(port) = self
            .project
            .parameter_ports
            .iter()
            .find(|port| !port.has_default && self.slots[port.position.index].is_none())
        {
            return Err(CompileError::Eval(
                GraphcalError::RequiredParamNotProvided {
                    name: port.name.to_string(),
                    src: self.project.source.clone(),
                    span: port.span.into(),
                },
            ));
        }
        let bindings = self
            .project
            .parameter_ports
            .iter()
            .zip(self.slots)
            .filter_map(|(port, binding)| {
                binding.map(|binding| (port.runtime_key.clone(), binding))
            })
            .collect();
        Ok(ParameterBindingRow {
            plan_id: self.project.plan_id,
            bindings,
        })
    }

    pub(super) fn insert(
        &mut self,
        position: ParameterPosition,
        binding: RuntimeParameterBinding,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if let Some(constraint) = self.project.plan.domain_constraints.get(&port.runtime_key)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(&binding.value, constraint)
        {
            return Err(self.project.binding_value_error(port, &violation.message));
        }
        let slot = &mut self.slots[position.index];
        if slot.is_some() {
            return Err(self.project.binding_value_error(
                port,
                &format!("parameter `{}` is bound more than once", port.name),
            ));
        }
        *slot = Some(binding);
        Ok(())
    }
}

/// One validated row of optional/defaulted and supplied parameter values.
#[derive(Debug, Clone)]
pub struct ParameterBindingRow {
    pub(super) plan_id: u64,
    pub(super) bindings: RuntimeParameterBindings,
}

impl PreparedProject {
    fn parameter_index(&self, name: &DeclName) -> Result<usize, CompileError> {
        self.parameter_lookup.get(name).copied().ok_or_else(|| {
            let actual_kind = self
                .tir
                .root()
                .source_order()
                .iter()
                .find_map(|(candidate, kind)| (candidate.member() == name).then_some(*kind));
            actual_kind.map_or_else(
                || CompileError::Eval(GraphcalError::OverrideUnknownParam { name: name.clone() }),
                |actual_kind| {
                    CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: name.clone(),
                        actual_kind,
                    })
                },
            )
        })
    }

    pub(super) fn port_at(
        &self,
        position: ParameterPosition,
    ) -> Result<&ParameterPort, CompileError> {
        if position.plan_id != self.plan_id {
            return Err(CompileError::Eval(GraphcalError::internal_error(
                "parameter position belongs to another prepared project",
                &self.source,
                DiagnosticAnchor::Builtin,
            )));
        }
        self.parameter_ports.get(position.index).ok_or_else(|| {
            CompileError::Eval(GraphcalError::internal_error(
                format!("parameter position {} is out of bounds", position.index),
                &self.source,
                DiagnosticAnchor::Builtin,
            ))
        })
    }

    fn lower_closed_binding(
        &self,
        port: &ParameterPort,
        expr: &Expr,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let hir =
            self.lower_closed_binding_expr(expr, &port.value_schema, self.tir.root_dag_id())?;
        validate_closed_hir(&hir).map_err(|message| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "binding for `{}` is not a closed value: {message}",
                    port.name
                ),
                src: self.source.clone(),
                span: hir.span.into(),
            })
        })?;
        graphcal_compiler::tir::dim_check::check_external_value_expr_type(
            &self.tir,
            &self.declared_types,
            &hir,
            &port.declared_type,
            &self.source,
        )?;
        Ok(hir)
    }

    /// Lower a closed boundary value against its canonical recursive schema.
    ///
    /// Constructor and field-value syntax is interpreted in the module that
    /// defines the expected algebraic type. This is deliberately different
    /// from lowering a source expression in the entry module: decoding a value
    /// for an imported type must not require the entry to import that type's
    /// constructors or the units used by its field contracts. Nested values
    /// switch owners again at each canonical constructor boundary.
    fn lower_closed_binding_expr(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        match (&expr.kind, expected) {
            (AstExprKind::ConstructorCall { callee, .. }, ModelValueSchema::Algebraic(_))
                if callee.as_bare().is_some() =>
            {
                self.lower_canonical_constructor_binding(expr, expected, owner)
            }
            (
                AstExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(
                    path,
                )),
                ModelValueSchema::Algebraic(type_id),
            ) if path.as_bare().is_some()
                && self
                    .schema_graph
                    .definition(type_id)
                    .is_some_and(|definition| {
                        definition.constructors().iter().any(|constructor| {
                            constructor.name().atom() == &path.leaf().name
                                && constructor.fields().is_empty()
                        })
                    }) =>
            {
                let constructor_expr = Expr::new(
                    AstExprKind::ConstructorCall {
                        callee: path.clone(),
                        generic_args: Vec::new(),
                        fields: Vec::new(),
                    },
                    expr.span,
                );
                self.lower_closed_binding_expr(&constructor_expr, expected, owner)
            }
            (AstExprKind::MapLiteral { .. }, ModelValueSchema::Indexed { .. }) => {
                self.lower_closed_map_binding(expr, expected, owner)
            }
            _ => self.lower_binding_expr_in_owner(expr, owner),
        }
    }

    fn lower_canonical_constructor_binding(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let AstExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } = &expr.kind
        else {
            return Err(self.binding_internal_error(
                "canonical external constructor has a non-constructor syntax node",
                expr.span,
            ));
        };
        let ModelValueSchema::Algebraic(type_id) = expected else {
            return Err(self.binding_internal_error(
                "canonical external constructor has a non-algebraic schema",
                expr.span,
            ));
        };
        let Some(definition) = self.schema_graph.definition(type_id) else {
            return Err(self.binding_internal_error(
                "canonical external constructor references a missing algebraic definition",
                expr.span,
            ));
        };
        let Some(constructor) = definition
            .constructors()
            .iter()
            .find(|constructor| constructor.name().atom() == &callee.leaf().name)
        else {
            return self.lower_binding_expr_in_owner(expr, owner);
        };
        let constructor_owner = type_id.identity().resolved().owner();
        let signature_expr = Expr::new(
            AstExprKind::ConstructorCall {
                callee: callee.clone(),
                generic_args: generic_args.clone(),
                fields: Vec::new(),
            },
            expr.span,
        );
        let signature = self.lower_binding_expr_in_owner(&signature_expr, constructor_owner)?;
        let HirExprKind::ConstructorCall {
            callee,
            generic_args,
            ..
        } = signature.kind
        else {
            return Err(self.binding_internal_error(
                "canonical external constructor did not lower to a constructor call",
                expr.span,
            ));
        };
        let fields = fields
            .iter()
            .map(|field| {
                let value = constructor
                    .fields()
                    .iter()
                    .find(|schema| schema.name() == &field.name.value)
                    .map_or_else(
                        || self.lower_binding_expr_in_owner(&field.value, constructor_owner),
                        |schema| {
                            self.lower_closed_binding_expr(
                                &field.value,
                                schema.value(),
                                constructor_owner,
                            )
                        },
                    )?;
                Ok(graphcal_compiler::hir::expr::FieldInit {
                    name: field.name.clone(),
                    value,
                })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::Expr::new(
            HirExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            },
            expr.span,
        ))
    }

    fn lower_closed_map_binding(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let AstExprKind::MapLiteral { entries } = &expr.kind else {
            return Err(self.binding_internal_error(
                "external map binding has a non-map syntax node",
                expr.span,
            ));
        };
        let entries = entries
            .iter()
            .map(|entry| {
                // Resolve source-shaped keys normally, but lower each value
                // against its recursive schema so nested constructor owners survive.
                let mut key_entry = entry.clone();
                key_entry.value = Expr::new(AstExprKind::Bool(true), entry.value.span);
                let key_expr = Expr::new(
                    AstExprKind::MapLiteral {
                        entries: vec![key_entry],
                    },
                    expr.span,
                );
                let lowered_key = self.lower_binding_expr_in_owner(&key_expr, owner)?;
                let HirExprKind::MapLiteral {
                    entries: lowered_entries,
                } = lowered_key.kind
                else {
                    return Err(self.binding_internal_error(
                        "external map key did not lower to a map literal",
                        expr.span,
                    ));
                };
                let Some(lowered_entry) = lowered_entries.into_iter().next() else {
                    return Err(self.binding_internal_error(
                        "external map key lowering produced no entry",
                        expr.span,
                    ));
                };
                let entry_schema =
                    map_entry_value_schema(expected, entry.keys.len()).map_err(|message| {
                        CompileError::Eval(GraphcalError::EvalError {
                            message: format!("invalid external map binding: {message}"),
                            src: self.source.clone(),
                            span: entry.value.span.into(),
                        })
                    })?;
                let value = self.lower_closed_binding_expr(&entry.value, entry_schema, owner)?;
                Ok(graphcal_compiler::hir::expr::MapEntry {
                    keys: lowered_entry.keys,
                    value,
                })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::Expr::new(
            HirExprKind::MapLiteral { entries },
            expr.span,
        ))
    }

    fn lower_binding_expr_in_owner(
        &self,
        expr: &Expr,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let scope = GenericScope::new();
        let prelude = PreludeTypeScope::graphcal();
        let context = ExprLoweringContext::new(
            owner,
            &self.module_resolver,
            &scope,
            &self.tir.registry().time_zones,
        )
        .with_prelude(&prelude);
        let (hir, diagnostics) = graphcal_compiler::hir::lower_expr_tolerant(expr, context);
        if let Some(error) = diagnostics.first() {
            return Err(CompileError::Eval(
                graphcal_compiler::hir::expr_lower_error_to_graphcal(error, &self.source),
            ));
        }
        Ok(hir)
    }

    fn binding_internal_error(&self, message: &str, span: Span) -> CompileError {
        CompileError::Eval(GraphcalError::InternalError {
            message: message.to_string(),
            src: self.source.clone(),
            span: span.into(),
        })
    }

    fn evaluate_closed_binding(
        &self,
        expr: &graphcal_compiler::hir::Expr,
    ) -> Result<RuntimeValue, CompileError> {
        let values = RuntimeValueMap::new();
        let locals = HirLocalValueMap::root();
        let builtin_fns = builtin_functions();
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let context = EvalContext {
            cancellation,
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: self.tir.registry(),
            src: &self.source,
            tir: &self.tir,
            current_dag: self.tir.root(),
            current_decl: None,
            root_values: Some(&values),
            root_presentation_instances: None,
            checked_execution_facts: Some(&self.plan.checked_execution_facts),
            presentation_calls: None,
            struct_field_constraints: Some(&self.plan.struct_field_constraints),
            generic_nat_bindings: None,
            host_fns: Some(&self.host_fns),
        };
        eval_hir_expr(expr, &values, &locals, &context).map_err(CompileError::from)
    }

    pub(super) fn binding_kind_error(&self, port: &ParameterPort, actual: &str) -> CompileError {
        self.binding_value_error(
            port,
            &format!(
                "cannot bind {actual} to `{}` of type `{}`",
                port.name,
                port.declared_type.format(&self.tir.registry().dimensions)
            ),
        )
    }

    pub(super) fn binding_value_error(&self, port: &ParameterPort, message: &str) -> CompileError {
        CompileError::Eval(GraphcalError::EvalError {
            message: message.to_string(),
            src: self.source.clone(),
            span: port.span.into(),
        })
    }
}

pub(super) fn build_parameter_ports(
    plan_id: u64,
    entry_interface: &CheckedEntryInterface,
    plan: &crate::exec_plan::ExecPlan,
    schemas: &mut ModelSchemaGraphBuilder<'_>,
) -> Result<Vec<ParameterPort>, CompileError> {
    entry_interface
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let declared_type = parameter.declared_type().clone();
            let value_schema = schemas
                .value_schema(&declared_type)
                .map_err(CompileError::Eval)?;
            let runtime_key = parameter.runtime_key().clone();
            let domain = plan
                .domain_constraints
                .get(&runtime_key)
                .map(parameter_domain);
            Ok(ParameterPort {
                name: parameter.name().clone(),
                position: ParameterPosition { plan_id, index },
                declared_type,
                value_schema,
                domain,
                has_default: parameter.has_default(),
                runtime_key,
                span: parameter.span(),
            })
        })
        .collect()
}

fn normalize_binding_literal(
    mut expr: Expr,
    expected: &ModelValueSchema,
    schemas: &ModelSchemaGraph,
) -> Result<Expr, &'static str> {
    normalize_binding_literal_in_place(&mut expr, expected, schemas)?;
    Ok(expr)
}

fn map_entry_value_schema(
    expected: &ModelValueSchema,
    key_count: usize,
) -> Result<&ModelValueSchema, &'static str> {
    (0..key_count).try_fold(expected, |schema, _| match schema {
        ModelValueSchema::Indexed { element, .. } => Ok(element.as_ref()),
        _ => Err("map entry has more keys than the declared value has indexed axes"),
    })
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    reason = "the boundary checks exact f64/i64 range and round-trip identity before conversion"
)]
fn normalize_binding_literal_in_place(
    expr: &mut Expr,
    expected: &ModelValueSchema,
    schemas: &ModelSchemaGraph,
) -> Result<(), &'static str> {
    match (&mut expr.kind, expected) {
        (AstExprKind::Integer(value), ModelValueSchema::Quantity(quantity))
            if quantity.dimension().is_dimensionless() =>
        {
            const MAX_EXACT_INTEGER: u64 = 1_u64 << f64::MANTISSA_DIGITS;
            if value.unsigned_abs() > MAX_EXACT_INTEGER {
                return Err("integer is not exactly representable as a real quantity");
            }
            expr.kind = AstExprKind::Number(*value as f64);
        }
        (AstExprKind::Number(value), ModelValueSchema::Int) => {
            const I64_EXCLUSIVE_UPPER: f64 = 9_223_372_036_854_775_808.0;
            if !value.is_finite()
                || value.fract() != 0.0
                || *value < -I64_EXCLUSIVE_UPPER
                || *value >= I64_EXCLUSIVE_UPPER
            {
                return Err("number is not exactly representable as Int");
            }
            let integer = *value as i64;
            if integer as f64 != *value {
                return Err("number is not exactly representable as Int");
            }
            expr.kind = AstExprKind::Integer(integer);
        }
        (AstExprKind::UnaryOp { operand, .. }, _) => {
            normalize_binding_literal_in_place(operand, expected, schemas)?;
        }
        (AstExprKind::FnCall { args, .. }, ModelValueSchema::Complex(quantity)) => {
            let component = ModelValueSchema::Quantity(quantity.clone());
            for argument in args {
                normalize_binding_literal_in_place(argument, &component, schemas)?;
            }
        }
        (
            AstExprKind::ConstructorCall { callee, fields, .. },
            ModelValueSchema::Algebraic(type_id),
        ) => {
            if let Some(constructor) = schemas.definition(type_id).and_then(|definition| {
                definition
                    .constructors()
                    .iter()
                    .find(|constructor| constructor.name().atom() == &callee.leaf().name)
            }) {
                for field in fields {
                    if let Some(field_schema) = constructor
                        .fields()
                        .iter()
                        .find(|schema| schema.name() == &field.name.value)
                    {
                        normalize_binding_literal_in_place(
                            &mut field.value,
                            field_schema.value(),
                            schemas,
                        )?;
                    }
                }
            }
        }
        (AstExprKind::MapLiteral { entries }, ModelValueSchema::Indexed { .. }) => {
            for entry in entries {
                let entry_schema = map_entry_value_schema(expected, entry.keys.len())?;
                normalize_binding_literal_in_place(&mut entry.value, entry_schema, schemas)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_closed_hir(expr: &graphcal_compiler::hir::Expr) -> Result<(), &'static str> {
    match &expr.kind {
        HirExprKind::Number(value) if value.is_finite() => Ok(()),
        HirExprKind::Number(_) => Err("numeric values must be finite"),
        HirExprKind::Integer(_)
        | HirExprKind::Bool(_)
        | HirExprKind::OffsetDateTimeLiteral(_)
        | HirExprKind::CivilDateTimeLiteral(_)
        | HirExprKind::ZonedDateTimeLiteral(_)
        | HirExprKind::IanaTimeZoneLiteral(_)
        | HirExprKind::VariantLiteral(_) => Ok(()),
        HirExprKind::QuantityLiteral { value, .. } if value.is_finite() => Ok(()),
        HirExprKind::QuantityLiteral { .. } => Err("quantity values must be finite"),
        HirExprKind::ConstRef(reference) if matches!(reference.value, ConstRef::Constructor(_)) => {
            Ok(())
        }
        HirExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => validate_closed_hir(operand),
        HirExprKind::FnCall { callee, args }
            if matches!(
                callee.value,
                FunctionRef::Builtin(BuiltinFnName::Complex | BuiltinFnName::Datetime)
                    | FunctionRef::Epoch { .. }
            ) =>
        {
            args.iter().try_for_each(validate_closed_hir)
        }
        HirExprKind::DisplayTimezone { expr, .. } => validate_closed_hir(expr),
        HirExprKind::ConstructorCall { fields, .. } => fields
            .iter()
            .try_for_each(|field| validate_closed_hir(&field.value)),
        HirExprKind::MapLiteral { entries } => entries
            .iter()
            .try_for_each(|entry| validate_closed_hir(&entry.value)),
        HirExprKind::KeyForm { arg, .. } => validate_closed_hir(arg),
        HirExprKind::Error { .. } => Err("an unresolved expression is not allowed"),
        HirExprKind::StringLiteral(_)
        | HirExprKind::TypeSystemRef(_)
        | HirExprKind::GraphRef(_)
        | HirExprKind::ConstRef(_)
        | HirExprKind::LocalRef(_)
        | HirExprKind::BinOp { .. }
        | HirExprKind::UnaryOp { .. }
        | HirExprKind::FnCall { .. }
        | HirExprKind::If { .. }
        | HirExprKind::Convert { .. }
        | HirExprKind::FieldAccess { .. }
        | HirExprKind::ForComp { .. }
        | HirExprKind::IndexAccess { .. }
        | HirExprKind::Scan { .. }
        | HirExprKind::Unfold { .. }
        | HirExprKind::Match { .. }
        | HirExprKind::DagCall { .. } => {
            Err("references, computations, and control-flow expressions are not allowed")
        }
    }
}
