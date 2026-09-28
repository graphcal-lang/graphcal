use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::syntax::ast::{ImportKind, ModulePath, UnitConstness};
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, StructTypeName};

use super::ModuleResolver;
use super::category::*;
use super::error::*;
use super::exports::*;
use super::symbols::*;
use crate::syntax::ast::Ident;
use crate::syntax::parser::Parser;

fn desugared_source(source: &str) -> ast::File {
    let raw = Parser::new(source).parse_file().unwrap();
    crate::desugar::desugared_ast::File::from(raw)
}

fn first_import(file: &ast::File) -> &ast::ImportDecl {
    file.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Import(import) => Some(import),
            _ => None,
        })
        .expect("source should contain an import")
}

fn imports(file: &ast::File) -> Vec<&ast::ImportDecl> {
    file.declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            ast::DeclKind::Import(import) => Some(import),
            _ => None,
        })
        .collect()
}

fn first_include(file: &ast::File) -> (&ModulePath, &ImportKind) {
    file.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Include(include) => Some((&include.path, &include.kind)),
            _ => None,
        })
        .expect("source should contain an include")
}

fn atom(s: &str) -> NameAtom {
    NameAtom::parse(s).unwrap()
}

fn path(segments: &[&str]) -> NamePath {
    let (leaf, owner) = segments.split_last().unwrap();
    NamePath::from_parts(
        NonEmpty::try_from_vec(owner.iter().map(|s| atom(s)).collect()).ok(),
        atom(leaf),
    )
}

fn module_path(segments: &[&str]) -> ModulePath {
    let idents = segments
        .iter()
        .map(|s| Ident {
            name: crate::syntax::token::SourceIdentifier::parse(*s).unwrap(),
            span: Span::new(0, 0),
        })
        .collect::<Vec<_>>();
    ModulePath {
        segments: NonEmpty::try_from_vec(idents).unwrap(),
        span: Span::new(0, 0),
    }
}

fn first_dag(file: &ast::File) -> &ast::DagDecl {
    file.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Dag(dag) => Some(dag),
            _ => None,
        })
        .expect("source should contain a dag")
}

#[test]
fn local_type_index_name_collision_is_rejected() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("type M { Mk(v: Dimensionless) }\npub index M = { A, B };");

    let err = ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::DuplicateSymbol {
            owner: err_owner,
            namespace: "Static",
            name,
            ..
        } if err_owner == owner && name == "M"
    ));
}

#[test]
fn local_dimension_type_name_collision_is_rejected() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("dim M = Length;\ntype M { Mk(v: Dimensionless) }");

    let err = ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::DuplicateSymbol {
            owner: err_owner,
            namespace: "Static",
            name,
            ..
        } if err_owner == owner && name == "M"
    ));
}

#[test]
fn aliases_of_same_index_preserve_label_identity() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index Phase = { Burn, Coast };");
    let main = desugared_source("import lib::{ index Phase, index Phase as P };");
    let imports = imports(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, imports[0], &lib_id)
        .unwrap();

    let direct = resolver
        .resolve_index_variant_parts(
            &main_id,
            &path(&["Phase"]),
            &IndexVariantName::expect_valid("Burn"),
        )
        .unwrap();
    let alias = resolver
        .resolve_index_variant_parts(
            &main_id,
            &path(&["P"]),
            &IndexVariantName::expect_valid("Burn"),
        )
        .unwrap();
    assert_eq!(direct, alias);
}

#[test]
fn same_named_type_and_constructor_remain_distinct() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source("type T { T }");

    let symbols = ModuleSymbols::from_declarations(owner, &file.declarations).unwrap();

    assert!(
        symbols
            .struct_types()
            .contains_key(&StructTypeName::expect_valid("T"))
    );
    assert!(
        symbols
            .constructors()
            .contains_key(&ConstructorName::expect_valid("T"))
    );
}

#[test]
fn exported_surface_uses_canonical_import_item_spellings() {
    let owner = DagId::root_in_package("test", "main");
    let file = desugared_source(
        "pub const node JPY: Dimensionless = 1.0;\n\
         pub base unit JPY: Dimensionless;\n\
         pub type Student { Student }\n\
         pub dim Information = Dimensionless;\n\
         pub index Category = { A };",
    );
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(owner.clone(), &file.declarations)
        .unwrap();

    let rendered = resolver
        .exported_import_items(&owner)
        .unwrap()
        .iter()
        .map(ExportedImportItem::render)
        .collect::<Vec<_>>();
    assert_eq!(
        rendered,
        [
            "JPY",
            "Student",
            "type Student",
            "dim Information",
            "unit JPY",
            "index Category",
        ]
    );
}

