use crate::model::{ModuleId, Package};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};
use syn::{File, Item, ItemMod};

pub struct ModuleSource {
    pub id: ModuleId,
    pub file: PathBuf,
    pub syntax: File,
}

pub struct SourceTree {
    pub modules: BTreeMap<ModuleId, ModuleSource>,
}

impl SourceTree {
    pub fn discover(repo: &Path) -> Result<Self> {
        let modules = [
            (Package::Compiler, repo.join("crates/graphcal-compiler/src")),
            (Package::Eval, repo.join("crates/graphcal-eval/src")),
        ]
        .into_iter()
        .try_fold(BTreeMap::new(), |mut all, (package, root)| {
            let files = rust_files(&root)?;
            files.into_iter().try_for_each(|file| {
                let relative = file.strip_prefix(&root).expect("file below source root");
                let path = module_path(relative)?;
                let text = fs::read_to_string(&file)
                    .with_context(|| format!("read {}", file.display()))?;
                let syntax = syn::parse_file(&text)
                    .map_err(|error| anyhow::anyhow!("parse {}: {}", file.display(), error))?;
                let id = ModuleId::new(package.clone(), path);
                if all
                    .insert(id.clone(), ModuleSource { id, file, syntax })
                    .is_some()
                {
                    bail!("duplicate module path");
                }
                Ok::<(), anyhow::Error>(())
            })?;
            Ok::<_, anyhow::Error>(all)
        })?;

        let mut tree = Self { modules };
        tree.reject_conditional_path_attributes()?;
        tree.remap_literal_path_modules()?;
        tree.discover_inline_modules()?;
        tree.validate_module_graph()?;
        Ok(tree)
    }

    fn remap_literal_path_modules(&mut self) -> Result<()> {
        let mut remaps = BTreeMap::new();
        self.modules.values().try_for_each(|source| {
            source.syntax.items.iter().try_for_each(|item| {
                let Item::Mod(module) = item else {
                    return Ok(());
                };
                let Some(path) = literal_path_attribute(module)? else {
                    return Ok(());
                };
                let target_file = source
                    .file
                    .parent()
                    .expect("source file has a parent")
                    .join(path);
                let target = self
                    .modules
                    .values()
                    .find(|candidate| candidate.file == target_file)
                    .map(|candidate| candidate.id.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "#[path] module {} points to missing {}",
                            module.ident,
                            target_file.display()
                        )
                    })?;
                let semantic = child_id(&source.id, &module.ident.to_string());
                if remaps.insert(target.clone(), semantic).is_some() {
                    bail!("multiple #[path] modules target {}", target_file.display());
                }
                Ok::<(), anyhow::Error>(())
            })
        })?;

        let old = std::mem::take(&mut self.modules);
        old.into_iter().try_for_each(|(id, mut source)| {
            let semantic = remapped_id(&id, &remaps);
            if semantic != id {
                source.id = semantic.clone();
            }
            if self.modules.insert(semantic, source).is_some() {
                bail!("literal #[path] mapping collides with another module");
            }
            Ok::<(), anyhow::Error>(())
        })
    }

    fn discover_inline_modules(&mut self) -> Result<()> {
        let roots = self
            .modules
            .values()
            .map(|source| {
                (
                    source.id.clone(),
                    source.file.clone(),
                    source.syntax.items.clone(),
                )
            })
            .collect::<Vec<_>>();
        roots.into_iter().try_for_each(|(parent, file, items)| {
            discover_inline(&mut self.modules, &parent, &file, &items)
        })
    }

    fn reject_conditional_path_attributes(&self) -> Result<()> {
        self.modules.values().try_for_each(|source| {
            let mut visitor = ConditionalPathVisitor {
                file: &source.file,
                error: None,
            };
            visitor.visit_file(&source.syntax);
            visitor.result()
        })
    }

    fn validate_module_graph(&self) -> Result<()> {
        let roots = self
            .modules
            .keys()
            .filter(|id| id.path.is_empty())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut reachable = BTreeSet::new();
        let mut pending = roots.into_iter().collect::<Vec<_>>();
        while let Some(module) = pending.pop() {
            if !reachable.insert(module.clone()) {
                continue;
            }
            let source = self.modules.get(&module).expect("reachable module exists");
            for item in &source.syntax.items {
                let Item::Mod(child) = item else { continue };
                let id = child_id(&module, &child.ident.to_string());
                if !self.modules.contains_key(&id) {
                    bail!(
                        "module declaration {} in {} points to missing source",
                        child.ident,
                        source.file.display()
                    );
                }
                pending.push(id);
            }
        }
        let unreachable = self
            .modules
            .keys()
            .filter(|id| !reachable.contains(*id))
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if unreachable.is_empty() {
            Ok(())
        } else {
            bail!(
                "source modules are not declared by a reachable mod item: {}",
                unreachable.join(", ")
            )
        }
    }
}

