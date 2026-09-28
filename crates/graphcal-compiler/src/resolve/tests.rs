use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::syntax::ast::{ImportKind, ModulePath, UnitConstness};
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, StructTypeName};

use super::builder::{SymbolTables, TestModules};
use super::category::*;
use super::error::*;
use super::exports::*;
use super::namespace::Namespace;
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

fn first_include_decl(file: &ast::File) -> &ast::IncludeDecl {
    file.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            ast::DeclKind::Include(include) => Some(include),
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
            namespace: Namespace::Static,
            name,
            ..
        } if err_owner == owner && name.as_str() == "M"
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
            namespace: Namespace::Static,
            name,
            ..
        } if err_owner == owner && name.as_str() == "M"
    ));
}

#[test]
fn aliases_of_same_index_preserve_label_identity() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index Phase = { Burn, Coast };");
    let main = desugared_source("import lib::{ index Phase, index Phase as P };");
    let imports = imports(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, imports[0], &lib_id);
    let resolver = modules.build().unwrap();

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
            .constructors
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
    let mut modules = TestModules::default();
    modules.add(owner.clone(), &file.declarations);
    let resolver = modules.build().unwrap();

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
                namespace: Namespace::Term,
                name,
                ..
            } if err_owner == owner && name.as_str() == "Red"
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let err = modules.build().unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: Namespace::Unit,
            name,
            ..
        } if owner == main_id && name.as_str() == "m"
    ));
}

#[test]
fn constructor_import_colliding_with_local_constructor_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type Foreign { Mk }");
    let main = desugared_source("type Local { Mk }\nimport lib::{ Mk };");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let err = modules.build().unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: Namespace::Term,
            name,
            ..
        } if owner == main_id && name.as_str() == "Mk"
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
        let mut modules = TestModules::default();
        modules.add(lib_id.clone(), &lib.declarations);
        modules.add(main_id.clone(), &main.declarations);
        modules.import(&main_id, import, &lib_id);
        let err = modules.build().unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateImportName {
                owner,
                namespace: Namespace::Term,
                name,
                ..
            } if owner == main_id && name.as_str() == "Red"
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

    let mut modules = TestModules::default();
    modules.add(z_id.clone(), &z.declarations);
    modules.add(a_id.clone(), &a.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, imports[0], &z_id);
    modules.import(&main_id, imports[1], &a_id);
    let resolver = modules.build().unwrap();

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

    let mut modules = TestModules::default();
    modules.add(type_lib_id.clone(), &type_lib.declarations);
    modules.add(index_lib_id.clone(), &index_lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, imports[0], &type_lib_id);
    modules.import(&main_id, imports[1], &index_lib_id);
    let err = modules.build().unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::DuplicateImportName {
            owner,
            namespace: Namespace::Static,
            name,
            ..
        } if owner == main_id && name.as_str() == "M"
    ));
}