#[test]
fn local_value_constructor_name_collision_is_rejected_in_either_order() {
    let owner = DagId::root_in_package("test", "main");
    for source in [
        "type Choice { Red }\nconst node Red: Dimensionless = 1.0;",
        "const node Red: Dimensionless = 1.0;\ntype Choice { Red }",
    ] {
        let file = desugared_source(source);
        let err = ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateSymbol {
                owner: err_owner,
                namespace: "Term",
                name,
                ..
            } if err_owner == owner && name == "Red"
        ));
    }
}

#[test]
fn unit_import_colliding_with_local_unit_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub base unit m: Dimensionless;");
    let main = desugared_source("base unit m: Dimensionless;\nimport lib::{ unit m };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_import(&main_id, import, &lib_id)
        .unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: "UnitName",
            name,
            ..
        } if owner == main_id && name == "m"
    ));
}

#[test]
fn constructor_import_colliding_with_local_constructor_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type Foreign { Mk }");
    let main = desugared_source("type Local { Mk }\nimport lib::{ Mk };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_import(&main_id, import, &lib_id)
        .unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: "ConstructorName",
            name,
            ..
        } if owner == main_id && name == "Mk"
    ));
}

#[test]
fn imported_value_constructor_collisions_are_rejected_in_either_direction() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");

    for (lib_source, main_source) in [
        (
            "pub type Foreign { Red }",
            "const node Red: Dimensionless = 1.0;\nimport lib::{ Red };",
        ),
        (
            "pub const node Red: Dimensionless = 1.0;",
            "type Local { Red }\nimport lib::{ Red };",
        ),
    ] {
        let lib = desugared_source(lib_source);
        let main = desugared_source(main_source);
        let import = first_import(&main);
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import, &lib_id)
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateImportName {
                owner,
                namespace: "Term",
                name,
                ..
            } if owner == main_id && name == "Red"
        ));
    }
}

#[test]
fn equal_labels_under_different_indexes_resolve_by_explicit_owner() {
    let a_id = DagId::root_in_package("test", "a");
    let z_id = DagId::root_in_package("test", "z");
    let main_id = DagId::root_in_package("test", "main");
    let a = desugared_source("pub index AIndex = { Shared };");
    let z = desugared_source("pub index ZIndex = { Shared };");
    let main = desugared_source("import z::{ index ZIndex };\nimport a::{ index AIndex };");
    let imports = imports(&main);

    let mut resolver = ModuleResolver::default();
    resolver.add_module(z_id.clone(), &z.declarations).unwrap();
    resolver.add_module(a_id.clone(), &a.declarations).unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, imports[0], &z_id)
        .unwrap();
    resolver
        .register_import(&main_id, imports[1], &a_id)
        .unwrap();

    let a_label = resolver
        .resolve_index_variant_parts(
            &main_id,
            &path(&["AIndex"]),
            &IndexVariantName::expect_valid("Shared"),
        )
        .unwrap();
    let z_label = resolver
        .resolve_index_variant_parts(
            &main_id,
            &path(&["ZIndex"]),
            &IndexVariantName::expect_valid("Shared"),
        )
        .unwrap();
    assert_ne!(a_label.index(), z_label.index());
}

#[test]
fn selective_import_cross_universe_name_collision_is_rejected() {
    let type_lib_id = DagId::root_in_package("test", "type_lib");
    let index_lib_id = DagId::root_in_package("test", "index_lib");
    let main_id = DagId::root_in_package("test", "main");
    let type_lib = desugared_source("pub type M { Mk(v: Dimensionless) }");
    let index_lib = desugared_source("pub index M = { A, B };");
    let main = desugared_source(
        "import type_lib::{ type M };
         import index_lib::{ index M };",
    );
    let imports = imports(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(type_lib_id.clone(), &type_lib.declarations)
        .unwrap();
    resolver
        .add_module(index_lib_id.clone(), &index_lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, imports[0], &type_lib_id)
        .unwrap();
    let err = resolver
        .register_import(&main_id, imports[1], &index_lib_id)
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: "Static",
            name,
            ..
        } if owner == main_id && name == "M"
    ));
}

