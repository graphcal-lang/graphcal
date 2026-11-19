//! Import/include projections and the static inputs they may bind.

use super::*;

#[test]
fn qualified_import_rejects_required_static_members() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub(bind) type Element;"),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Element) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportRequiredStaticInput { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn qualified_import_rejects_transitive_required_static_dependencies() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) type Element;\n\
                 pub type Box { Box(value: Element) }\n\
                 pub type Wrapper { Wrapper(value: Box) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Wrapper) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Type, dependency, .. }), .. }), .. }) if dependency.as_str() == "Element"
    ));
}

#[test]
fn qualified_import_resolves_ambiguous_static_dependencies_to_their_symbol() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) dim Basis;\n\
                 pub type Reading { Reading(value: Basis) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Reading) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Dimension, dependency, .. }), .. }), .. }) if dependency.as_str() == "Basis"
    ));
}

#[test]
fn selective_import_rejects_transitive_required_static_dependencies() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) type Element;\n\
                 pub type Box { Box(value: Element) }\n\
                 pub type Wrapper { Wrapper(value: Box) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib::{ type Wrapper };\nnode output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Type, dependency, .. }), .. }), .. }) if dependency.as_str() == "Element"
    ));
}

#[test]
fn selective_include_projects_effective_bound_static_target() {
    let source = "type Concrete { Concrete }\n\
                  dag target { pub(bind) type Slot; }\n\
                  include target(type Slot: Concrete)::{ type Slot as Effective };\n\
                  node value: Effective = Concrete;\n\
                  node output: Dimensionless = 1.0;";
    compile_and_eval(source).expect("the projected type is the effective binding target");
}

#[test]
fn cross_file_selective_include_projects_effective_bound_static_target() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub(bind) type Slot;"),
            (
                "main.gcl",
                "type Concrete { Concrete }\n\
                 include pipeline.lib(type Slot: Concrete)::{ type Slot as Effective };\n\
                 type Envelope { Envelope(value: Effective) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    compile_and_eval_project(&root, &HashMap::new(), None, &fs())
        .expect("cross-file projection uses the concrete binding target");
}

#[test]
fn selective_include_projects_closed_static_and_runtime_unit_categories() {
    let source = "dag target {\n\
                      pub type Record { Record }\n\
                      pub dim Distance = Length;\n\
                      pub index Axis = { Only };\n\
                      pub unit double_metre: Length = 2.0 m;\n\
                  }\n\
                  include target()::{\n\
                      type Record as EffectiveRecord,\n\
                      dim Distance as EffectiveDistance,\n\
                      index Axis as EffectiveAxis,\n\
                      unit double_metre as effective_double_metre,\n\
                  };\n\
                  type Envelope { Envelope(value: EffectiveRecord) }\n\
                  node distance: EffectiveDistance = 1.0 m;\n\
                  node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis { 1.0 };\n\
                  node scaled: Length = 1.0 effective_double_metre;";
    let result = compile_and_eval(source).expect("all typed projection markers resolve");
    assert_quantity_value(&result, "distance", 1.0);
    assert_quantity_value(&result, "scaled", 2.0);
}

#[test]
fn selective_include_unit_projection_resolves_in_unit_definitions() {
    // A projected unit alias is visible to the importer's own unit
    // definitions, not only to expressions.
    let source = "dag target {\n\
                      pub const unit double_metre: Length = 2.0 m;\n\
                  }\n\
                  include target()::{ unit double_metre as effective_double_metre };\n\
                  const unit quad_metre: Length = 2.0 effective_double_metre;\n\
                  node scaled: Length = 1.0 quad_metre;";
    let result = compile_and_eval(source).expect("projected unit alias resolves");
    assert_quantity_value(&result, "scaled", 4.0);
}

