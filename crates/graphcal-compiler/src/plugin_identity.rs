//! Resolved plugin identities, separate from source-level import spellings.

use crate::dag_id::DagPackageId;
use crate::syntax::function_name::FnName;
use crate::syntax::plugin::{PluginPath, PluginSourceKind};

/// A Wasm artifact belongs to a package instance; host functions belong to the
/// embedding application. Neither source aliases nor module names are identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PluginIdentity {
    Host(PluginPath),
    Wasm {
        package: DagPackageId,
        path: PluginPath,
    },
}

impl PluginIdentity {
    /// Resolve an import spelling in its declaring package's scope.
    #[must_use]
    pub fn resolve(path: &PluginPath, package: &DagPackageId) -> Self {
        match path.source_kind() {
            PluginSourceKind::HostRegistry => Self::Host(path.clone()),
            PluginSourceKind::WasmModule => Self::Wasm {
                package: package.clone(),
                path: path.clone(),
            },
        }
    }

    /// Source spelling, used only for artifact lookup and diagnostic rendering.
    #[must_use]
    pub const fn path(&self) -> &PluginPath {
        match self {
            Self::Host(path) | Self::Wasm { path, .. } => path,
        }
    }

    /// Owning package; embedder-provided host functions are intentionally global.
    #[must_use]
    pub const fn package(&self) -> Option<&DagPackageId> {
        match self {
            Self::Host(_) => None,
            Self::Wasm { package, .. } => Some(package),
        }
    }
}

impl std::fmt::Display for PluginIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(path) => path.fmt(formatter),
            Self::Wasm { package, path } => write!(formatter, "{path} (package {package})"),
        }
    }
}

/// Canonical callable identity, independent of import aliases.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternFnKey {
    pub plugin: PluginIdentity,
    pub name: FnName,
}

/// SHA-256 digest of a plugin artifact's bytes: a `graphcal.lock` pin or the
/// hash of the file on disk. Rendered as 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginDigest([u8; 32]);

impl PluginDigest {
    /// The digest with this exact binary representation.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Display for PluginDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_versions_do_not_alias_but_host_functions_do() {
        let first = DagPackageId::new("first-version");
        let second = DagPackageId::new("second-version");
        let wasm = PluginPath::new("plugins/solver.wasm");
        assert_ne!(
            PluginIdentity::resolve(&wasm, &first),
            PluginIdentity::resolve(&wasm, &second)
        );
        let host = PluginPath::new("graphcal:demo");
        assert_eq!(
            PluginIdentity::resolve(&host, &first),
            PluginIdentity::resolve(&host, &second)
        );
    }

    #[test]
    fn plugin_digests_render_as_lowercase_hex() {
        let mut bytes = [0_u8; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x0f;
        let rendered = PluginDigest::from_bytes(bytes).to_string();
        assert_eq!(rendered.len(), 64);
        assert!(rendered.starts_with("ab00"));
        assert!(rendered.ends_with("000f"));
    }
}