fn literal_path_attribute(module: &ItemMod) -> Result<Option<PathBuf>> {
    module
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("path"))
        .try_fold(None, |found, attr| {
            let syn::Meta::NameValue(name_value) = &attr.meta else {
                bail!("#[path] must contain a string literal");
            };
            let syn::Expr::Lit(expr) = &name_value.value else {
                bail!("#[path] must contain a string literal");
            };
            let syn::Lit::Str(value) = &expr.lit else {
                bail!("#[path] must contain a string literal");
            };
            if found.is_some() {
                bail!("module has duplicate #[path] attributes");
            }
            Ok(Some(PathBuf::from(value.value())))
        })
}

fn remapped_id(id: &ModuleId, remaps: &BTreeMap<ModuleId, ModuleId>) -> ModuleId {
    let Some((old, semantic)) = remaps
        .iter()
        .filter(|(old, _)| {
            old.package == id.package
                && old.path.len() <= id.path.len()
                && old
                    .path
                    .iter()
                    .zip(&id.path)
                    .all(|(left, right)| left == right)
        })
        .max_by_key(|(old, _)| old.path.len())
    else {
        return id.clone();
    };
    let mut path = semantic.path.clone();
    path.extend(id.path[old.path.len()..].iter().cloned());
    ModuleId::new(semantic.package.clone(), path)
}

fn child_id(current: &ModuleId, name: &str) -> ModuleId {
    let mut path = current.path.clone();
    path.push(name.to_owned());
    ModuleId::new(current.package.clone(), path)
}

fn discover_inline(
    modules: &mut BTreeMap<ModuleId, ModuleSource>,
    parent: &ModuleId,
    file: &Path,
    items: &[Item],
) -> Result<()> {
    reject_block_modules(items, file)?;
    items.iter().try_for_each(|item| {
        let Item::Mod(module) = item else {
            return Ok(());
        };
        let mut path = parent.path.clone();
        path.push(module.ident.to_string());
        let id = ModuleId::new(parent.package.clone(), path);
        if let Some((_, content)) = &module.content {
            if modules.contains_key(&id) {
                bail!("inline module {} duplicates a source file", id);
            }
            let syntax = File {
                shebang: None,
                frontmatter: None,
                attrs: Vec::new(),
                items: content.clone(),
            };
            modules.insert(
                id.clone(),
                ModuleSource {
                    id: id.clone(),
                    file: file.to_path_buf(),
                    syntax,
                },
            );
            discover_inline(modules, &id, file, content)?;
        }
        Ok(())
    })
}

struct ConditionalPathVisitor<'a> {
    file: &'a Path,
    error: Option<anyhow::Error>,
}

impl<'a> ConditionalPathVisitor<'a> {
    fn result(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }
}

fn meta_contains_path(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::NameValue(value) => value.path.is_ident("path"),
        syn::Meta::Path(path) => path.is_ident("path"),
        syn::Meta::List(list) => {
            list.path.is_ident("path")
                || list
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                    )
                    .map(|metas| metas.iter().any(meta_contains_path))
                    .unwrap_or(true)
        }
    }
}

impl<'ast> Visit<'ast> for ConditionalPathVisitor<'_> {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("cfg_attr")
            && let syn::Meta::List(list) = &attribute.meta
        {
            let contains_path = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .map(|metas| metas.iter().any(meta_contains_path))
                .unwrap_or(true);
            if contains_path && self.error.is_none() {
                self.error = Some(anyhow::anyhow!(
                    "conditional #[path] mapping in {} is unsupported; use a literal #[path]",
                    self.file.display()
                ));
            }
        }
        visit::visit_attribute(self, attribute);
    }
}