#[test]
fn resolves_qualified_index_variant_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index Phase = { Burn, Coast };");
    let main = desugared_source("import lib as physics;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let resolved_name = resolver
        .resolve_index_variant_parts(
            &main_id,
            &path(&["physics", "Phase"]),
            &IndexVariantName::expect_valid("Burn"),
        )
        .unwrap();

    assert_eq!(resolved_name.index().owner(), &lib_id);
    assert_eq!(resolved_name.index().as_str(), "Phase");
    assert_eq!(resolved_name.variant().as_str(), "Burn");
}

#[test]
fn selective_type_alias_resolves_to_original_owner_and_leaf() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type Vec3 { Vec3 }");
    let main = desugared_source("import lib::{ type Vec3 as Vector };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let resolved_name = resolver
        .resolve_struct_type_path(&main_id, &path(&["Vector"]))
        .unwrap();

    assert_eq!(resolved_name.owner(), &lib_id);
    assert_eq!(resolved_name.as_str(), "Vec3");
}

#[test]
fn type_import_in_child_dag_does_not_import_same_named_constructor() {
    let main_id = DagId::root_in_package("test", "main");
    let child_id = main_id.child("build_transfer");
    let main = desugared_source(
        "pub type TransferResult { TransferResult }
         dag build_transfer {
             import main::{ type TransferResult };
         }",
    );
    let dag = first_dag(&main);
    let import = dag
        .body
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Import(import) => Some(import),
            _ => None,
        })
        .expect("dag body should contain an import");

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.add_module(child_id.clone(), &dag.body).unwrap();
    resolver
        .register_import(&child_id, import, &main_id)
        .unwrap();

    let resolved_type = resolver
        .resolve_struct_type_path(&child_id, &path(&["TransferResult"]))
        .unwrap();
    assert_eq!(resolved_type.owner(), &main_id);
    assert_eq!(resolved_type.as_str(), "TransferResult");

    let err = resolver
        .resolve_constructor_path(&child_id, &path(&["TransferResult"]))
        .unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::UnknownName {
            owner,
            namespace: "ConstructorName",
            name,
        } if owner == child_id && name == "TransferResult"
    ));
}

#[test]
fn type_marker_importing_index_reports_wrong_universe() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index M = { A };");
    let main = desugared_source("import lib::{ type M };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_import(&main_id, import, &lib_id)
        .unwrap_err();
    assert!(err.to_string().contains("did you mean `index M`?"));

    assert!(matches!(
        err,
        ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            ..
        } if owner == lib_id
            && mismatch.name().as_str() == "M"
            && mismatch.expected() == ImportItemNamespace::Type
            && mismatch.alternatives().as_slice() == [ImportItemNamespace::Index]
    ));
}

#[test]
fn bare_importing_type_reports_wrong_category() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type Foo { MkFoo }");
    let main = desugared_source("import lib::{ Foo };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_import(&main_id, import, &lib_id)
        .unwrap_err();
    assert!(err.to_string().contains("did you mean `type Foo`?"));

    assert!(matches!(
        err,
        ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            ..
        } if owner == lib_id
            && mismatch.name().as_str() == "Foo"
            && mismatch.expected() == ImportItemNamespace::Term
            && mismatch.alternatives().as_slice() == [ImportItemNamespace::Type]
    ));
}

#[test]
fn wrong_import_category_lists_all_legal_same_name_alternatives() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "pub const node JPY: Dimensionless = 1.0;\n\
         pub base unit JPY: Dimensionless;",
    );
    let main = desugared_source("import lib::{ dim JPY };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_import(&main_id, import, &lib_id)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("did you mean `JPY` or `unit JPY`?")
    );
    assert!(matches!(
        err,
        ModuleResolveError::WrongImportCategory { mismatch, .. }
            if mismatch.alternatives().as_slice()
                == [ImportItemNamespace::Term, ImportItemNamespace::Unit]
    ));
}

#[test]
fn qualified_private_type_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("type Secret { Secret }");
    let main = desugared_source("import lib as hidden;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let err = resolver
        .resolve_struct_type_path(&main_id, &path(&["hidden", "Secret"]))
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            namespace: "StructTypeName",
            name,
        } if owner == lib_id && name == "Secret"
    ));
}