#[test]
fn renamed_include_projection_does_not_bind_the_source_name_in_declarations() {
    // `dim X as Y` / `unit X as Y` bind only `Y` in the importer's own
    // `dim` / `unit` declarations; `X` is as unknown as any undeclared name.
    let target = "dag target {\n\
                      pub dim Rate = Mass / Time;\n\
                      pub const unit double_metre: Length = 2.0 m;\n\
                  }\n";
    for (body, expected) in [
        (
            "include target()::{ dim Rate as R };\ndim D = Rate * Time;",
            "Rate",
        ),
        (
            "include target()::{ dim Rate as R };\nunit u: Rate = 2.0 kg/s;",
            "Rate",
        ),
    ] {
        match compile_and_eval(&format!("{target}{body}")) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind:
                            SemanticErrorKind::Dimension(DimensionError::UnknownDimension {
                                name, ..
                            }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), expected, "{body}");
            }
            other => panic!("expected UnknownDimension for {expected} in {body}, got {other:?}"),
        }
    }
    for body in [
        "include target()::{ unit double_metre as dm };\n\
         const unit quad_metre: Length = 2.0 double_metre;",
        "include target()::{ unit double_metre as dm };\n\
         unit quad_metre: Length = 2.0 double_metre;",
    ] {
        match compile_and_eval(&format!("{target}{body}")) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { name, .. }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), "double_metre", "{body}");
            }
            other => panic!("expected UnknownUnit for double_metre in {body}, got {other:?}"),
        }
    }
}

#[test]
fn selective_include_projects_specialized_adt_constructors() {
    let source = "dag target {\n\
                      pub(bind) dim Quantity;\n\
                      param amount: Quantity;\n\
                      pub type Reading { Missing, Present(value: Quantity) }\n\
                      pub node supplied: Reading = Present(value: @amount);\n\
                  }\n\
                  include target(amount: 2.0 m, dim Quantity: Length)::{\n\
                      type Reading,\n\
                      Missing as NoReading,\n\
                      Present as HasReading,\n\
                      supplied,\n\
                  };\n\
                  node local: Reading = HasReading(value: 2.0 m);\n\
                  node reading: Reading = @supplied;\n\
                  node output: Length = match @reading {\n\
                      HasReading(value: amount) => amount,\n\
                      NoReading => 0.0 m,\n\
                  };";
    let result = compile_and_eval(source)
        .expect("constructors retain the specialized nominal type identity");
    assert_quantity_value(&result, "output", 2.0);
}

#[test]
fn selective_include_rejects_constructor_of_rebound_owner_type() {
    let source = "type Replacement { Replacement }\n\
                  dag target { pub(bind) type Choice { Pick } }\n\
                  include target(type Choice: Replacement)::{ Pick };\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::IncludeConstructorOwnerRebound { constructor, owner_type, .. }), .. }), .. }) if constructor.as_str() == "Pick" && owner_type.to_string().ends_with("Choice")
    ));
}

#[test]
fn selective_include_projects_structural_index_binding_target() {
    let source = "dag target { pub(bind) index Axis; }\n\
                  include target(index Axis: Fin(2))::{ index Axis as EffectiveAxis };\n\
                  node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis { 1.0 };";
    compile_and_eval(source).expect("the projected index has structural Fin(2) identity");
}

#[test]
fn structural_index_binding_cardinality_must_be_closed() {
    let program = |cardinality: &str| {
        format!(
            "dag target {{ pub(bind) index Axis; }}\n\
             include target(index Axis: Fin({cardinality}))::{{ index Axis as EffectiveAxis }};\n\
             node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis {{ 1.0 }};"
        )
    };
    compile_and_eval(&program("1 + 2 * 3")).expect("a closed cardinality binds");
    assert!(matches!(
        compile_and_eval(&program("N + 1")).unwrap_err(),
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::UnknownIndex { name, .. }), .. }), .. }) if name.to_string() == "N"
    ));
    assert!(matches!(
        compile_and_eval(&program("4294967296 * 4294967296")).unwrap_err(),
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::FinCardinalityMultiplicationOverflow),
                ..
            }),
            ..
        })
    ));
    // A closed part that overflows is reported even after a name, with the
    // same code as a `Fin(...)` written in a declaration.
    assert!(matches!(
        compile_and_eval(&program("N + 4294967296 * 4294967296")).unwrap_err(),
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::FinCardinalityMultiplicationOverflow),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn include_rejects_required_static_inputs_as_binding_targets() {
    let source = "pub(bind) type Replacement;\n\
                  dag target { pub(bind) type Slot; }\n\
                  include target(type Slot: Replacement) as configured;\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::InvalidStaticBindingTarget { kind: graphcal_compiler::static_interface::StaticInputKind::Type, name, target, .. }), .. }), .. }) if name.as_str() == "Slot" && target.as_str() == "Replacement"
    ));
}