struct BlockModuleVisitor<'a> {
    file: &'a Path,
    in_block: bool,
    error: Option<anyhow::Error>,
}

impl<'a> BlockModuleVisitor<'a> {
    fn result(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }
}

impl<'ast> Visit<'ast> for BlockModuleVisitor<'_> {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let previous = self.in_block;
        self.in_block = true;
        visit::visit_block(self, block);
        self.in_block = previous;
    }

    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        if self.in_block && self.error.is_none() {
            self.error = Some(anyhow::anyhow!(
                "block-local module {} in {} is unsupported; move it to module scope",
                module.ident,
                self.file.display()
            ));
        }
        visit::visit_item_mod(self, module);
    }
}

fn reject_block_modules(items: &[Item], file: &Path) -> Result<()> {
    let syntax = syn::File {
        shebang: None,
        frontmatter: None,
        attrs: Vec::new(),
        items: items.to_vec(),
    };
    let mut visitor = BlockModuleVisitor {
        file,
        in_block: false,
        error: None,
    };
    visitor.visit_file(&syntax);
    visitor.result()
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    fs::read_dir(dir)
        .with_context(|| format!("read directory {}", dir.display()))?
        .try_for_each(|entry| {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                collect(&path, files)
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
                Ok(())
            } else {
                Ok(())
            }
        })
}

fn module_path(relative: &Path) -> Result<Vec<String>> {
    let mut components = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let file = components
        .pop()
        .ok_or_else(|| anyhow::anyhow!("empty Rust path"))?;
    if file == "lib.rs" || file == "main.rs" || file == "mod.rs" {
        return Ok(components);
    }
    let stem = file
        .strip_suffix(".rs")
        .ok_or_else(|| anyhow::anyhow!("not Rust: {file}"))?;
    components.push(stem.to_owned());
    Ok(components)
}

pub fn module_ids(tree: &SourceTree) -> BTreeSet<ModuleId> {
    tree.modules.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn discovery_rejects_conditional_paths_and_missing_modules() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("graphcal-discover-reject-{stamp}"));
        let eval = root.join("crates/graphcal-eval/src");
        let compiler = root.join("crates/graphcal-compiler/src");
        fs::create_dir_all(&eval).expect("create eval fixture");
        fs::create_dir_all(&compiler).expect("create compiler fixture");
        fs::write(
            eval.join("lib.rs"),
            "#[cfg_attr(feature = \"x\", path = \"other.rs\")] mod mapped;",
        )
        .expect("write conditional mapping");
        fs::write(compiler.join("lib.rs"), "").expect("write compiler fixture");
        assert!(SourceTree::discover(&root).is_err());
        fs::write(eval.join("lib.rs"), "mod missing;").expect("write missing module");
        assert!(SourceTree::discover(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn literal_path_modules_use_semantic_paths_and_descendants() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("graphcal-discover-{stamp}"));
        let eval = root.join("crates/graphcal-eval/src");
        let compiler = root.join("crates/graphcal-compiler/src");
        fs::create_dir_all(eval.join("actual")).expect("create eval fixture");
        fs::create_dir_all(&compiler).expect("create compiler fixture");
        fs::write(eval.join("lib.rs"), "#[path = \"actual.rs\"] mod semantic;")
            .expect("write root");
        fs::write(eval.join("actual.rs"), "mod nested;").expect("write mapped module");
        fs::write(eval.join("actual/nested.rs"), "").expect("write mapped child");
        fs::write(compiler.join("lib.rs"), "").expect("write compiler root");

        let tree = SourceTree::discover(&root).expect("discover fixture");
        assert!(tree.modules.contains_key(&ModuleId::new(
            Package::Eval,
            vec!["semantic".into(), "nested".into()]
        )));
        assert!(!tree.modules.keys().any(|id| {
            id.package == Package::Eval && id.path.starts_with(&["actual".to_owned()])
        }));
        let _ = fs::remove_dir_all(root);
    }
}