#[test]
fn include_selective_private_decl_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("node hidden: Dimensionless = 1.0;");
    let main = desugared_source("include lib()::{ hidden };");
    let (include_path, include_kind) = first_include(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let err = resolver
        .register_include(&main_id, include_path, include_kind, &lib_id)
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            namespace: _,
            name,
        } if owner == lib_id && name == "hidden"
    ));
}

#[test]
fn include_projection_classification_is_shared_and_exhaustive() {
    let cases = [
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Const),
            Some(IncludeProjection::ConstNode),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Param),
            Some(IncludeProjection::RuntimeTerm),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Node),
            Some(IncludeProjection::RuntimeTerm),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Assert),
            Some(IncludeProjection::Assertion),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Plot),
            Some(IncludeProjection::Visualization),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Figure),
            Some(IncludeProjection::Visualization),
        ),
        (
            ExportedImportItemKind::Decl(DeclSymbolKind::Layer),
            Some(IncludeProjection::Visualization),
        ),
        (ExportedImportItemKind::Decl(DeclSymbolKind::Dag), None),
        (
            ExportedImportItemKind::Constructor,
            Some(IncludeProjection::Constructor),
        ),
        (
            ExportedImportItemKind::Type,
            Some(IncludeProjection::StaticDeclaration),
        ),
        (
            ExportedImportItemKind::Dimension,
            Some(IncludeProjection::StaticDeclaration),
        ),
        (
            ExportedImportItemKind::Index,
            Some(IncludeProjection::StaticDeclaration),
        ),
        (
            ExportedImportItemKind::Unit(UnitConstness::Const),
            Some(IncludeProjection::StaticUnit),
        ),
        (
            ExportedImportItemKind::Unit(UnitConstness::Dynamic),
            Some(IncludeProjection::RuntimeUnit),
        ),
    ];

    for (kind, expected) in cases {
        assert_eq!(include_projection(kind), expected);
    }
}

#[test]
fn selective_include_rejects_dag_projection_at_selector() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let instance_id = main_id.named_instance_child("projection");
    let lib = desugared_source("pub dag child { pub node output: Dimensionless = 1.0; }");
    let main = desugared_source("include lib()::{ child };");
    let (include_path, include_kind) = first_include(&main);
    let ImportKind::Selective(items) = include_kind else {
        panic!("expected selective include")
    };

    let mut resolver = ModuleResolver::default();
    resolver.add_module(lib_id, &lib.declarations).unwrap();
    resolver
        .add_module(instance_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    let error = resolver
        .register_include(&main_id, include_path, include_kind, &instance_id)
        .unwrap_err();
    assert!(matches!(
        error,
        ModuleResolveError::IncludeItemNotProjectable {
            name,
            kind: ExportedImportItemKind::Decl(DeclSymbolKind::Dag),
            span,
            ..
        } if name == "child" && span == items[0].name.span
    ));
}

#[test]
fn selective_include_constructor_keeps_source_canonical_identity() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let instance_id = main_id.named_instance_child("projection");
    let lib = desugared_source("pub type Choice { Pick }");
    let main = desugared_source("include lib()::{ Pick as Selected };");
    let include = main
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.kind {
            ast::DeclKind::Include(include) => Some(include),
            _ => None,
        })
        .expect("include declaration");

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(instance_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_include(&main_id, &include.path, &include.kind, &instance_id)
        .unwrap();
    resolver
        .apply_include_static_projection_bindings(&main_id, Some(&lib_id), include)
        .unwrap();

    let constructor_target = resolver
        .resolve_constructor_path(&main_id, &path(&["Selected"]))
        .unwrap();
    assert_eq!(constructor_target.owner(), &lib_id);
    assert_eq!(constructor_target.as_str(), "Pick");
}