#[test]
fn include_requires_every_required_static_input_category() {
    for (declaration, expected_kind) in [
        (
            "pub(bind) type Element;",
            graphcal_compiler::static_interface::StaticInputKind::Type,
        ),
        (
            "pub(bind) dim Quantity;",
            graphcal_compiler::static_interface::StaticInputKind::Dimension,
        ),
        (
            "pub(bind) index Axis;",
            graphcal_compiler::static_interface::StaticInputKind::Index,
        ),
    ] {
        let (_directory, root) = write_pipeline_project(
            &[
                ("lib.gcl", declaration),
                (
                    "main.gcl",
                    "include pipeline.lib() as configured;\nnode output: Dimensionless = 1.0;",
                ),
            ],
            "main.gcl",
        );
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::RequiredStaticInputNotBound { kind, .. }), .. }), .. })
                if kind == expected_kind
        ));
    }
}

/// TIR-2 (#2265): a file module whose root declarations mention its own
/// required type port is a valid library. Checking it alone, and including it
/// with the port bound, must succeed; evaluating it as the entry DAG reports
/// the unbound port (I010), never an internal error.
#[test]
fn file_root_declarations_may_mention_a_required_type_port() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) type T;\nparam b: T;\npub node out: T = @b;",
            ),
            (
                "boxed.gcl",
                "pub(bind) type T;\npub type Box { Box(v: T) }\nparam b: Box;",
            ),
            (
                "main.gcl",
                "pub type Outer { Leaf }\n\
                 include pipeline.lib(type T: Outer, b: Leaf) as inst;\n\
                 node o: Outer = @inst::out;",
            ),
        ],
        "main.gcl",
    );
    let libraries = [
        root.with_file_name("lib.gcl"),
        root.with_file_name("boxed.gcl"),
    ];
    for file in libraries.iter().chain([&root]) {
        let project = crate::loader::load_project(file, None, &fs()).unwrap();
        ProjectCompiler::new(&project)
            .check()
            .unwrap_or_else(|error| panic!("{} must check: {error}", file.display()));
    }
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(
        result
            .nodes()
            .any(|(name, value)| name.to_string() == "o" && value.is_ok())
    );

    for library in &libraries {
        let error = compile_and_eval_project(library, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::RequiredStaticInputNotBound { kind: graphcal_compiler::static_interface::StaticInputKind::Type, name }), .. }), .. })
                    if name.as_str() == "T"
            ),
            "{error:?}"
        );
    }
}

#[test]
fn fixed_static_declarations_are_not_binding_ports() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type Element { Element }"),
            (
                "main.gcl",
                "type LocalElement { LocalElement }\n\
                 include pipeline.lib(type Element: LocalElement) as configured;\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::DagInputCategoryMismatch {
                    expected: "type",
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn bare_graph_declaration_refs_fail_before_dependency_planning() {
    let cases = [
        (
            "later runtime node",
            "node b_ref: Dimensionless = z_target * 3.0;\n\
             node z_target: Dimensionless = 2.0;",
            "z_target",
        ),
        (
            "earlier runtime node",
            "node a: Dimensionless = 2.0;\n\
             node b: Dimensionless = a * 3.0;",
            "a",
        ),
        (
            "parameter",
            "param input: Dimensionless = 2.0;\n\
             node output: Dimensionless = input;",
            "input",
        ),
        (
            "const node",
            "const node factor: Dimensionless = 2.0;\n\
             node output: Dimensionless = factor;",
            "factor",
        ),
        (
            "mixed bare and graph-reference cycle",
            "node a: Dimensionless = @b + 1.0;\n\
             node b: Dimensionless = a + 1.0;",
            "a",
        ),
    ];

    for (case, source, expected_name) in cases {
        let error = compile_and_eval_named(source, "test.gcl").unwrap_err();
        assert!(
            matches!(
                error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BareGraphDeclarationRef { name, .. }), .. }), .. })
                    if name.to_string() == expected_name
            ),
            "case: {case}"
        );
    }
}
