//! The wire identity of a dimension or an index a model declares.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::label::{ModuleName, PackageName};

/// A declaration named as the user names it on an import path: the declaring
/// package, the module path below it, and the declared name.
///
/// The identity does not depend on where the files live: the same model in
/// another directory, or under another source directory, has the same
/// identities. Two declarations with the same name in different modules have
/// different identities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredName<L> {
    package: PackageName,
    module: Vec<ModuleName>,
    name: L,
}

impl<L> DeclaredName<L> {
    /// The declaration `name` of the module `package.module…`; an empty
    /// `module` is the package's root module.
    #[must_use]
    pub const fn new(package: PackageName, module: Vec<ModuleName>, name: L) -> Self {
        Self {
            package,
            module,
            name,
        }
    }

    /// The declaring package.
    #[must_use]
    pub const fn package(&self) -> &PackageName {
        &self.package
    }

    /// The module path below the package; empty for its root module.
    #[must_use]
    pub fn module(&self) -> &[ModuleName] {
        &self.module
    }

    /// The declared name.
    #[must_use]
    pub const fn name(&self) -> &L {
        &self.name
    }
}

impl<L: fmt::Display> fmt::Display for DeclaredName<L> {
    /// The import-path spelling, `package.module.name`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.package)?;
        for segment in &self.module {
            write!(formatter, ".{segment}")?;
        }
        write!(formatter, ".{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::IndexName;

    #[test]
    fn names_travel_as_typed_parts_and_display_as_import_paths() {
        let name = DeclaredName::new(
            PackageName::new("rocket").unwrap(),
            vec![ModuleName::new("units").unwrap()],
            IndexName::new("Phase").unwrap(),
        );
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(
            json,
            r#"{"package":"rocket","module":["units"],"name":"Phase"}"#
        );
        assert_eq!(
            serde_json::from_str::<DeclaredName<IndexName>>(&json).unwrap(),
            name
        );
        assert_eq!(name.to_string(), "rocket.units.Phase");
        assert!(
            serde_json::from_str::<DeclaredName<IndexName>>(
                r#"{"package":"rocket","module":[],"name":"Phase","file":"x.gcl"}"#
            )
            .is_err()
        );
    }
}
