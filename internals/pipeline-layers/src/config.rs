use crate::model::{EdgeKey, ModuleId, Package, Role};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct RawRoleMap {
    module: Vec<RawModule>,
}
#[derive(Debug, Deserialize)]
struct RawModule {
    package: String,
    path: Vec<String>,
    role: String,
}
#[derive(Debug, Deserialize)]
struct RawBaseline {
    exception: Vec<RawException>,
}
#[derive(Debug, Deserialize)]
struct RawException {
    from_package: String,
    from_path: Vec<String>,
    to_package: String,
    to_path: Vec<String>,
    test_only: bool,
    reason: String,
}

#[derive(Clone, Debug)]
pub struct RoleMap {
    entries: BTreeMap<ModuleId, Role>,
}

impl RoleMap {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read role map {}", path.display()))?;
        let raw: RawRoleMap =
            toml::from_str(&text).with_context(|| format!("parse role map {}", path.display()))?;
        let mut entries = BTreeMap::new();
        raw.module.into_iter().try_for_each(|item| {
            let package = Package::from_name(&item.package)
                .ok_or_else(|| anyhow::anyhow!("unknown package {:?}", item.package))?;
            let role = Role::parse(&item.role)
                .ok_or_else(|| anyhow::anyhow!("unknown role {:?}", item.role))?;
            if entries
                .insert(ModuleId::new(package, item.path), role)
                .is_some()
            {
                bail!("duplicate role-map module");
            }
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(Self { entries })
    }

    pub fn validate(&self, modules: &BTreeSet<ModuleId>) -> Result<()> {
        let unknown = self
            .entries
            .keys()
            .filter(|id| !modules.contains(*id))
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            bail!(
                "role map contains modules absent from source: {}",
                unknown
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let missing = modules
            .iter()
            .filter(|id| self.role_for(id).is_none())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "role map is missing modules: {}",
                missing
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }

    pub fn role_for(&self, id: &ModuleId) -> Option<Role> {
        self.entries.get(id).copied()
    }

    pub fn nearest_role_for(&self, id: &ModuleId) -> Option<Role> {
        (0..=id.path.len()).rev().find_map(|length| {
            self.entries
                .get(&ModuleId::new(
                    id.package.clone(),
                    id.path[..length].to_vec(),
                ))
                .copied()
        })
    }
}

#[derive(Clone, Debug)]
pub struct Baseline {
    pub exceptions: BTreeMap<EdgeKey, String>,
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read baseline {}", path.display()))?;
        let raw: RawBaseline =
            toml::from_str(&text).with_context(|| format!("parse baseline {}", path.display()))?;
        let mut exceptions = BTreeMap::new();
        raw.exception.into_iter().try_for_each(|item| {
            let from_package = Package::from_name(&item.from_package).ok_or_else(|| {
                anyhow::anyhow!("unknown baseline package {:?}", item.from_package)
            })?;
            let to_package = Package::from_name(&item.to_package)
                .ok_or_else(|| anyhow::anyhow!("unknown baseline package {:?}", item.to_package))?;
            if item.reason.trim().is_empty() {
                bail!("baseline exception has an empty reason");
            }
            let key = EdgeKey {
                from: ModuleId::new(from_package, item.from_path),
                to: ModuleId::new(to_package, item.to_path),
                test_only: item.test_only,
            };
            if exceptions.insert(key, item.reason).is_some() {
                bail!("duplicate baseline exception");
            }
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(Self { exceptions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_file(label: &str, content: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("graphcal-layer-{label}-{stamp}.toml"));
        fs::write(&path, content).expect("write temporary TOML");
        path
    }

    #[test]
    fn malformed_role_and_baseline_entries_fail() {
        let role = temporary_file(
            "role",
            "[[module]]\npackage = \"eval\"\npath = []\nrole = \"unknown\"\n",
        );
        assert!(RoleMap::load(&role).is_err());
        let baseline = temporary_file(
            "baseline",
            "[[exception]]\nfrom_package = \"unknown\"\nfrom_path = []\nto_package = \"eval\"\nto_path = []\ntest_only = false\nreason = \"why\"\n",
        );
        assert!(Baseline::load(&baseline).is_err());
        let _ = fs::remove_file(role);
        let _ = fs::remove_file(baseline);
    }

    #[test]
    fn role_validation_rejects_missing_and_stale_modules() {
        let path = temporary_file(
            "role-validation",
            "[[module]]\npackage = \"eval\"\npath = []\nrole = \"facade\"\n",
        );
        let roles = RoleMap::load(&path).expect("role map parses");
        assert!(
            roles
                .validate(&BTreeSet::from([ModuleId::new(
                    Package::Eval,
                    vec!["new".into()]
                )]))
                .is_err()
        );
        assert!(
            roles
                .validate(&BTreeSet::from([ModuleId::new(Package::Eval, vec![])]))
                .is_ok()
        );
        let _ = fs::remove_file(path);
    }
}
