#[cfg(test)]
mod tests {
    use crate::analyze::{analyze, forbidden};
    use crate::discover::{ModuleSource, SourceTree};
    use crate::model::{EvidenceForm, ModuleId, Package, Role};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn tree(root: &str, extra: &[(&[&str], &str)]) -> SourceTree {
        let mut modules = BTreeMap::new();
        insert(&mut modules, Package::Eval, &[], root, "root.rs");
        extra.iter().for_each(|(path, text)| {
            insert(&mut modules, Package::Eval, path, text, "module.rs");
        });
        insert(&mut modules, Package::Compiler, &[], "", "compiler.rs");
        insert(
            &mut modules,
            Package::Compiler,
            &["checking"],
            "",
            "compiler.rs",
        );
        insert(
            &mut modules,
            Package::Compiler,
            &["checking", "leaf"],
            "",
            "compiler.rs",
        );
        add_inline_modules(&mut modules);
        SourceTree { modules }
    }

    fn add_inline_modules(modules: &mut BTreeMap<ModuleId, ModuleSource>) {
        let sources = modules
            .values()
            .map(|source| {
                (
                    source.id.clone(),
                    source.file.clone(),
                    source.syntax.items.clone(),
                )
            })
            .collect::<Vec<_>>();
        sources.iter().for_each(|(parent, file, items)| {
            add_inline_from_items(modules, parent, file, items);
        });
    }

    fn add_inline_from_items(
        modules: &mut BTreeMap<ModuleId, ModuleSource>,
        parent: &ModuleId,
        file: &PathBuf,
        items: &[syn::Item],
    ) {
        items.iter().for_each(|item| {
            let syn::Item::Mod(module) = item else { return };
            let Some((_, content)) = &module.content else {
                return;
            };
            let mut path = parent.path.clone();
            path.push(module.ident.to_string());
            let id = ModuleId::new(parent.package.clone(), path);
            if modules.contains_key(&id) {
                return;
            }
            modules.insert(
                id.clone(),
                ModuleSource {
                    id: id.clone(),
                    file: file.clone(),
                    syntax: syn::File {
                        shebang: None,
                        frontmatter: None,
                        attrs: Vec::new(),
                        items: content.clone(),
                    },
                },
            );
            add_inline_from_items(modules, &id, file, content);
        });
    }

    fn insert(
        modules: &mut BTreeMap<ModuleId, ModuleSource>,
        package: Package,
        path: &[&str],
        text: &str,
        file: &str,
    ) {
        let id = ModuleId::new(
            package,
            path.iter().map(|part| (*part).to_owned()).collect(),
        );
        modules.insert(
            id.clone(),
            ModuleSource {
                id,
                file: PathBuf::from(file),
                syntax: syn::parse_file(text).expect("fixture parses"),
            },
        );
    }

    fn keys(
        root: &str,
        extra: &[(&[&str], &str)],
    ) -> BTreeMap<crate::model::EdgeKey, crate::model::Edge> {
        analyze(&tree(root, extra))
            .expect("analysis succeeds")
            .edges
    }

    fn id(path: &[&str]) -> ModuleId {
        ModuleId::new(
            Package::Eval,
            path.iter().map(|part| (*part).to_owned()).collect(),
        )
    }

