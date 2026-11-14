//! Canonical Graphcal spelling of an accepted structured browser value.
//!
//! The spelling is what `graphcal eval --param name=<spelling>` accepts for
//! the same value: a constructor as `Ctor(field: value, ...)`, an indexed
//! value of any rank as one `table[...]` literal (row labels, a column header,
//! and slice sections exactly as table syntax requires), and every leaf in its
//! own source spelling. A browser report prints it in its reproduction
//! command, so the command reproduces the evaluation the page shows.

use std::fmt::Write as _;

use super::binding_compile::{
    StructuredBindingError, StructuredBindingErrorKind, StructuredBindingPathSegment,
    StructuredValueExpr, structured_error,
};
use super::{ModelIndexKind, ModelIndexSchema, ModelValueSchema, PreparedProject};

/// The labels a table spells for the entries of one axis.
enum AxisLabels<'a> {
    /// Declared variants: bare in rows and headers, `Index#Variant` in slice
    /// labels.
    Named {
        index: String,
        variants: &'a [graphcal_compiler::syntax::index_name::IndexVariantName],
    },
    /// `Fin(N)` positions: omitted in rows and headers, `#N` in slice labels.
    Finite { cardinality: usize },
}

/// The state threaded through one table body.
struct TableBody<'a, 'b> {
    /// Schema of every cell.
    leaf_schema: &'a ModelValueSchema,
    /// Slice labels of the enclosing outer keys.
    slice: Vec<String>,
    path: &'b mut Vec<StructuredBindingPathSegment>,
    output: &'b mut String,
}

fn check_entry_count(
    entries: &[StructuredValueExpr],
    axis: &AxisLabels<'_>,
    path: &[StructuredBindingPathSegment],
) -> Result<(), StructuredBindingError> {
    if entries.len() == axis.len() {
        Ok(())
    } else {
        Err(structured_error(
            path,
            StructuredBindingErrorKind::EntryCount {
                expected: axis.len(),
                received: entries.len(),
            },
        ))
    }
}

impl AxisLabels<'_> {
    const fn len(&self) -> usize {
        match self {
            Self::Named { variants, .. } => variants.len(),
            Self::Finite { cardinality } => *cardinality,
        }
    }

    /// The bare row or column label of entry `position`, if the axis has one.
    fn row_label(&self, position: usize) -> Option<&str> {
        match self {
            Self::Named { variants, .. } => variants
                .get(position)
                .map(graphcal_compiler::syntax::names::NameDef::as_str),
            Self::Finite { .. } => None,
        }
    }

    fn slice_label(&self, position: usize) -> String {
        match self {
            Self::Named { index, variants } => variants
                .get(position)
                .map_or_else(String::new, |variant| format!("{index}#{variant}")),
            Self::Finite { .. } => format!("#{position}"),
        }
    }
}

impl PreparedProject {
    /// The canonical source spelling of `value`, already accepted against
    /// `schema` by the structured binding compiler.
    pub(super) fn structured_source(
        &self,
        value: &StructuredValueExpr,
        schema: &ModelValueSchema,
    ) -> Result<String, StructuredBindingError> {
        let mut output = String::new();
        self.write_structured_source(value, schema, &mut Vec::new(), &mut output)?;
        Ok(output)
    }

    fn write_structured_source(
        &self,
        value: &StructuredValueExpr,
        schema: &ModelValueSchema,
        path: &mut Vec<StructuredBindingPathSegment>,
        output: &mut String,
    ) -> Result<(), StructuredBindingError> {
        match (value, schema) {
            (StructuredValueExpr::Literal(_), ModelValueSchema::Algebraic(_)) => Err(
                structured_error(path, StructuredBindingErrorKind::ExpectedAlgebraic),
            ),
            (StructuredValueExpr::Literal(_), ModelValueSchema::Indexed { .. }) => Err(
                structured_error(path, StructuredBindingErrorKind::ExpectedIndexed),
            ),
            (StructuredValueExpr::Literal(literal), _) => {
                output.push_str(literal.spelling());
                Ok(())
            }
            (
                StructuredValueExpr::Algebraic {
                    definition,
                    constructor,
                    fields,
                },
                ModelValueSchema::Algebraic(expected),
            ) => {
                let graph = self.model_schema_graph();
                if graph.definition_index(expected) != Some(*definition) {
                    return Err(structured_error(
                        path,
                        StructuredBindingErrorKind::ForeignConstructor,
                    ));
                }
                let constructor = graph
                    .definition_at(*definition)
                    .and_then(|definition| definition.constructors().get(*constructor))
                    .ok_or_else(|| {
                        structured_error(path, StructuredBindingErrorKind::UnknownConstructor)
                    })?;
                if fields.len() != constructor.fields().len() {
                    return Err(structured_error(
                        path,
                        StructuredBindingErrorKind::FieldCount {
                            constructor: constructor.name().clone(),
                            expected: constructor.fields().len(),
                            received: fields.len(),
                        },
                    ));
                }
                // A bare constructor is resolved against the parameter's
                // declared type, exactly as the structured value was.
                let _ = write!(output, "{}(", constructor.name());
                for (index, (field, field_schema)) in
                    fields.iter().zip(constructor.fields()).enumerate()
                {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    let _ = write!(output, "{}: ", field_schema.name());
                    path.push(StructuredBindingPathSegment::Field(index));
                    let written =
                        self.write_structured_source(field, field_schema.value(), path, output);
                    path.pop();
                    written?;
                }
                output.push(')');
                Ok(())
            }
            (
                StructuredValueExpr::Indexed { entries },
                ModelValueSchema::Indexed { element, axis },
            ) => self.write_table(entries, element, axis, path, output),
            (StructuredValueExpr::Algebraic { .. }, _) => Err(structured_error(
                path,
                StructuredBindingErrorKind::UnexpectedAlgebraic,
            )),
            (StructuredValueExpr::Indexed { .. }, _) => Err(structured_error(
                path,
                StructuredBindingErrorKind::UnexpectedIndexed,
            )),
        }
    }