#[test]
fn resolves_qualified_index_variant_to_canonical_owner() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index Phase = { Burn, Coast };");
    let main = desugared_source("import lib as physics;");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

    let resolved_name = resolver
        .resolve_struct_type_path(&main_id, &path(&["Vector"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();

    assert_eq!(resolved_name.owner(), &lib_id);
    assert_eq!(resolved_name.as_str(), "Vec3");
}

#[test]
fn type_import_in_child_dag_does_not_import_same_named_constructor() {
    let main_id = DagId::root_in_package("test", "main");
    let child_id = main_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid(
        "build_transfer",
    ));
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

    let mut modules = TestModules::default();
    modules.add(main_id.clone(), &main.declarations);
    modules.add(child_id.clone(), &dag.body);
    modules.import(&child_id, import, &main_id);
    let resolver = modules.build().unwrap();

    let resolved_type = resolver
        .resolve_struct_type_path(&child_id, &path(&["TransferResult"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();
    assert_eq!(resolved_type.owner(), &main_id);
    assert_eq!(resolved_type.as_str(), "TransferResult");

    let err = resolver
        .resolve_constructor_path(&child_id, &path(&["TransferResult"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap_err();
    assert!(matches!(
        err,
        ModuleResolveError::UnknownName {
            owner,
            category: NameCategory::Table(SymbolTable::Constructor),
            name,
        } if owner == child_id && name.as_str() == "TransferResult"
    ));
}

#[test]
fn type_marker_importing_index_reports_wrong_universe() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub index M = { A };");
    let main = desugared_source("import lib::{ type M };");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let err = modules.build().unwrap_err();
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let err = modules.build().unwrap_err();
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let err = modules.build().unwrap_err();
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

    let err = resolver
        .resolve_struct_type_path(&main_id, &path(&["hidden", "Secret"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            category: NameCategory::Table(SymbolTable::StructType),
            name,
        } if owner == lib_id && name.as_str() == "Secret"
    ));
}

#[test]
fn include_selective_private_decl_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("node hidden: Dimensionless = 1.0;");
    let main = desugared_source("include lib()::{ hidden };");
    let include = first_include_decl(&main);
    let instance_id = main_id.instance_child(include.instance_scope());

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, &include.path, &lib_id);
    let err = modules.build().unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            category: _,
            name,
        } if owner == instance_id && name.as_str() == "hidden"
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
    let lib = desugared_source("pub dag child { pub node output: Dimensionless = 1.0; }");
    let main = desugared_source("include lib()::{ child };");
    let (include_path, include_kind) = first_include(&main);
    let ImportKind::Selective(items) = include_kind else {
        panic!("expected selective include")
    };

    let mut modules = TestModules::default();
    modules.add_file(&lib_id, &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, include_path, &lib_id);
    let error = modules.build().unwrap_err();
    assert!(matches!(
        error,
        ModuleResolveError::IncludeItemNotProjectable {
            name,
            kind: ExportedImportItemKind::Decl(DeclSymbolKind::Dag),
            span,
            ..
        } if name.as_str() == "child" && span == items[0].name.span
    ));
}

#[test]
fn selective_include_constructor_keeps_source_canonical_identity() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub type Choice { Pick }");
    let main = desugared_source("include lib()::{ Pick as Selected };");
    let include = first_include_decl(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, &include.path, &lib_id);
    let resolver = modules.build().unwrap();

    let constructor_target = resolver
        .resolve_constructor_path(&main_id, &path(&["Selected"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();
    assert_eq!(constructor_target.owner(), &lib_id);
    assert_eq!(constructor_target.as_str(), "Pick");
}

#[test]
fn selective_include_rejects_constructor_when_owner_type_is_rebound() {
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub(bind) type Choice { Pick }");
    let main = desugared_source(
        "type Replacement { Replacement }
         include lib(type Choice: Replacement)::{ Pick };",
    );
    let include = first_include_decl(&main);
    let ImportKind::Selective(items) = &include.kind else {
        panic!("expected selective include")
    };

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, &include.path, &lib_id);
    let error = modules.build().unwrap_err();
    assert!(matches!(
        error,
        ModuleResolveError::ConstructorOwnerRebound {
            constructor,
            owner_type,
            span,
            ..
        } if constructor.as_str() == "Pick"
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
    let mut modules = TestModules::default();
    modules.add(main_id.clone(), &empty.declarations);
    modules.add(sibling_id, &empty.declarations);
    let resolver = modules.build().unwrap();

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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

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
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "pub dag helper {
            pub node result: Dimensionless = 1.0;
        }",
    );
    let helper = first_dag(&lib);
    let main = desugared_source("import lib.helper as imported;");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id, &lib.declarations);
    modules.add(helper_id.clone(), &helper.body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &helper_id);
    let resolver = modules.build().unwrap();

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
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(helper_id.clone(), &helper.body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &helper_id);
    let resolver = modules.build().unwrap();

    let errors = [
        resolver
            .resolve_module_path(&main_id, &module_path(&["imported"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_decl_path(&main_id, &path(&["imported", "result"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_dimension_path(&main_id, &path(&["imported", "Distance"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_unit_path(&main_id, &path(&["imported", "tick"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_struct_type_path(&main_id, &path(&["imported", "Shape"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_constructor_path(&main_id, &path(&["imported", "Shape"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_index_path(&main_id, &path(&["imported", "Axis"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
    ];

    for error in errors {
        assert!(matches!(
            error,
            ModuleResolveError::PrivateName {
                owner,
                category: NameCategory::Dag,
                name,
            } if owner == lib_id && name.as_str() == "helper"
        ));
    }
}

#[test]
fn selective_import_from_private_inline_dag_is_rejected() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node result: Dimensionless = 1.0;
        }",
    );
    let helper = first_dag(&lib);
    let main = desugared_source("import lib.helper::{ result };");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(helper_id.clone(), &helper.body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &helper_id);

    assert!(matches!(
        modules.build(),
        Err(ModuleResolveError::PrivateName {
            owner,
            category: NameCategory::Dag,
            name,
        }) if owner == lib_id && name.as_str() == "helper"
    ));
}

#[test]
fn public_child_under_private_dag_cannot_be_an_import_tunnel() {
    let lib_id = DagId::root_in_package("test", "lib");
    let private_id = lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid(
        "private_parent",
    ));
    let child_id = private_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid(
        "public_child",
    ));
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(private_id, &private_dag.body);
    modules.add(child_id.clone(), &public_child.body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &child_id);
    let resolver = modules.build().unwrap();

    for error in [
        resolver
            .resolve_module_path(&main_id, &module_path(&["child"]))
            .map(|_| ())
            .unwrap_err(),
        resolver
            .resolve_decl_path(&main_id, &path(&["child", "result"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|_| ())
            .unwrap_err(),
    ] {
        assert!(matches!(
            error,
            ModuleResolveError::PrivateName {
                owner,
                category: NameCategory::Dag,
                name,
            } if owner == lib_id && name.as_str() == "private_parent"
        ));
    }
}

#[test]
fn selectively_imported_dag_alias_is_callable() {
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source("pub dag helper { pub node result: Dimensionless = 1.0; }");
    let helper = first_dag(&lib);
    let main = desugared_source("import lib::{helper as imported};");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(helper_id.clone(), &helper.body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

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
    let outer_id =
        main_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("outer"));
    let inner_id =
        outer_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("inner"));
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

    let mut modules = TestModules::default();
    modules.add(main_id.clone(), &main.declarations);
    modules.add(outer_id, &outer.body);
    modules.add(inner_id.clone(), &inner.body);
    let resolver = modules.build().unwrap();

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
    let (include_path, _) = first_include(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, include_path, &lib_id);
    let resolver = modules.build().unwrap();

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
    let defaults = desugared_source("pub node result: Dimensionless = 1.0;");
    let main = desugared_source("include app.defaults() as configured;");
    let (include_path, _) = first_include(&main);

    let mut modules = TestModules::default();
    modules.add(defaults_id.clone(), &defaults.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&main_id, include_path, &defaults_id);
    let resolver = modules.build().unwrap();

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
    let main_id = DagId::root_in_package("test", "main");
    let main = desugared_source("dag shared {} import lib as shared;");

    // The alias occupies the Term slot as soon as its declaration is
    // collected, before (and whether or not) the loader registers the edge.
    let result = SymbolTables::default().add_module(main_id, &main.declarations);
    assert!(matches!(
        result,
        Err(ModuleResolveError::DuplicateImportName {
            namespace: Namespace::Term,
            ref name,
            first,
            duplicate,
            ..
        }) if name.as_str() == "shared" && first.offset() < duplicate.offset()
    ));
}

/// Collect `source` as module `main`, returning the duplicate Term name, if any.
fn duplicate_term_alias(source: &str) -> Option<(String, Span, Span)> {
    let main = desugared_source(source);
    match SymbolTables::default()
        .add_module(DagId::root_in_package("test", "main"), &main.declarations)
    {
        Ok(()) => None,
        Err(ModuleResolveError::DuplicateImportName {
            namespace: Namespace::Term,
            name,
            first,
            duplicate,
            ..
        }) => Some((name.to_string(), first, duplicate)),
        Err(other) => panic!("unexpected resolver error for {source:?}: {other:?}"),
    }
}

#[test]
fn include_alias_colliding_with_local_node_is_rejected() {
    // B4: a module-form include alias is a Term name like an import alias,
    // even when it instantiates a local inline DAG whose edge the loader never
    // registers. The node is the first definition; the alias the duplicate.
    let source = "dag velocity { param r: Dimensionless; pub node v: Dimensionless = @r; }\n\
                  include velocity(r: 1.0) as parking;\n\
                  node parking: Dimensionless = 2.0;";
    let (name, first, duplicate) = duplicate_term_alias(source).unwrap();
    assert_eq!(name, "parking");
    assert_eq!(
        &source[first.offset()..first.offset() + first.len()],
        "parking"
    );
    assert!(duplicate.offset() < first.offset());
}

#[test]
fn every_alias_form_claims_a_term_slot() {
    for source in [
        "dag velocity {}\nimport velocity as parking;\nnode parking: Dimensionless = 2.0;",
        "dag velocity {}\ninclude velocity() as parking;\ntype parking { parking }",
        "import app.lib;\nnode lib: Dimensionless = 2.0;",
        "include app.lib();\nconst node lib: Dimensionless = 2.0;",
        "import plugin \"p.wasm\" as parking { fn f(x: Dimensionless) -> Dimensionless; }\nnode parking: Dimensionless = 2.0;",
    ] {
        assert!(
            duplicate_term_alias(source).is_some(),
            "{source:?} should be rejected"
        );
    }
}

#[test]
fn alias_respelling_its_local_dag_is_that_dags_own_name() {
    for source in [
        "dag velocity {}\ninclude velocity();",
        "dag velocity {}\ninclude velocity() as velocity;",
        "dag velocity {}\nimport velocity;",
    ] {
        assert_eq!(duplicate_term_alias(source), None, "{source:?}");
    }
    // A single-segment path that names a non-DAG local is still a collision.
    assert!(
        duplicate_term_alias("node velocity: Dimensionless = 1.0;\ninclude velocity();").is_some()
    );
}

#[test]
fn local_and_selected_bindings_to_same_dag_are_one_callable() {
    let root_id = DagId::root_in_package("test", "self");
    let helper_id =
        root_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let calculation_id = root_id.inline_dag_child(
        crate::syntax::decl_name::DeclName::expect_valid("calculation"),
    );
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

    let mut modules = TestModules::default();
    modules.add(root_id.clone(), &root.declarations);
    modules.add(helper_id.clone(), &helper.body);
    modules.add(calculation_id.clone(), &calculation.body);
    modules.import(&calculation_id, import, &root_id);
    let resolver = modules.build().unwrap();

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
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node shown: Dimensionless = 1.0;
        }",
    );
    let main = desugared_source("import lib as lib;");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(helper_id, &first_dag(&lib).body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

    let err = resolver
        .resolve_module_path(&main_id, &module_path(&["lib", "helper"]))
        .unwrap_err();

    assert!(matches!(
        err,
        ModuleResolveError::PrivateName {
            owner,
            category: NameCategory::Dag,
            name,
        } if owner == lib_id && name.as_str() == "helper"
    ));
}

#[test]
fn qualified_symbol_path_through_private_dag_is_rejected() {
    // Regression: `resolve_symbol_path` resolved the qualifier without
    // the dag-visibility check that `resolve_module_path` enforces, so
    // `lib.helper.shown` resolved even though `helper` is a private dag.
    let lib_id = DagId::root_in_package("test", "lib");
    let helper_id =
        lib_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("helper"));
    let main_id = DagId::root_in_package("test", "main");
    let lib = desugared_source(
        "dag helper {
            pub node shown: Dimensionless = 1.0;
        }",
    );
    let main = desugared_source("import lib as lib;");
    let import = first_import(&main);

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(helper_id, &first_dag(&lib).body);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

    let err = resolver
        .resolve_decl_path(&main_id, &path(&["lib", "helper", "shown"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap_err();

    assert!(
        matches!(
            err,
            ModuleResolveError::PrivateName {
                ref owner,
                category: NameCategory::Dag,
                ref name,
            } if *owner == lib_id && name.as_str() == "helper"
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

    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, import, &lib_id);
    let resolver = modules.build().unwrap();

    let resolved_name = resolver
        .resolve_constructor_path(&main_id, &path(&["mission", "Impulsive"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
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

    let mut modules = TestModules::default();
    modules.add(leaf_id.clone(), &leaf.declarations);
    modules.add(middle_id.clone(), &middle.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&middle_id, middle_import, &leaf_id);
    modules.import(&main_id, main_import, &middle_id);
    let resolver = modules.build().unwrap();
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
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();

    assert_eq!(resolved_name.owner(), &leaf_id);
    assert_eq!(resolved_name.as_str(), "Acceleration");
}

#[test]
fn lookup_categories_render_the_established_labels() {
    let owner = DagId::root_in_package("test", "main");
    let unknown = |category| ModuleResolveError::UnknownName {
        owner: owner.clone(),
        category,
        name: NameAtom::parse("x").unwrap(),
    };
    let private = |category| ModuleResolveError::PrivateName {
        owner: owner.clone(),
        category,
        name: NameAtom::parse("x").unwrap(),
    };
    let cases = [
        (NameCategory::Table(SymbolTable::Decl), "DeclName"),
        (
            NameCategory::Table(SymbolTable::Constructor),
            "ConstructorName",
        ),
        (NameCategory::Table(SymbolTable::Dimension), "DimName"),
        (
            NameCategory::Table(SymbolTable::StructType),
            "StructTypeName",
        ),
        (NameCategory::Table(SymbolTable::Index), "IndexName"),
        (NameCategory::Table(SymbolTable::Unit), "UnitName"),
        (NameCategory::Namespace(Namespace::Static), "Static"),
        (NameCategory::Namespace(Namespace::Term), "Term"),
        (NameCategory::Namespace(Namespace::Unit), "Unit"),
        (NameCategory::TermImport, "term import namespace"),
        (NameCategory::DagAlias, "dag alias"),
        (NameCategory::Dag, "dag"),
    ];
    for (category, label) in cases {
        assert_eq!(
            unknown(category).to_string(),
            format!("unknown {label} `x` in module `main`")
        );
        assert_eq!(
            private(category).to_string(),
            format!("private {label} `x` in module `main`")
        );
    }
}

#[test]
fn duplicate_and_decl_kind_errors_render_the_established_messages() {
    use crate::resolved_name::ResolvedDeclName;
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::function_name::FnName;
    use crate::syntax::index_name::IndexName;

    let owner = DagId::root_in_package("test", "main");
    let span = Span::new(0, 1);
    let unexpected = |expected| ModuleResolveError::UnexpectedDeclKind {
        name: ResolvedDeclName::from_def(owner.clone(), DeclName::expect_valid("x")),
        expected,
        actual: DeclSymbolKind::Node,
    };
    let cases = [
        (
            ModuleResolveError::DuplicateSymbol {
                owner: owner.clone(),
                namespace: Namespace::Unit,
                name: NameAtom::parse("m").unwrap(),
                first: span,
                duplicate: span,
            },
            "duplicate Unit `m` in module `main`",
        ),
        (
            ModuleResolveError::DuplicateImportName {
                owner: owner.clone(),
                namespace: Namespace::Static,
                name: NameAtom::parse("M").unwrap(),
                first: span,
                duplicate: span,
            },
            "duplicate imported Static `M` in module `main`",
        ),
        (
            ModuleResolveError::DuplicateIndexVariant {
                owner: owner.clone(),
                variant: IndexVariantName::expect_valid("Burn")
                    .qualified_by(&IndexName::expect_valid("Phase")),
                first: span,
                duplicate: span,
            },
            "duplicate IndexVariantName `Phase#Burn` in module `main`",
        ),
        (
            ModuleResolveError::DuplicatePluginFunction {
                owner: owner.clone(),
                function: FnName::expect_valid("f"),
                first: span,
                duplicate: span,
            },
            "duplicate FnName `f` in module `main`",
        ),
        (
            unexpected(ExpectedDeclKind::Const),
            "expected const declaration `main.x`, found node",
        ),
        (
            unexpected(ExpectedDeclKind::InstanceIndependentConst),
            "expected instance-independent const declaration `main.x`, found node",
        ),
        (
            unexpected(ExpectedDeclKind::GraphValue),
            "expected graph value declaration `main.x`, found node",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

fn decl(name: &str) -> crate::syntax::decl_name::DeclName {
    crate::syntax::decl_name::DeclName::expect_valid(name)
}

#[test]
fn file_submodule_and_inline_dag_with_one_spelling_are_ambiguous() {
    let lib = desugared_source("pub dag x { pub const node a: Dimensionless = 1.0; }");
    let lib_x = desugared_source("pub const node a: Dimensionless = 2.0;");
    let lib_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib"]));
    let inline_id = lib_id.inline_dag_child(decl("x"));
    let file_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib", "x"]));

    let mut tables = SymbolTables::default();
    tables.add_file(lib_id, &lib.declarations).unwrap();
    let error = tables
        .add_module(file_id.clone(), &lib_x.declarations)
        .unwrap_err();

    assert_eq!(
        error,
        ModuleResolveError::AmbiguousModulePath {
            first: inline_id,
            second: file_id,
        }
    );
    assert_eq!(
        error.to_string(),
        "module path `pkg.lib.x` is ambiguous: it names a module in file `pkg.lib` and a module in file `pkg.lib.x`"
    );
}

#[test]
fn same_spelling_in_another_package_or_instance_is_not_ambiguous() {
    let body = desugared_source("pub const node a: Dimensionless = 2.0;");
    let lib_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib"]));
    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &body.declarations);
    modules.add(lib_id.inline_dag_child(decl("x")), &body.declarations);
    modules.add(
        DagId::new("other", NonEmpty::new("pkg", vec!["lib", "x"])),
        &body.declarations,
    );
    modules.add(
        lib_id.instance_child(crate::syntax::module_name::ScopeSegment::Named(
            crate::syntax::module_name::ModuleAliasName::expect_valid("x"),
        )),
        &body.declarations,
    );
}

#[test]
fn alias_qualifier_does_not_reach_a_file_submodule() {
    let lib = desugared_source("pub const node b: Dimensionless = 1.0;");
    let lib_x = desugared_source("pub const node a: Dimensionless = 2.0;");
    let main = desugared_source("import pkg.lib as l;");
    let lib_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib"]));
    let lib_x_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib", "x"]));
    let main_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["main"]));
    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(lib_x_id, &lib_x.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, first_import(&main), &lib_id);
    let resolver = modules.build().unwrap();

    // `l.x` names an inline `dag x` of `lib`, never the file `lib/x.gcl`.
    assert_eq!(
        resolver
            .resolve_decl_path(&main_id, &path(&["l", "x", "a"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved),
        Err(ModuleResolveError::UnknownModule {
            owner: lib_id.inline_dag_child(decl("x")),
        })
    );
    assert_eq!(
        resolver
            .resolve_decl_path(&main_id, &path(&["l", "b"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|name| name.owner().clone()),
        Ok(lib_id)
    );
}

#[test]
fn resolved_symbols_carry_their_target_declaration_facts() {
    let lib = desugared_source(
        "pub const node k: Dimensionless = 1.0;
         pub node n: Dimensionless = 2.0;
         pub type T<D: Dim> { T }
         pub base unit u: Dimensionless;",
    );
    let main = desugared_source(
        "import lib::{ k, n as renamed, type T, unit u };
         param p: Dimensionless;
         node hidden: Dimensionless = 1.0;
         pub node shown: Dimensionless = 1.0;",
    );
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, first_import(&main), &lib_id);
    let resolver = modules.build().unwrap();

    // A selective import carries the kind of the target's declaration.
    let k = resolver.resolve_decl_path(&main_id, &path(&["k"])).unwrap();
    assert_eq!(*k.kind(), DeclSymbolKind::Const);
    assert_eq!(k.resolved().owner(), &lib_id);
    let renamed = resolver
        .resolve_decl_path(&main_id, &path(&["renamed"]))
        .unwrap();
    assert_eq!(*renamed.kind(), DeclSymbolKind::Node);
    assert_eq!(renamed.resolved().as_str(), "n");
    assert_eq!(
        resolver
            .resolve_struct_type_path(&main_id, &path(&["T"]))
            .unwrap()
            .kind()
            .len(),
        1
    );
    assert_eq!(
        *resolver
            .resolve_unit_path(&main_id, &path(&["u"]))
            .unwrap()
            .kind(),
        UnitConstness::Const
    );

    // Const-only resolution rejects a non-const target by its carried kind.
    assert!(
        resolver
            .resolve_const_decl_path(&main_id, &path(&["k"]))
            .is_ok()
    );
    assert!(matches!(
        resolver.resolve_const_decl_path(&main_id, &path(&["renamed"])),
        Err(ModuleResolveError::UnexpectedDeclKind {
            expected: ExpectedDeclKind::Const,
            actual: DeclSymbolKind::Node,
            ..
        })
    ));

    // An identity carried past resolution finds its declaration's facts.
    let n = resolver
        .symbol(renamed.resolved())
        .expect("the target is declared in `lib`");
    assert_eq!(*n.kind(), DeclSymbolKind::Node);
    assert!(n.visibility().is_public());

    // Parameters are instance inputs even without `pub`.
    let accessible = |name: &str| {
        resolver
            .resolve_decl_path(&main_id, &path(&[name]))
            .unwrap()
            .is_instance_accessible()
    };
    assert!(accessible("p"));
    assert!(!accessible("hidden"));
    assert!(accessible("shown"));
}

#[test]
fn recursive_include_expansion_is_rejected_with_its_template_cycle() {
    let main = desugared_source(
        "dag first { include second() as next; }
         dag second { include first() as next; }",
    );
    let main_id = DagId::root_in_package("test", "main");
    let first = main_id.inline_dag_child(decl("first"));
    let second = main_id.inline_dag_child(decl("second"));
    let mut modules = TestModules::default();
    modules.add_file(&main_id, &main.declarations);
    modules.edge(&first, &module_path(&["second"]), &second);
    modules.edge(&second, &module_path(&["first"]), &first);

    assert_eq!(
        modules.build(),
        Err(ModuleResolveError::RecursiveIncludeExpansion {
            module: first.clone(),
            cycle: vec![first.clone(), second, first],
        })
    );
}

#[test]
fn include_of_a_later_sibling_sees_its_completed_scope() {
    // `a` precedes `b`, but its include selects a re-export that only `b`'s
    // own import introduces, so `b`'s scope must be completed first.
    let lib = desugared_source("pub const node k: Dimensionless = 1.0;");
    let main = desugared_source(
        "dag a { include b()::{ k }; }
         dag b { import lib::{ pub k }; }",
    );
    let lib_id = DagId::root_in_package("test", "lib");
    let main_id = DagId::root_in_package("test", "main");
    let a = main_id.inline_dag_child(decl("a"));
    let b = main_id.inline_dag_child(decl("b"));
    let mut modules = TestModules::default();
    modules.add(lib_id.clone(), &lib.declarations);
    modules.add_file(&main_id, &main.declarations);
    modules.edge(&a, &module_path(&["b"]), &b);
    modules.edge(&b, &module_path(&["lib"]), &lib_id);
    let resolver = modules.build().unwrap();

    let k = resolver
        .resolve_decl_path(&a, &path(&["k"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();
    assert_eq!((k.owner(), k.as_str()), (&lib_id, "k"));

    // The instance carries `b`'s completed scope as well.
    let include = first_include_decl(&ast::File {
        declarations: first_dag(&main).body.clone(),
    })
    .clone();
    let instance = a.instance_child(include.instance_scope());
    assert!(resolver.symbols(&instance).is_some());
    assert_eq!(
        resolver
            .resolve_decl_path(&instance, &path(&["k"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved),
        Ok(k)
    );
}

#[test]
fn include_instances_are_expanded_through_nested_templates() {
    let leaf = desugared_source("pub node value: Dimensionless = 1.0;");
    let middle = desugared_source("include leaf() as inner;");
    let main = desugared_source("include middle() as outer;");
    let leaf_id = DagId::root_in_package("test", "leaf");
    let middle_id = DagId::root_in_package("test", "middle");
    let main_id = DagId::root_in_package("test", "main");
    let mut modules = TestModules::default();
    modules.add(leaf_id.clone(), &leaf.declarations);
    modules.add(middle_id.clone(), &middle.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.edge(&middle_id, &module_path(&["leaf"]), &leaf_id);
    modules.edge(&main_id, &module_path(&["middle"]), &middle_id);
    let resolver = modules.build().unwrap();

    let named = |alias: &str| {
        crate::syntax::module_name::ScopeSegment::Named(
            crate::syntax::module_name::ModuleAliasName::expect_valid(alias),
        )
    };
    let outer = main_id.instance_child(named("outer"));
    let inner = outer.instance_child(named("inner"));
    assert!(resolver.symbols(&outer).is_some());
    assert!(resolver.symbols(&inner).is_some());
    // The nested instance inherits `middle`'s scope, whose alias names it.
    assert_eq!(
        resolver.module_alias_role(
            &outer,
            &crate::syntax::module_name::ModuleAliasName::expect_valid("inner")
        ),
        Some(super::scope::ModuleAliasRole::IncludedInstance)
    );
    let value = resolver
        .resolve_decl_path(&inner, &path(&["value"]))
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .unwrap();
    assert_eq!(value.owner(), &inner);
}

#[test]
fn file_submodule_does_not_inherit_the_visibility_of_a_parent_file_dag() {
    // A private non-`dag` declaration `x` in `lib.gcl` has no bearing on the
    // file `lib/x.gcl`, and neither would a private `dag x` (which would be
    // ambiguous and rejected when both modules are registered).
    let lib = desugared_source("const node x: Dimensionless = 1.0;");
    let lib_x = desugared_source("pub const node a: Dimensionless = 2.0;");
    let main = desugared_source("import pkg.lib.x::{ a };");
    let lib_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib"]));
    let lib_x_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["lib", "x"]));
    let main_id = DagId::new("pkg", NonEmpty::new("pkg", vec!["main"]));
    let mut modules = TestModules::default();
    modules.add(lib_id, &lib.declarations);
    modules.add(lib_x_id.clone(), &lib_x.declarations);
    modules.add(main_id.clone(), &main.declarations);
    modules.import(&main_id, first_import(&main), &lib_x_id);
    let resolver = modules.build().unwrap();
    assert_eq!(
        resolver
            .resolve_decl_path(&main_id, &path(&["a"]))
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map(|name| name.owner().clone()),
        Ok(lib_x_id)
    );
}
