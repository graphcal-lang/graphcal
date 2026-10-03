//! Source module paths of the files a project loaded, for naming declarations
//! of modules no evaluated root can name.
//!
//! A module's [`DagId`] is its identity (package-relative file components,
//! then inline-DAG and instance segments), which source never spells. The
//! project records each loaded file under the module path its importers write
//! (`pipeline.lib`); output boundaries render the declarations of those
//! modules through it (`pipeline.lib.helper::pending`).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

use crate::dag_id::{DagId, DagSegment};
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_path_key::ModulePathKey;
use crate::syntax::non_empty::NonEmpty;

/// A declaration named by the source module path of its module.
///
/// Inline DAGs extend the module path, and the concrete instance scopes below
/// it qualify the declaration (`pipeline.lib.helper::inst::pending`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDeclName {
    module: ModulePathKey,
    name: ScopedName,
}

impl ModuleDeclName {
    /// The source module path of the declaring module.
    #[must_use]
    pub const fn module(&self) -> &ModulePathKey {
        &self.module
    }
}

impl fmt::Display for ModuleDeclName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.module, self.name)
    }
}

/// The source module path of each loaded file root, by identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModulePaths(HashMap<DagId, ModulePathKey>);

impl ModulePaths {
    /// Record that the import or include path `path` names `target`, a file
    /// root or an inline DAG declared in one: the path's leading segments
    /// name the file.
    ///
    /// When importers spell one file differently (a dependency imported under
    /// different aliases), the least spelling is kept, so names do not depend
    /// on recording order. A path too short to name `target` records nothing.
    pub fn record(&mut self, path: &ModulePathKey, target: &DagId) {
        let file = target.file_root();
        let inline = target
            .segments()
            .len()
            .saturating_sub(file.segments().len());
        let segments = path.segments().as_slice();
        let Some(file_segments) = segments
            .len()
            .checked_sub(inline)
            .and_then(|kept| NonEmpty::try_from_vec(segments[..kept].to_vec()).ok())
        else {
            return;
        };
        let file_path = ModulePathKey::new(file_segments);
        match self.0.entry(file) {
            Entry::Vacant(entry) => {
                entry.insert(file_path);
            }
            Entry::Occupied(mut entry) => {
                if file_path < *entry.get() {
                    entry.insert(file_path);
                }
            }
        }
    }

    /// The declaration `leaf` of module `owner`, named by the source module
    /// path of `owner`'s file, or `None` when no path to that file was
    /// recorded.
    #[must_use]
    pub fn name(&self, owner: &DagId, leaf: &DeclName) -> Option<ModuleDeclName> {
        let file = owner.file_root();
        let file_path = self.0.get(&file)?;
        let below = owner.segments().as_slice().get(file.segments().len()..)?;
        let inline_dags = below
            .iter()
            .map_while(DagSegment::inline_dag)
            .map(|name| name.atom().clone())
            .collect::<Vec<_>>();
        // File segments are a prefix of every identity, so the segments after
        // the inline DAGs are all instance scopes.
        let scopes = below[inline_dags.len()..]
            .iter()
            .map(DagSegment::scope)
            .collect::<Option<Vec<_>>>()?;
        let (first, rest) = file_path.segments().split_first();
        let module = ModulePathKey::new(NonEmpty::new(
            first.clone(),
            rest.iter().cloned().chain(inline_dags).collect(),
        ));
        Some(ModuleDeclName {
            module,
            name: ScopedName::from_parts(NonEmpty::try_from_vec(scopes).ok(), leaf.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};
    use crate::syntax::names::NameAtom;

    use super::*;

    fn path(segments: &[&str]) -> ModulePathKey {
        let atoms = segments
            .iter()
            .map(|segment| NameAtom::parse(*segment).unwrap())
            .collect();
        ModulePathKey::new(NonEmpty::try_from_vec(atoms).unwrap())
    }

    fn lib() -> DagId {
        DagId::new("pipeline", NonEmpty::new("src", vec!["pipeline", "lib"]))
    }

    fn leaf(name: &str) -> DeclName {
        DeclName::expect_valid(name)
    }

    #[test]
    fn declarations_take_the_module_path_of_their_file() {
        let mut paths = ModulePaths::default();
        let helper = lib().inline_dag_child(leaf("helper"));
        // Recorded through an inline DAG: the file keeps the path's prefix.
        paths.record(&path(&["pipeline", "lib", "helper"]), &helper);

        let name = paths.name(&lib(), &leaf("x")).unwrap();
        assert_eq!(name.module(), &path(&["pipeline", "lib"]));
        assert_eq!(name.to_string(), "pipeline.lib::x");
        assert_eq!(
            paths.name(&helper, &leaf("pending")).unwrap().to_string(),
            "pipeline.lib.helper::pending"
        );
        let instance =
            helper.instance_child(ScopeSegment::Named(ModuleAliasName::expect_valid("inst")));
        assert_eq!(
            paths.name(&instance, &leaf("y")).unwrap().to_string(),
            "pipeline.lib.helper::inst::y"
        );
        let other = DagId::new("pipeline", NonEmpty::new("src", vec!["pipeline", "other"]));
        assert_eq!(paths.name(&other, &leaf("x")), None);
    }

    #[test]
    fn the_least_spelling_of_a_file_is_kept() {
        let mut paths = ModulePaths::default();
        paths.record(&path(&["zeta", "lib"]), &lib());
        paths.record(&path(&["alpha", "lib"]), &lib());
        paths.record(&path(&["beta", "lib"]), &lib());
        assert_eq!(
            paths.name(&lib(), &leaf("x")).unwrap().to_string(),
            "alpha.lib::x"
        );
    }

    #[test]
    fn a_path_too_short_for_its_target_records_nothing() {
        let mut paths = ModulePaths::default();
        let nested = lib()
            .inline_dag_child(leaf("a"))
            .inline_dag_child(leaf("b"));
        paths.record(&path(&["a", "b"]), &nested);
        assert_eq!(paths.name(&lib(), &leaf("x")), None);
    }
}
