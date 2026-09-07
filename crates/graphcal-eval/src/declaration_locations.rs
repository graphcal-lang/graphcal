//! Unique physical declaration locations prepared from completed body indexes.

use std::collections::{HashMap, hash_map::Entry};

use graphcal_compiler::dag_id::DagId;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;

/// An authoritative location index, not a semantic-owner fallback.
#[derive(Debug)]
pub struct DeclarationLocations {
    bodies: HashMap<RuntimeDeclKey, DagId>,
}

#[derive(Debug, Error)]
pub enum DeclarationLocationError {
    #[error(
        "declaration `{declaration}` has duplicate physical locations in `{first}` and `{second}`"
    )]
    Duplicate {
        declaration: RuntimeDeclKey,
        first: DagId,
        second: DagId,
    },
    #[error("declaration `{0}` has no prepared physical location")]
    Missing(RuntimeDeclKey),
}

impl DeclarationLocations {
    pub(crate) fn try_new(
        entries: impl IntoIterator<Item = (RuntimeDeclKey, DagId)>,
    ) -> Result<Self, DeclarationLocationError> {
        entries
            .into_iter()
            .try_fold(
                HashMap::new(),
                |mut bodies, (declaration, body)| match bodies.entry(declaration) {
                    Entry::Vacant(entry) => {
                        entry.insert(body);
                        Ok(bodies)
                    }
                    Entry::Occupied(entry) => Err(DeclarationLocationError::Duplicate {
                        declaration: entry.key().clone(),
                        first: entry.get().clone(),
                        second: body,
                    }),
                },
            )
            .map(|bodies| Self { bodies })
    }

    pub(crate) fn body_for(
        &self,
        declaration: &RuntimeDeclKey,
    ) -> Result<&DagId, DeclarationLocationError> {
        self.bodies
            .get(declaration)
            .ok_or_else(|| DeclarationLocationError::Missing(declaration.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};

    fn owner(file: &str) -> DagId {
        DagId::from_virtual_relative_path(std::path::Path::new(file)).unwrap()
    }

    fn declaration() -> RuntimeDeclKey {
        RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
            owner("instance.gcl"),
            DeclName::expect_valid("answer"),
        ))
    }

    #[test]
    fn physical_body_is_not_inferred_from_semantic_owner() {
        let declaration = declaration();
        let physical = owner("importer.gcl");
        let locations =
            DeclarationLocations::try_new([(declaration.clone(), physical.clone())]).unwrap();
        assert_eq!(locations.body_for(&declaration).unwrap(), &physical);
        assert_ne!(
            locations.body_for(&declaration).unwrap(),
            declaration.as_resolved().owner()
        );
    }

    #[test]
    fn duplicate_locations_are_rejected_even_when_bodies_match() {
        let first = owner("first.gcl");
        for second in [first.clone(), owner("second.gcl")] {
            assert!(matches!(
                DeclarationLocations::try_new([
                    (declaration(), first.clone()),
                    (declaration(), second)
                ]),
                Err(DeclarationLocationError::Duplicate { .. })
            ));
        }
    }

    #[test]
    fn missing_locations_are_errors() {
        let locations = DeclarationLocations::try_new([]).unwrap();
        assert!(matches!(
            locations.body_for(&declaration()),
            Err(DeclarationLocationError::Missing(_))
        ));
    }
}
