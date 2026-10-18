//! Detection of nominal types that refer to themselves.
//!
//! A nominal type may not reach itself through its constructor fields, the
//! generic arguments written in them, or its generic parameter defaults. With
//! no such cycle, the size of every value follows from its type, so every
//! value has a finite schema at each transport boundary.

use std::collections::HashMap;

use crate::resolved_name::ResolvedStructTypeName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName};

use super::nominal::NominalTypeDef;
use super::types::{DeclType, GenericArg, ValueType, ValueTypeKind};

/// Where one definition names the next type of a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeReferenceSite {
    /// The type of a constructor payload field.
    Field {
        constructor: ConstructorName,
        field: FieldName,
    },
    /// The default of a `Type`-sorted generic parameter.
    GenericDefault { param: GenericParamName },
}

/// One edge of a type cycle: `from` names `to` at `site`.
#[derive(Debug, Clone)]
pub struct TypeCycleStep<'a> {
    from: &'a NominalTypeDef,
    site: TypeReferenceSite,
    to: Spanned<ResolvedStructTypeName>,
}

impl<'a> TypeCycleStep<'a> {
    /// The definition that holds the reference.
    #[must_use]
    pub const fn from(&self) -> &'a NominalTypeDef {
        self.from
    }

    /// The field or generic default that holds the reference.
    #[must_use]
    pub const fn site(&self) -> &TypeReferenceSite {
        &self.site
    }

    /// The referenced type.
    #[must_use]
    pub const fn to(&self) -> &ResolvedStructTypeName {
        &self.to.value
    }

    /// Span of the reference in the source of [`Self::from`].
    #[must_use]
    pub const fn span(&self) -> Span {
        self.to.span
    }
}

/// A closed chain of type references: each step starts where the previous
/// one ends, and the last step ends at the first step's start.
#[derive(Debug, Clone)]
pub struct TypeCycle<'a> {
    steps: NonEmpty<TypeCycleStep<'a>>,
}

impl<'a> TypeCycle<'a> {
    /// The definition the cycle starts and ends at.
    #[must_use]
    pub fn start(&self) -> &'a NominalTypeDef {
        self.steps.first().from()
    }

    /// The references in order, starting at [`Self::start`].
    #[must_use]
    pub fn steps(&self) -> &[TypeCycleStep<'a>] {
        self.steps.as_slice()
    }
}

/// The first type cycle that passes through one of `roots`, if any.
///
/// `lookup` returns the definition of every type reachable from `roots`; a
/// type it does not know cannot close a cycle. Roots are searched in the given
/// order, and each definition's references in declaration order, so the
/// reported cycle is deterministic. A cycle reached from a root but not
/// passing through any root is also reported.
#[must_use]
pub fn find_type_cycle<'a>(
    roots: impl IntoIterator<Item = &'a NominalTypeDef>,
    lookup: impl Fn(&ResolvedStructTypeName) -> Option<&'a NominalTypeDef>,
) -> Option<TypeCycle<'a>> {
    let mut search = Search {
        lookup,
        edges: HashMap::new(),
        state: HashMap::new(),
    };
    roots.into_iter().find_map(|root| search.cycle_from(root))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    Active,
    Done,
}

/// A depth-first search that remembers visited definitions across roots.
struct Search<'a, L> {
    lookup: L,
    edges: HashMap<&'a ResolvedStructTypeName, Vec<TypeCycleStep<'a>>>,
    state: HashMap<&'a ResolvedStructTypeName, Visit>,
}