#[test]
fn selective_include_rejects_constructor_when_owner_type_is_rebound() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let instance_id = main_id.named_instance_child("projection");
    let lib = desugared_source("pub(bind) type Choice { Pick }");
    let main = desugared_source(
        "type Replacement { Replacement }
         include lib(type Choice: Replacement)::{ Pick };",
    );
    let include = main
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.kind {
            ast::DeclKind::Include(include) => Some(include),
            _ => None,
        })
        .expect("include declaration");
    let ImportKind::Selective(items) = &include.kind else {
        panic!("expected selective include")
    };

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(instance_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_include(&main_id, &include.path, &include.kind, &instance_id)
        .unwrap();

    let error = resolver
        .apply_include_static_projection_bindings(&main_id, Some(&lib_id), include)
        .unwrap_err();
    assert!(matches!(
        error,
        ModuleResolveError::ConstructorOwnerRebound {
            constructor,
            owner_type,
            span,
            ..
        } if constructor == "Pick"
            && owner_type.owner() == &lib_id
            && owner_type.as_str() == "Choice"
            && span == items[0].name.span
    ));
}

#[test]
fn loaded_sibling_file_is_not_callable_without_an_import() {
    let main_id = DagId::new("test", NonEmpty::new("src", vec!["pkg", "main"]));
    let sibling_id = DagId::new("test", NonEmpty::new("src", vec!["pkg", "library"]));
    let empty = desugared_source("");
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(main_id.clone(), &empty.declarations)
        .unwrap();
    resolver
        .add_module(sibling_id, &empty.declarations)
        .unwrap();

    assert!(matches!(
        resolver.resolve_module_path(&main_id, &module_path(&["library"])),
        Err(ModuleResolveError::UnknownModule { .. })
    ));
}

#[test]
fn imported_file_module_alias_is_callable_as_its_exact_target() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub node result: Dimensionless = 1.0;");
    let main = desugared_source("import lib;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    assert_eq!(
        resolver
            .resolve_module_path(&main_id, &module_path(&["lib"]))
            .unwrap(),
        lib_id
    );
}

#[test]
fn imported_inline_dag_alias_is_callable_as_its_exact_target() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "pub dag helper {
            pub node result: Dimensionless = 1.0;
        }",
    );
    let helper = first_dag(&lib);
    let main = desugared_source("import lib.helper as imported;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver.add_module(lib_id, &lib.declarations).unwrap();
    resolver
        .add_module(helper_id.clone(), &helper.body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, import, &helper_id)
        .unwrap();

    assert_eq!(
        resolver
            .resolve_module_path(&main_id, &module_path(&["imported"]))
            .unwrap(),
        helper_id
    );
}

