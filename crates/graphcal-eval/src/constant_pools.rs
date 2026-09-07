//! Immutable constant-pool views. Preparing closures never copies constant payloads.

use crate::decl_key::RuntimeDeclKey;
use crate::execution_facts::RuntimeValueMap;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConstantPoolError {
    #[error("constant `{0}` occurs in multiple prepared pools")]
    Duplicate(RuntimeDeclKey),
    #[error("constant `{0}` is absent from its retained pool")]
    Missing(RuntimeDeclKey),
}

#[derive(Debug)]
pub struct ConstantPools {
    pools: Vec<Arc<RuntimeValueMap>>,
    locations: HashMap<RuntimeDeclKey, Arc<RuntimeValueMap>>,
}

impl ConstantPools {
    pub fn try_new(
        pools: impl IntoIterator<Item = Arc<RuntimeValueMap>>,
    ) -> Result<Self, ConstantPoolError> {
        pools.into_iter().try_fold(
            Self {
                pools: Vec::new(),
                locations: HashMap::new(),
            },
            |mut result, pool| {
                for key in pool.keys() {
                    if result
                        .locations
                        .insert(key.clone(), Arc::clone(&pool))
                        .is_some()
                    {
                        return Err(ConstantPoolError::Duplicate(key.clone()));
                    }
                }
                result.pools.push(pool);
                Ok(result)
            },
        )
    }

    pub fn get(&self, key: &RuntimeDeclKey) -> Option<&RuntimeValue> {
        self.locations.get(key).and_then(|pool| pool.get(key))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RuntimeDeclKey, &RuntimeValue)> {
        self.pools.iter().flat_map(|pool| pool.iter())
    }
}

/// A validated import into its defining body's immutable pool, not a copied value.
#[derive(Debug)]
pub struct ConstantReference {
    pool: Arc<RuntimeValueMap>,
    key: RuntimeDeclKey,
}

impl ConstantReference {
    pub fn try_new(
        pool: Arc<RuntimeValueMap>,
        key: RuntimeDeclKey,
    ) -> Result<Self, ConstantPoolError> {
        if !pool.contains_key(&key) {
            return Err(ConstantPoolError::Missing(key));
        }
        Ok(Self { pool, key })
    }

    pub fn value(&self) -> Result<&RuntimeValue, ConstantPoolError> {
        self.pool
            .get(&self.key)
            .ok_or_else(|| ConstantPoolError::Missing(self.key.clone()))
    }
}