    #[test]
    fn direct_paths_have_exact_producer_edges() {
        let edges = keys(
            "mod producer { pub struct Item; } mod consumer { fn f() { let _: crate::producer::Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false,
        }));
        assert_eq!(edges.len(), 3, "root->children plus consumer->producer");
    }

    #[test]
    fn aliases_keep_item_segments_and_are_order_independent() {
        let edges = keys(
            "mod producer { pub struct Item; } mod consumer { use alias::Item as Local; use crate::producer as alias; fn f() { let _: Local; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false,
        }));
    }

    #[test]
    fn reexport_laundering_still_points_consumers_at_the_producer() {
        let edges = keys(
            "mod producer { pub struct Item; } mod bridge { pub use crate::producer::Item as ExportedItem; } mod consumer { fn f() { let _: crate::bridge::ExportedItem; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["bridge"]),
            to: id(&["producer"]),
            test_only: false,
        }));
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false,
        }));
    }

    #[test]
    fn glob_reexport_chains_and_bare_imports_reach_the_producer() {
        let edges = keys(
            "mod producer { pub struct Item; } mod bridge { pub use crate::producer::Item; } mod bridge2 { pub use crate::bridge::*; } mod consumer { use crate::bridge2::*; fn f() { let _: Item; } }",
            &[],
        );
        let target = crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false,
        };
        assert!(edges.contains_key(&target));
        assert_eq!(
            edges[&target]
                .evidence
                .iter()
                .filter(|e| e.form == EvidenceForm::QualifiedPath)
                .count(),
            1
        );
    }

    #[test]
    fn explicit_import_shadows_a_glob_import() {
        let edges = keys(
            "mod first { pub struct Item; } mod second { pub struct Item; } mod consumer { use crate::second::*; use crate::first::Item as Item; fn f() { let _: Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["first"]),
            test_only: false,
        }));
        assert!(!edges.values().any(|edge| {
            edge.key.from == id(&["consumer"])
                && edge.key.to == id(&["second"])
                && edge
                    .evidence
                    .iter()
                    .any(|e| e.form == EvidenceForm::QualifiedPath)
        }));
    }

    #[test]
    fn cfg_is_structural_and_out_of_line_test_context_is_propagated() {
        let root = r#"
            mod producer { pub struct Item; }
            #[cfg(not(test))] mod production { use crate::producer::Item; }
            #[cfg(any(test, feature = "x"))] mod any_branch { use crate::producer::Item; }
            #[cfg(all(test, feature = "x"))] mod all_branch { use crate::producer::Item; }
            #[cfg(not(not(test)))] mod nested_not { use crate::producer::Item; }
            #[cfg(test)] mod out;
        "#;
        let edges = keys(root, &[(&["out"], "use crate::producer::Item;")]);
        for (module, test_only) in [
            ("production", false),
            ("any_branch", false),
            ("all_branch", true),
            ("nested_not", true),
            ("out", true),
        ] {
            assert!(
                edges.contains_key(&crate::model::EdgeKey {
                    from: id(&[module]),
                    to: id(&["producer"]),
                    test_only,
                }),
                "missing {module} edge ({test_only})"
            );
        }
        assert!(!edges.contains_key(&crate::model::EdgeKey {
            from: id(&["any_branch"]),
            to: id(&["producer"]),
            test_only: true,
        }));
    }

    #[test]
    fn repeated_super_and_external_crate_aliases_are_resolved() {
        let root = r#"
            extern crate graphcal_compiler as gc;
            mod producer { pub struct Item; }
            mod outer { mod inner { use super::super::producer::Item; fn f() { let _: Item; } } }
            mod external { use super::gc::checking::leaf::Marker; fn f() { let _: Marker; } }
        "#;
        let edges = keys(
            root,
            &[
                (
                    &["outer", "inner"],
                    "use super::super::producer::Item; fn f() { let _: Item; }",
                ),
                (
                    &["external"],
                    "use super::gc::checking::leaf::Marker; fn f() { let _: Marker; }",
                ),
            ],
        );
        assert!(edges.keys().any(|key| {
            key.from == id(&["outer", "inner"]) && key.to == id(&["producer"]) && !key.test_only
        }));
        assert!(edges.keys().any(|key| {
            key.from == id(&["external"])
                && key.to
                    == ModuleId::new(Package::Compiler, vec!["checking".into(), "leaf".into()])
        }));
    }

    #[test]
    fn crate_macro_paths_are_scanned() {
        let edges = keys(
            "mod producer { pub struct Item; } macro_rules! generated { () => { $crate::producer::Item }; } fn f() { generated!(); }",
            &[],
        );
        assert!(edges.values().any(|edge| {
            edge.key.to == id(&["producer"])
                && edge
                    .evidence
                    .iter()
                    .any(|e| e.form == EvidenceForm::MacroTokenPath)
        }));
    }

    #[test]
    fn unsupported_source_mapping_fails_closed() {
        let path = tree("#[path = \"elsewhere.rs\"] mod hidden;", &[]);
        assert!(analyze(&path).is_err());
        let include = tree("include!(\"generated.rs\");", &[]);
        assert!(analyze(&include).is_err());
        let generated_include = tree(
            "macro_rules! generated { () => { include!(\"generated.rs\") }; }",
            &[],
        );
        assert!(analyze(&generated_include).is_err());
    }

    #[test]
    fn direct_glob_reexports_resolve_declared_items() {
        let edges = keys(
            "mod producer { pub struct Item; } mod bridge { pub use crate::producer::*; } mod consumer { fn f() { let _: crate::bridge::Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false
        }));
    }

    #[test]
    fn relative_reexports_keep_their_defining_scope_through_globs() {
        let edges = keys(
            "mod producer { pub struct Item; } mod outer { pub mod bridge { pub use super::super::producer::Item; } } mod bridge2 { pub use crate::outer::bridge::*; } mod consumer { fn f() { let _: crate::bridge2::Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false
        }));
    }

    #[test]
    fn multiple_cfg_attributes_are_conjoined() {
        let edges = keys(
            "mod producer { pub struct Item; } #[cfg(test)] #[cfg(feature = \"x\")] mod consumer { use crate::producer::Item; }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: true
        }));
        assert!(!edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false
        }));
    }

    #[test]
    fn cfg_alias_alternatives_preserve_production_and_test_edges() {
        let edges = keys(
            r#"
                mod production { pub struct Item; }
                #[cfg(test)] mod testing { pub struct Item; }
                mod consumer {
                    #[cfg(feature = "production")] use crate::production::Item as Item;
                    #[cfg(test)] use crate::testing::Item as Item;
                    fn f() { let _: Item; }
                }
            "#,
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["production"]),
            test_only: false,
        }));
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["testing"]),
            test_only: true,
        }));
    }

    #[test]
    fn feature_alias_alternatives_do_not_launder_a_target() {
        let edges = keys(
            r#"
                mod first { pub struct Item; }
                mod second { pub struct Item; }
                mod consumer {
                    #[cfg(feature = "first")] use crate::first::Item as Item;
                    #[cfg(feature = "second")] use crate::second::Item as Item;
                    fn f() { let _: Item; }
                }
            "#,
            &[],
        );
        for module in ["first", "second"] {
            assert!(edges.contains_key(&crate::model::EdgeKey {
                from: id(&["consumer"]),
                to: id(&[module]),
                test_only: false,
            }));
        }
    }

    #[test]
    fn lexical_imports_resolve_without_mutating_module_scope() {
        let edges = keys(
            "mod producer { pub struct Item; } mod bridge { pub use crate::producer::Item; } mod consumer { fn f() { use b::Item as Local; use crate::bridge as b; let _: Local; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["bridge"]),
            test_only: false,
        }));
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false,
        }));
    }

    #[test]
    fn consumer_keeps_reexport_boundary_as_well_as_final_producer() {
        let edges = keys(
            "mod producer { pub struct Item; } mod bridge { pub use crate::producer::Item; } mod consumer { fn f() { let _: crate::bridge::Item; } }",
            &[],
        );
        for target in ["bridge", "producer"] {
            assert!(
                edges.contains_key(&crate::model::EdgeKey {
                    from: id(&["consumer"]),
                    to: id(&[target]),
                    test_only: false
                }),
                "missing consumer -> {target}"
            );
        }
    }

    #[test]
    fn conditional_reexports_are_resolved_at_an_independent_consumer() {
        let edges = keys(
            r#"mod production { pub struct Item; } mod testing { pub struct Item; } mod bridge { #[cfg(not(test))] pub use crate::production::Item as Selected; #[cfg(test)] pub use crate::testing::Item as Selected; } mod consumer { fn f() { let _: crate::bridge::Selected; } }"#,
            &[],
        );
        for (target, test_only) in [("production", false), ("testing", true)] {
            assert!(edges.contains_key(&crate::model::EdgeKey {
                from: id(&["consumer"]),
                to: id(&[target]),
                test_only
            }));
        }
        assert!(!edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["testing"]),
            test_only: false
        }));
    }

    #[test]
    fn glob_lookup_keeps_export_names_and_does_not_select_unrelated_items() {
        let edges = keys(
            "mod producer { pub struct Item; } mod unrelated { pub struct Other; } mod bridge { pub use crate::producer::Item; pub use crate::unrelated::Other; } mod bridge2 { pub use crate::bridge::*; } mod consumer { fn f() { let _: crate::bridge2::Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false
        }));
        assert!(!edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["unrelated"]),
            test_only: false
        }));
    }

    #[test]
    fn qualified_module_alias_does_not_capture_lexical_shadowing() {
        let edges = keys(
            "mod producer { pub struct Item; } mod safe { pub struct Item; } mod bridge { pub use crate::producer::Item; } mod consumer { use crate::bridge as global; use global as exported; fn f() { use crate::safe as global; let _: crate::consumer::exported::Item; } }",
            &[],
        );
        assert!(edges.contains_key(&crate::model::EdgeKey {
            from: id(&["consumer"]),
            to: id(&["producer"]),
            test_only: false
        }));
    }

    #[test]
    fn empty_cfg_boolean_operators_keep_their_identity_values() {
        let edges = keys(
            "mod producer { pub struct Item; } #[cfg(not(any()))] mod enabled { use crate::producer::Item; } #[cfg(not(all()))] mod disabled { use crate::producer::Item; }",
            &[],
        );
        for (name, test_only) in [("enabled", false), ("disabled", true)] {
            assert!(edges.contains_key(&crate::model::EdgeKey {
                from: id(&[name]),
                to: id(&["producer"]),
                test_only
            }));
        }
    }

    #[test]
    fn checked_in_layer_fixtures_have_opposite_policy_outcomes() {
        let roles = BTreeMap::from([
            (id(&[]), Role::Facade),
            (id(&["contracts"]), Role::Contracts),
            (id(&["checking"]), Role::Checking),
            (id(&["interpreter"]), Role::Interpreter),
            (id(&["facade"]), Role::Facade),
        ]);
        let positive = keys(include_str!("../fixtures/positive.rs"), &[]);
        assert!(!positive.is_empty());
        assert!(
            positive
                .keys()
                .all(|edge| !forbidden(roles[&edge.from], roles[&edge.to]))
        );
        let negative = keys(include_str!("../fixtures/negative.rs"), &[]);
        let violations = negative
            .keys()
            .filter(|edge| forbidden(roles[&edge.from], roles[&edge.to]))
            .collect::<Vec<_>>();
        for target in ["checking", "facade"] {
            assert!(
                violations
                    .iter()
                    .any(|edge| edge.from == id(&["interpreter"]) && edge.to == id(&[target]))
            );
        }
    }

    #[test]
    fn visibility_scopes_are_not_implementation_dependencies() {
        let edges = keys(
            "mod contracts { pub(crate) struct Token; mod nested { pub(in crate::contracts) struct Other; } }",
            &[],
        );
        assert!(
            !edges
                .keys()
                .any(|edge| edge.from == id(&["contracts"]) && edge.to == id(&[]))
        );
        assert!(
            !edges
                .keys()
                .any(|edge| edge.from == id(&["contracts", "nested"])
                    && edge.to == id(&["contracts"]))
        );
    }

    #[test]
    fn forbidden_matrix_keeps_shell_boundaries_strict() {
        assert!(forbidden(Role::Contracts, Role::Checking));
        assert!(forbidden(Role::Contracts, Role::Interpreter));
        assert!(forbidden(Role::Contracts, Role::Loading));
        assert!(forbidden(Role::Contracts, Role::Facade));
        assert!(forbidden(Role::Checking, Role::Loading));
        assert!(!forbidden(Role::Interpreter, Role::Contracts));
        assert!(!forbidden(Role::Checking, Role::Contracts));
    }
}
