//! Expression-lowerer state and its lexical-scope bookkeeping.

use std::collections::HashMap;

use crate::resolve::namespace::Namespace;
use crate::resolve::reserved_name::validate_reserved_name;
use crate::syntax::local_name::LocalName;
use crate::syntax::span::Span;

use super::context::ExprLoweringContext;
use super::error::ExprLowerError;
use crate::hir::expr::{LocalDef, LocalId};

pub(super) struct ExprLowerer<'a> {
    pub(super) ctx: ExprLoweringContext<'a>,
    pub(super) local_scopes: Vec<HashMap<LocalName, LocalDef>>,
    pub(super) next_local: u32,
}

impl<'a> ExprLowerer<'a> {
    pub(super) const fn new(ctx: ExprLoweringContext<'a>) -> Self {
        Self {
            ctx,
            local_scopes: Vec::new(),
            next_local: 0,
        }
    }

    pub(super) fn allocate_local(
        &mut self,
        name: LocalName,
        span: Span,
    ) -> Result<LocalDef, ExprLowerError> {
        let id = LocalId(self.next_local);
        let Some(next_local) = self.next_local.checked_add(1) else {
            return Err(ExprLowerError::TooManyLocals { span });
        };
        self.next_local = next_local;
        Ok(LocalDef { id, name, span })
    }

    pub(super) fn push_scope(&mut self, bindings: Vec<LocalDef>) -> Result<(), ExprLowerError> {
        let mut scope = HashMap::new();
        for binding in bindings {
            if let Some(first) = scope.get(&binding.name).cloned().or_else(|| {
                self.local_scopes
                    .iter()
                    .rev()
                    .find_map(|visible| visible.get(&binding.name).cloned())
            }) {
                return Err(ExprLowerError::DuplicateLocalBinding {
                    name: binding.name,
                    first: first.span,
                    duplicate: binding.span,
                });
            }
            let atom = binding.name.atom();
            let builtin_occupied = validate_reserved_name(Namespace::Term, atom).is_err();
            let visible = self
                .ctx
                .scope
                .resolver
                .visible_span(self.ctx.scope.owner, Namespace::Term, atom)
                .map_err(|source| ExprLowerError::ModuleResolve {
                    source,
                    span: binding.span,
                })?;
            if builtin_occupied || visible.is_some() {
                return Err(ExprLowerError::LocalBindingShadowsTerm {
                    name: binding.name,
                    original: visible,
                    duplicate: binding.span,
                });
            }
            scope.insert(binding.name.clone(), binding);
        }
        self.local_scopes.push(scope);
        Ok(())
    }

    pub(super) fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }

    pub(super) fn lookup_local(
        &self,
        name: &LocalName,
        span: Span,
    ) -> Result<LocalId, ExprLowerError> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .map(|def| def.id)
            .ok_or_else(|| ExprLowerError::UnknownLocalRef {
                name: name.clone(),
                span,
            })
    }
}