#[test]
fn direct_alias_of_private_inline_dag_rejects_modules_and_every_symbol_namespace() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node result: Dimensionless = 1.0;
            pub base dim Distance;
            pub base unit tick: Dimensionless;
            pub type Shape { Shape }
            pub index Axis = { A };
        }",
    );
    let helper = first_dag(&lib);
    let main = desugared_source("import lib.helper as imported;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(helper_id.clone(), &helper.body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, import, &helper_id)
        .unwrap();

    let errors = [
        resolver
            .resolve_module_path(&main_id, &module_path(&["imported"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_decl_path(&main_id, &path(&["imported", "result"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_dimension_path(&main_id, &path(&["imported", "Distance"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_unit_path(&main_id, &path(&["imported", "tick"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_struct_type_path(&main_id, &path(&["imported", "Shape"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_constructor_path(&main_id, &path(&["imported", "Shape"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_index_path(&main_id, &path(&["imported", "Axis"]))
            .map(|_| ())
            .unwrap_err(),
    ];

    for error in errors {
        assert!(matches!(
            error,
            ModuleResolveError::PrivateName {
                owner,
                namespace: "dag",
                name,
            } if owner == lib_id && name == "helper"
        ));
    }
}

#[test]
fn selective_import_from_private_inline_dag_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node result: Dimensionless = 1.0;
        }",
    );
    let helper = first_dag(&lib);
    let main = desugared_source("import lib.helper::{ result };");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(helper_id.clone(), &helper.body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();

    assert!(matches!(
        resolver.register_import(&main_id, import, &helper_id),
        Err(ModuleResolveError::PrivateName {
            owner,
            namespace: "dag",
            name,
        }) if owner == lib_id && name == "helper"
    ));
}

#[test]
fn public_child_under_private_dag_cannot_be_an_import_tunnel() {
    let lib_id = DagId::root_in_package("test", "lib");
    let private_id = lib_id.child("private_parent");
    let child_id = private_id.child("public_child");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag private_parent {
            pub dag public_child {
                pub node result: Dimensionless = 1.0;
            }
        }",
    );
    let private_dag = first_dag(&lib);
    let private_body = ast::File {
        declarations: private_dag.body.clone(),
    };
    let public_child = first_dag(&private_body);
    let main = desugared_source("import lib.private_parent.public_child as child;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver.add_module(private_id, &private_dag.body).unwrap();
    resolver
        .add_module(child_id.clone(), &public_child.body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&main_id, import, &child_id)
        .unwrap();

    for error in [
        resolver
            .resolve_module_path(&main_id, &module_path(&["child"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_decl_path(&main_id, &path(&["child", "result"]))
            .map(|_| ())
            .unwrap_err(),
    ] {
        assert!(matches!(
            error,
            ModuleResolveError::PrivateName {
                owner,
                namespace: "dag",
                name,
            } if owner == lib_id && name == "private_parent"
        ));
    }
}

#[test]
fn selectively_imported_dag_alias_is_callable() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub dag helper { pub node result: Dimensionless = 1.0; }");
    let helper = first_dag(&lib);
    let main = desugared_source("import lib::{helper as imported};");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(helper_id.clone(), &helper.body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    assert_eq!(
        resolver
            .resolve_module_path(&main_id, &module_path(&["imported"]))
            .unwrap(),
        helper_id
    );
}

#[test]
fn local_inline_dag_can_qualify_its_nested_child() {
    let main_id = DagId::root_in_package("test", "main");
    let outer_id = main_id.child("outer");
    let inner_id = outer_id.child("inner");
    let main = desugared_source("dag outer { dag inner {} }");
    let outer = first_dag(&main);
    let inner = outer
        .body
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Dag(dag) => Some(dag),
            _ => None,
        })
        .expect("outer should contain inner");

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.add_module(outer_id, &outer.body).unwrap();
    resolver.add_module(inner_id.clone(), &inner.body).unwrap();

    assert_eq!(
        resolver
            .resolve_module_path(&main_id, &module_path(&["outer", "inner"]))
            .unwrap(),
        inner_id
    );
}

#[test]
fn included_instance_alias_is_not_callable() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub node result: Dimensionless = 1.0;");
    let main = desugared_source("include lib() as instance;");
    let (include_path, include_kind) = first_include(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_include(&main_id, include_path, include_kind, &lib_id)
        .unwrap();

    assert!(matches!(
        resolver.resolve_module_path(&main_id, &module_path(&["instance"])),
        Err(ModuleResolveError::IncludedInstanceNotCallable { alias, .. })
            if alias.as_str() == "instance"
    ));
}

#[test]
fn aliased_include_does_not_expose_same_named_file_module() {
    let main_id = DagId::root_in_package("test", "app");
    let defaults_id =
        DagId::from_relative_path("test", std::path::Path::new("app/defaults.gcl")).unwrap();
    let instance_id = main_id.named_instance_child("configured");
    let defaults = desugared_source("pub node result: Dimensionless = 1.0;");
    let main = desugared_source("include app.defaults() as configured;");
    let (include_path, include_kind) = first_include(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(defaults_id, &defaults.declarations)
        .unwrap();
    resolver
        .add_module(instance_id.clone(), &defaults.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_include(&main_id, include_path, include_kind, &instance_id)
        .unwrap();

    assert!(matches!(
        resolver.resolve_module_path(&main_id, &module_path(&["defaults"])),
        Err(ModuleResolveError::UnknownModule { .. })
    ));
    assert!(matches!(
        resolver.resolve_module_path(&main_id, &module_path(&["configured"])),
        Err(ModuleResolveError::IncludedInstanceNotCallable { alias, .. })
            if alias.as_str() == "configured"
    ));
}

#[test]
fn local_dag_and_imported_module_alias_collide_in_term_namespace() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let local_id = main_id.child("shared");
    let lib = desugared_source("pub node result: Dimensionless = 1.0;");
    let main = desugared_source("dag shared {} import lib as shared;");
    let local = first_dag(&main);
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.add_module(local_id, &local.body).unwrap();
    assert!(matches!(
        resolver.register_import(&main_id, import, &lib_id),
        Err(ModuleResolveError::DuplicateImportName {
            namespace: "Term",
            name,
            ..
        }) if name == "shared"
    ));
}

#[test]
fn local_and_selected_bindings_to_same_dag_are_one_callable() {
    let root_id = DagId::root_in_package("test", "self");
    let helper_id = root_id.child("helper");
    let calculation_id = root_id.child("calculation");
    let root = desugared_source(
        "pub dag helper {}
         dag calculation { import self::{ helper }; }",
    );
    let [helper, calculation] = root
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            ast::DeclKind::Dag(dag) => Some(dag),
            _ => None,
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("two DAG declarations");
    let import = calculation
        .body
        .iter()
        .find_map(|declaration| match &declaration.kind {
            ast::DeclKind::Import(import) => Some(import),
            _ => None,
        })
        .expect("calculation imports its parent");

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(root_id.clone(), &root.declarations)
        .unwrap();
    resolver
        .add_module(helper_id.clone(), &helper.body)
        .unwrap();
    resolver
        .add_module(calculation_id.clone(), &calculation.body)
        .unwrap();
    resolver
        .register_import(&calculation_id, import, &root_id)
        .unwrap();

    assert_eq!(
        resolver
            .resolve_module_path(&calculation_id, &module_path(&["helper"]))
            .unwrap(),
        helper_id
    );
}

#[test]
fn qualified_private_dag_path_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node shown: Dimensionless = 1.0;
        }",
    );
    let main = desugared_source("import lib as lib;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(helper_id, &first_dag(&lib).body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let err = resolver
        .resolve_module_path(&main_id, &module_path(&["lib", "helper"]))
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            namespace: "dag",
            name,
        } if owner == lib_id && name == "helper"
    ));
}

#[test]
fn qualified_symbol_path_through_private_dag_is_rejected() {
    // Regression: `resolve_symbol_path` resolved the qualifier without
    // the dag-visibility check that `resolve_module_path` enforces, so
    // `lib.helper.shown` resolved even though `helper` is a private dag.
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id = lib_id.child("helper");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node shown: Dimensionless = 1.0;
        }",
    );
    let main = desugared_source("import lib as lib;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(helper_id, &first_dag(&lib).body)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let err = resolver
        .resolve_decl_path(&main_id, &path(&["lib", "helper", "shown"]))
        .unwrap_err();

    assert!(
        matches!(
            err,
            ModuleResolveError::PrivateName {
                ref owner,
                namespace: "dag",
                ref name,
            } if *owner == lib_id && name == "helper"
        ),
        "expected PrivateName for dag `helper`, got: {err:?}"
    );
}

#[test]
fn qualified_constructor_resolves_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type BurnKind { Impulsive, Coast }");
    let main = desugared_source("import lib as mission;");
    let import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(lib_id.clone(), &lib.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver.register_import(&main_id, import, &lib_id).unwrap();

    let resolved_name = resolver
        .resolve_constructor_path(&main_id, &path(&["mission", "Impulsive"]))
        .unwrap();

    assert_eq!(resolved_name.owner(), &lib_id);
    assert_eq!(resolved_name.as_str(), "Impulsive");
}

#[test]
fn selective_pub_reexport_resolves_to_original_owner() {
    let leaf_id = DagId::root_in_package("test", "leaf");
    let middle_id = DagId::root_in_package("test", "middle");
    let main_id = DagId::root_in_package("test", "main");
    let leaf = desugared_source("pub dim Acceleration = Length / Time^2;");
    let middle = desugared_source("import leaf::{ pub dim Acceleration };");
    let main = desugared_source("import middle::{ dim Acceleration };");
    let middle_import = first_import(&middle);
    let main_import = first_import(&main);

    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(leaf_id.clone(), &leaf.declarations)
        .unwrap();
    resolver
        .add_module(middle_id.clone(), &middle.declarations)
        .unwrap();
    resolver
        .add_module(main_id.clone(), &main.declarations)
        .unwrap();
    resolver
        .register_import(&middle_id, middle_import, &leaf_id)
        .unwrap();
    resolver
        .register_import(&main_id, main_import, &middle_id)
        .unwrap();
    assert_eq!(
        resolver
            .exported_import_items(&middle_id)
            .unwrap()
            .iter()
            .map(ExportedImportItem::render)
            .collect::<Vec<_>>(),
        ["dim Acceleration"]
    );

    let resolved_name = resolver
        .resolve_dimension_path(&main_id, &path(&["Acceleration"]))
        .unwrap();

    assert_eq!(resolved_name.owner(), &leaf_id);
    assert_eq!(resolved_name.as_str(), "Acceleration");
}