    /// One `table[...]` literal over every nested indexed level.
    fn write_table(
        &self,
        entries: &[StructuredValueExpr],
        element: &ModelValueSchema,
        axis: &ModelIndexSchema,
        path: &mut Vec<StructuredBindingPathSegment>,
        output: &mut String,
    ) -> Result<(), StructuredBindingError> {
        let mut table_axes = vec![axis];
        let mut leaf_schema = element;
        while let ModelValueSchema::Indexed { element, axis } = leaf_schema {
            table_axes.push(axis);
            leaf_schema = element;
        }
        let labels = table_axes
            .iter()
            .map(|axis| self.axis_labels(axis, path))
            .collect::<Result<Vec<_>, _>>()?;
        let names = labels
            .iter()
            .map(|labels| match labels {
                AxisLabels::Named { index, .. } => index.clone(),
                AxisLabels::Finite { cardinality } => format!("Fin({cardinality})"),
            })
            .collect::<Vec<_>>();
        let _ = write!(output, "table[{}] {{", names.join(", "));
        let mut body = TableBody {
            leaf_schema,
            slice: Vec::new(),
            path,
            output,
        };
        self.write_table_body(entries, &labels, &mut body)?;
        output.push_str(" }");
        Ok(())
    }

    /// The body of a table over `labels`: rows for one axis, a header and
    /// rows for two, and one slice section per outer key for more.
    fn write_table_body(
        &self,
        entries: &[StructuredValueExpr],
        labels: &[AxisLabels<'_>],
        body: &mut TableBody<'_, '_>,
    ) -> Result<(), StructuredBindingError> {
        match labels {
            [] => Ok(()),
            [rows] => {
                let cells = self.table_cells(entries, rows, body)?;
                for (position, cell) in cells.iter().enumerate() {
                    body.output.push(' ');
                    if let Some(label) = rows.row_label(position) {
                        let _ = write!(body.output, "{label}: ");
                    }
                    let _ = write!(body.output, "{cell};");
                }
                Ok(())
            }
            [rows, columns] => {
                check_entry_count(entries, rows, body.path)?;
                if !body.slice.is_empty() {
                    let _ = write!(body.output, " [{}]", body.slice.join(", "));
                }
                if let AxisLabels::Named { variants, .. } = columns {
                    let header = variants
                        .iter()
                        .map(graphcal_compiler::syntax::names::NameDef::as_str)
                        .collect::<Vec<_>>();
                    let _ = write!(body.output, " : {};", header.join(", "));
                }
                for (position, entry) in entries.iter().enumerate() {
                    body.path
                        .push(StructuredBindingPathSegment::Entry(position));
                    let cells = match entry {
                        StructuredValueExpr::Indexed { entries } => {
                            self.table_cells(entries, columns, body)
                        }
                        _ => Err(structured_error(
                            body.path,
                            StructuredBindingErrorKind::ExpectedIndexed,
                        )),
                    };
                    body.path.pop();
                    body.output.push(' ');
                    if let Some(label) = rows.row_label(position) {
                        let _ = write!(body.output, "{label}: ");
                    }
                    let _ = write!(body.output, "{};", cells?.join(", "));
                }
                Ok(())
            }
            [outer, inner @ ..] => {
                check_entry_count(entries, outer, body.path)?;
                for (position, entry) in entries.iter().enumerate() {
                    body.path
                        .push(StructuredBindingPathSegment::Entry(position));
                    body.slice.push(outer.slice_label(position));
                    let written = match entry {
                        StructuredValueExpr::Indexed { entries } => {
                            self.write_table_body(entries, inner, body)
                        }
                        _ => Err(structured_error(
                            body.path,
                            StructuredBindingErrorKind::ExpectedIndexed,
                        )),
                    };
                    body.slice.pop();
                    body.path.pop();
                    written?;
                }
                Ok(())
            }
        }
    }

    /// The source spelling of every leaf on the innermost axis.
    fn table_cells(
        &self,
        entries: &[StructuredValueExpr],
        axis: &AxisLabels<'_>,
        body: &mut TableBody<'_, '_>,
    ) -> Result<Vec<String>, StructuredBindingError> {
        check_entry_count(entries, axis, body.path)?;
        entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                body.path
                    .push(StructuredBindingPathSegment::Entry(position));
                let mut cell = String::new();
                let written =
                    self.write_structured_source(entry, body.leaf_schema, body.path, &mut cell);
                body.path.pop();
                written.map(|()| cell)
            })
            .collect()
    }

    fn axis_labels<'a>(
        &self,
        axis: &'a ModelIndexSchema,
        path: &[StructuredBindingPathSegment],
    ) -> Result<AxisLabels<'a>, StructuredBindingError> {
        match axis.kind() {
            ModelIndexKind::Finite { index } => Ok(AxisLabels::Finite {
                cardinality: index.cardinality().get(),
            }),
            ModelIndexKind::Named { variants } => {
                let index = axis
                    .identity()
                    .declared_resolved()
                    .and_then(|resolved| self.source_index_path(resolved))
                    .ok_or_else(|| {
                        structured_error(path, StructuredBindingErrorKind::AxisNotVisible)
                    })?;
                Ok(AxisLabels::Named {
                    index: index.to_string(),
                    variants: variants.as_slice(),
                })
            }
            ModelIndexKind::Coordinate { .. } => Err(structured_error(
                path,
                StructuredBindingErrorKind::CoordinateEntries,
            )),
        }
    }
}