impl<'a, L: Fn(&ResolvedStructTypeName) -> Option<&'a NominalTypeDef>> Search<'a, L> {
    /// The outgoing references of `definition`, computed once.
    fn edge(
        &mut self,
        definition: &'a NominalTypeDef,
        position: usize,
    ) -> Option<TypeCycleStep<'a>> {
        self.edges
            .entry(definition.identity())
            .or_insert_with(|| outgoing_edges(definition))
            .get(position)
            .cloned()
    }

    fn cycle_from(&mut self, root: &'a NominalTypeDef) -> Option<TypeCycle<'a>> {
        if self.state.contains_key(root.identity()) {
            return None;
        }
        self.state.insert(root.identity(), Visit::Active);
        // Each frame is a definition on the current path and the position of
        // its next edge; `taken[i]` is the edge from `path[i]` to `path[i + 1]`.
        let mut path: Vec<(&'a NominalTypeDef, usize)> = vec![(root, 0)];
        let mut taken: Vec<TypeCycleStep<'a>> = Vec::new();
        while let Some(&(node, next)) = path.last() {
            let Some(step) = self.edge(node, next) else {
                self.state.insert(node.identity(), Visit::Done);
                path.pop();
                taken.pop();
                continue;
            };
            if let Some(frame) = path.last_mut() {
                frame.1 += 1;
            }
            let Some(target) = (self.lookup)(step.to()) else {
                continue;
            };
            match self.state.get(target.identity()) {
                Some(Visit::Done) => {}
                Some(Visit::Active) => {
                    let start = path
                        .iter()
                        .position(|(on_path, _)| on_path.identity() == target.identity())?;
                    let mut steps = taken.drain(start..).chain(std::iter::once(step));
                    let first = steps.next()?;
                    return Some(TypeCycle {
                        steps: NonEmpty::new(first, steps.collect()),
                    });
                }
                None => {
                    self.state.insert(target.identity(), Visit::Active);
                    path.push((target, 0));
                    taken.push(step);
                }
            }
        }
        None
    }
}

/// Every type reference of `definition`, in declaration order.
fn outgoing_edges(definition: &NominalTypeDef) -> Vec<TypeCycleStep<'_>> {
    let from = definition;
    let defaults = definition.generic_params().iter().flat_map(|param| {
        let mut referenced = Vec::new();
        if let Some(GenericArg::Type(ty)) = param.default() {
            value_type_references(ty, &mut referenced);
        }
        referenced.into_iter().map(|to| TypeCycleStep {
            from,
            site: TypeReferenceSite::GenericDefault {
                param: param.name().clone(),
            },
            to: to.clone(),
        })
    });
    let fields = definition
        .union_members()
        .into_iter()
        .flatten()
        .flat_map(|constructor| {
            constructor.fields().iter().flat_map(move |field| {
                let mut referenced = Vec::new();
                decl_type_references(&field.type_annotation().decl_type, &mut referenced);
                referenced.into_iter().map(move |to| TypeCycleStep {
                    from,
                    site: TypeReferenceSite::Field {
                        constructor: constructor.name(),
                        field: field.name().clone(),
                    },
                    to: to.clone(),
                })
            })
        });
    defaults.chain(fields).collect()
}

fn decl_type_references<'a>(
    ty: &'a DeclType,
    output: &mut Vec<&'a Spanned<ResolvedStructTypeName>>,
) {
    match ty {
        DeclType::Value(value) | DeclType::Indexed { element: value, .. } => {
            value_type_references(value, output);
        }
    }
}

fn value_type_references<'a>(
    ty: &'a ValueType,
    output: &mut Vec<&'a Spanned<ResolvedStructTypeName>>,
) {
    match &ty.kind {
        ValueTypeKind::Struct(name) => output.push(name),
        ValueTypeKind::TypeApplication { name, generic_args } => {
            output.push(name);
            for arg in generic_args {
                match arg {
                    GenericArg::Type(argument) => value_type_references(argument, output),
                    GenericArg::Dim(_) | GenericArg::Index(_) | GenericArg::Nat(_) => {}
                }
            }
        }
        ValueTypeKind::Builtin(_)
        | ValueTypeKind::DimExpr(_)
        | ValueTypeKind::GenericTypeParam(_)
        | ValueTypeKind::Complex(_)
        | ValueTypeKind::Key(_) => {}
    }
}
