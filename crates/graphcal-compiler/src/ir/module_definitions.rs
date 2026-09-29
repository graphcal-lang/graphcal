//! Owner-qualified Static and Unit definitions contributed by one module.
//!
//! A module's dimensions, units, indexes, and nominal types are keyed by their
//! canonical [`ResolvedName`](crate::resolved_name::ResolvedName) identities
//! and are guaranteed to be owned by that module, so the project type store
//! can take them over without looking any source spelling up again.

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::dag_id::DagId;
use crate::dimension::{BaseDimId, Dimension};
use crate::hir::nominal::NominalTypeRegistry;
use crate::registry::dimension_table::BaseDimensionInfo;
use crate::registry::index::IndexDef;
use crate::registry::unit::UnitInfo;
use crate::resolved_name::{
    ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};

/// A definition presented to a module it is not owned by.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ForeignDefinitionError {
    #[error("dimension `{identity}` is not owned by module `{module}`")]
    Dimension {
        identity: ResolvedDimName,
        module: DagId,
    },
    #[error("unit `{identity}` is not owned by module `{module}`")]
    Unit {
        identity: ResolvedUnitName,
        module: DagId,
    },
    #[error("index `{identity}` is not owned by module `{module}`")]
    Index {
        identity: ResolvedIndexName,
        module: DagId,
    },
    #[error("nominal type `{identity}` is not owned by module `{module}`")]
    NominalType {
        identity: ResolvedStructTypeName,
        module: DagId,
    },
}

/// Dimensions, units, and indexes one module defines, plus the base-dimension
/// metadata (canonical unit, affine policy) it declares.
#[derive(Debug, Clone)]
pub struct StaticDefinitions {
    owner: DagId,
    dimensions: HashMap<ResolvedDimName, Dimension>,
    units: HashMap<ResolvedUnitName, UnitInfo>,
    indexes: HashMap<ResolvedIndexName, IndexDef>,
    base_dimensions: BTreeMap<BaseDimId, BaseDimensionInfo>,
}

impl StaticDefinitions {
    /// An empty definition set owned by `owner`.
    #[must_use]
    pub fn new(owner: DagId) -> Self {
        Self {
            owner,
            dimensions: HashMap::new(),
            units: HashMap::new(),
            indexes: HashMap::new(),
            base_dimensions: BTreeMap::new(),
        }
    }

    /// The module owning every definition in this set.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Record the definition of one of this module's dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignDefinitionError`] when `identity` belongs to another module.
    pub fn insert_dimension(
        &mut self,
        identity: ResolvedDimName,
        dimension: Dimension,
    ) -> Result<(), ForeignDefinitionError> {
        if identity.owner() != &self.owner {
            return Err(ForeignDefinitionError::Dimension {
                identity,
                module: self.owner.clone(),
            });
        }
        self.dimensions.insert(identity, dimension);
        Ok(())
    }

    /// Record the definition of one of this module's units.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignDefinitionError`] when `identity` belongs to another module.
    pub fn insert_unit(
        &mut self,
        identity: ResolvedUnitName,
        info: UnitInfo,
    ) -> Result<(), ForeignDefinitionError> {
        if identity.owner() != &self.owner {
            return Err(ForeignDefinitionError::Unit {
                identity,
                module: self.owner.clone(),
            });
        }
        self.units.insert(identity, info);
        Ok(())
    }

    /// Record the definition of one of this module's indexes.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignDefinitionError`] when `identity` belongs to another module.
    pub fn insert_index(
        &mut self,
        identity: ResolvedIndexName,
        definition: IndexDef,
    ) -> Result<(), ForeignDefinitionError> {
        if identity.owner() != &self.owner {
            return Err(ForeignDefinitionError::Index {
                identity,
                module: self.owner.clone(),
            });
        }
        self.indexes.insert(identity, definition);
        Ok(())
    }

    /// Record base-dimension metadata this module declares.
    pub fn insert_base_dimension(&mut self, id: BaseDimId, info: BaseDimensionInfo) {
        self.base_dimensions.insert(id, info);
    }

    /// This module's dimensions.
    pub fn dimensions(&self) -> impl Iterator<Item = (&ResolvedDimName, &Dimension)> {
        self.dimensions.iter()
    }

    /// This module's units.
    pub fn units(&self) -> impl Iterator<Item = (&ResolvedUnitName, &UnitInfo)> {
        self.units.iter()
    }

    /// This module's indexes.
    pub fn indexes(&self) -> impl Iterator<Item = (&ResolvedIndexName, &IndexDef)> {
        self.indexes.iter()
    }

    /// Base-dimension metadata declared by this module.
    pub fn base_dimensions(&self) -> impl Iterator<Item = (&BaseDimId, &BaseDimensionInfo)> {
        self.base_dimensions.iter()
    }

    /// Look up one of this module's dimensions.
    #[must_use]
    pub fn dimension(&self, identity: &ResolvedDimName) -> Option<&Dimension> {
        self.dimensions.get(identity)
    }

    /// Look up one of this module's units.
    #[must_use]
    pub fn unit(&self, identity: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.units.get(identity)
    }

    /// Look up one of this module's indexes.
    #[must_use]
    pub fn index(&self, identity: &ResolvedIndexName) -> Option<&IndexDef> {
        self.indexes.get(identity)
    }
}

/// Every Static and Unit definition one module contributes to the project,
/// all canonically owned by that module.
#[derive(Debug, Clone)]
pub struct ModuleDefinitions {
    statics: StaticDefinitions,
    nominal_types: NominalTypeRegistry,
}

impl ModuleDefinitions {
    /// Pair a module's Static definitions with its lowered nominal types.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignDefinitionError`] when a nominal type is owned by
    /// another module.
    pub fn try_new(
        statics: StaticDefinitions,
        nominal_types: NominalTypeRegistry,
    ) -> Result<Self, ForeignDefinitionError> {
        if let Some(foreign) = nominal_types
            .values()
            .find(|definition| definition.identity().owner() != statics.owner())
        {
            return Err(ForeignDefinitionError::NominalType {
                identity: foreign.identity().clone(),
                module: statics.owner().clone(),
            });
        }
        Ok(Self {
            statics,
            nominal_types,
        })
    }

    /// The module owning every definition.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        self.statics.owner()
    }

    /// Dimensions, units, indexes, and base-dimension metadata.
    #[must_use]
    pub const fn statics(&self) -> &StaticDefinitions {
        &self.statics
    }

    /// Nominal types with canonical HIR signatures.
    #[must_use]
    pub const fn nominal_types(&self) -> &NominalTypeRegistry {
        &self.nominal_types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::dimension::DimName;

    #[test]
    fn definitions_reject_identities_owned_by_another_module() {
        let owner = DagId::root_in_package("test", "owner");
        let other = DagId::root_in_package("test", "other");
        let mut statics = StaticDefinitions::new(owner.clone());
        let local = ResolvedDimName::from_def(owner, DimName::expect_valid("Local"));
        let foreign = ResolvedDimName::from_def(other, DimName::expect_valid("Foreign"));

        statics
            .insert_dimension(local.clone(), Dimension::dimensionless())
            .unwrap();
        assert!(matches!(
            statics.insert_dimension(foreign.clone(), Dimension::dimensionless()),
            Err(ForeignDefinitionError::Dimension { identity, .. }) if identity == foreign
        ));
        assert_eq!(statics.dimension(&local), Some(&Dimension::dimensionless()));
        assert_eq!(statics.dimension(&foreign), None);
    }
}
