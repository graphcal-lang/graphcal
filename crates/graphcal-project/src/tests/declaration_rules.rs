//! Declaration-level rules: constructor syntax, attributes, reserved names, include producers.

use super::*;

#[test]
fn payload_constructor_empty_pattern_keeps_missing_field_diagnostic() {
    let source = "type Reading { Missing, Present(value: Length) }\n\
                  node reading: Reading = Present(value: 2.0 m);\n\
                  node output: Length = match @reading {\n\
                      Missing => 0.0 m,\n\
                      Present() => 1.0 m,\n\
                  };";
    let error = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(StructError::MissingPatternFields { constructor, .. }), .. }), .. })
                if constructor.as_str() == "Present"
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn selective_include_rejects_dag_blueprint_projection() {
    let source = "dag target {\n\
                      pub dag child { pub node output: Dimensionless = 1.0; }\n\
                  }\n\
                  include target()::{ child };\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::IncludeItemNotProjectable { name, .. }), .. }), .. }) if name.as_str() == "child"
    ));
}

#[test]
fn safety_attributes_reject_repetition_and_invalid_assumptions() {
    let repeated_expected_fail = "#[expected_fail]\n\
                                  #[expected_fail]\n\
                                  assert check = false;";
    let error = compile_and_eval_named(repeated_expected_fail, "test.gcl").unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let invalid_assumes = [
        "assert guard = true;\n#[assumes]\nnode output: Dimensionless = 1.0;",
        "assert guard = true;\n#[assumes(guard, guard)]\nnode output: Dimensionless = 1.0;",
        "assert premise_a = true;\nassert premise_b = true;\n#[assumes(premise_a)]\n#[assumes(premise_b)]\nnode output: Dimensionless = 1.0;",
    ];
    for source in invalid_assumes {
        assert!(compile_and_eval_named(source, "test.gcl").is_err());
    }

    let valid = "assert premise_a = true;\n\
                 assert premise_b = true;\n\
                 #[assumes(premise_b, premise_a)]\n\
                 node output: Dimensionless = 1.0;";
    assert!(compile_and_eval_named(valid, "test.gcl").is_ok());
}

#[test]
fn repeated_expected_fail_is_rejected_on_file_and_inline_include_items() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub assert check = false;\n"),
            (
                "main.gcl",
                "include pipeline.lib()::{\n    #[expected_fail]\n    #[expected_fail]\n    check,\n};\n",
            ),
        ],
        "main.gcl",
    );
    let file_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        file_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let inline = "dag checks { pub assert check = false; }\n\
                  include checks()::{\n\
                      #[expected_fail]\n\
                      #[expected_fail]\n\
                      check,\n\
                  };";
    let inline_error = compile_and_eval_named(inline, "test.gcl").unwrap_err();
    assert!(matches!(
        inline_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn repeated_hidden_is_rejected_on_plots_and_include_items() {
    let declaration = "node x: Dimensionless = 1.0;\n\
                       #[hidden]\n\
                       #[hidden]\n\
                       plot chart = { mark: point, encode: { x: @x } };";
    let declaration_error = compile_and_eval_named(declaration, "test.gcl").unwrap_err();
    assert!(matches!(
        declaration_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::Hidden,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let producer = "pub node x: Dimensionless = 1.0;\n\
                    pub plot chart = { mark: point, encode: { x: @x } };\n";
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", producer),
            (
                "main.gcl",
                "include pipeline.lib()::{ #[hidden] #[hidden] chart };\n",
            ),
        ],
        "main.gcl",
    );
    let include_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        include_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::Hidden,
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn hidden_with_arguments_is_rejected_on_include_items() {
    let producer = "pub node x: Dimensionless = 1.0;\n\
                    pub plot chart = { mark: point, encode: { x: @x } };\n";
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", producer),
            (
                "main.gcl",
                "include pipeline.lib()::{ #[hidden(now)] chart };\n",
            ),
        ],
        "main.gcl",
    );
    let include_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        include_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::HiddenTakesNoArguments),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn time_scale_spellings_are_disjoint_from_graph_value_namespaces() {
    for scale in graphcal_compiler::semantic::time_scale::TimeScale::ALL {
        for declaration in [
            format!("param {scale}: Dimensionless = 1.0;"),
            format!("node {scale}: Dimensionless = 1.0;"),
            format!("const node {scale}: Dimensionless = 1.0;"),
        ] {
            let source = format!(
                "{declaration}\n\
                 node copied: Dimensionless = @{scale};\n\
                 node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
            );
            compile_and_eval_named(&source, "test.gcl").unwrap();
        }

        let aliased = format!(
            "dag producer {{ pub node value: Dimensionless = 1.0; }}\n\
             include producer()::{{ value as {scale} }};\n\
             node copied: Dimensionless = @{scale};\n\
             node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
        );
        compile_and_eval_named(&aliased, "test.gcl").unwrap();

        let constructor = format!(
            "type ScaleTag {{ {scale} }}\n\
             node tag: ScaleTag = {scale};\n\
             node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
        );
        compile_and_eval_named(&constructor, "test.gcl").unwrap();
    }

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type ScaleTag { Only }\n"),
            (
                "main.gcl",
                "import pipeline.lib::{ type ScaleTag, Only as UTC };\n\
                 node tag: ScaleTag = UTC;\n\
                 node event: Datetime<UTC> = epoch<UTC>(\"2024-01-01T00:00:00\");\n",
            ),
        ],
        "main.gcl",
    );
    compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let bare_error = compile_and_eval_named(
        "node UTC: Dimensionless = 1.0;\nnode invalid: Dimensionless = UTC;",
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        bare_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BareGraphDeclarationRef { name, .. }), .. }), .. })
            if name.to_string() == "UTC"
    ));
}

/// Selective-include aliases obey the reserved-name policy of the namespace
/// they occupy, and the diagnostic points at the local alias (RES-3).
fn assert_include_aliases_take_no_reserved_spelling(library: &str) {
    let invalid_includes = [
        ("include pipeline.lib()::{ runtime as E };\n", "E"),
        ("include pipeline.lib()::{ type Choice, Only as E };\n", "E"),
        (
            "dag producer { pub node value: Dimensionless = 1.0; }\ninclude producer()::{ value as E };\n",
            "E",
        ),
        (
            "dag producer { pub type Choice { Only } }\ninclude producer()::{ type Choice, Only as E };\n",
            "E",
        ),
        // RES-3: selective-include aliases of units, dimensions, types, and
        // indexes must not redefine a prelude or built-in spelling.
        ("include pipeline.lib()::{ unit custom_unit as m };\n", "m"),
        (
            "include pipeline.lib()::{ dim CustomDim as Length };\n",
            "Length",
        ),
        (
            "include pipeline.lib()::{ type CustomType as Bool };\n",
            "Bool",
        ),
        (
            "include pipeline.lib()::{ index CustomIndex as Mass };\n",
            "Mass",
        ),
        ("include pipeline.lib()::{ constant as sum };\n", "sum"),
        (
            "dag producer { pub const unit ft: Length = 0.3048 m; }\n\
             include producer()::{ unit ft as m };\n\
             node x: Length = 1.0 m;\n",
            "m",
        ),
    ];
    for (main, expected_name) in invalid_includes {
        let error = if main.starts_with("dag") {
            compile_and_eval_named(main, "test.gcl").unwrap_err()
        } else {
            let (_directory, root) =
                write_pipeline_project(&[("lib.gcl", library), ("main.gcl", main)], "main.gcl");
            compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err()
        };
        let CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }),
                    primary: span,
                    ..
                }),
            ..
        }) = error
        else {
            panic!("{main}: expected N009, got {error:?}");
        };
        assert_eq!(name.to_string(), expected_name, "{main}");
        let alias_start = main.find(&format!("{expected_name} }}")).unwrap();
        assert_eq!(
            span,
            graphcal_compiler::syntax::span::Span::new(alias_start, expected_name.len()),
            "{main}: the diagnostic points at the local alias"
        );
    }
}

#[test]
fn reserved_name_policy_covers_import_include_and_reexport_aliases() {
    let library = "pub dim CustomDim = Length;\n\
                   pub const unit custom_unit: Length = 2.0 m;\n\
                   pub type CustomType { CustomType }\n\
                   pub type Choice { Only }\n\
                   pub index CustomIndex = { One };\n\
                   pub const node constant: Dimensionless = 1.0;\n\
                   pub node runtime: Dimensionless = 2.0;\n";
    let invalid_imports = [
        ("dim CustomDim as Velocity", "Velocity"),
        ("unit custom_unit as m", "m"),
        ("type CustomType as Bool", "Bool"),
        ("index CustomIndex as Length", "Length"),
        ("constant as E", "E"),
        ("Only as E", "E"),
        ("Only as sum", "sum"),
        ("Only as scan", "scan"),
    ];

    for (item, expected_name) in invalid_imports {
        let main = format!("import pipeline.lib::{{ {item} }};\n");
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", library), ("main.gcl", &main)], "main.gcl");
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. })
                if name.to_string() == expected_name
        ));
    }

    assert_include_aliases_take_no_reserved_spelling(library);

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub const node value: Dimensionless = 1.0;\n"),
            ("middle.gcl", "import pipeline.lib::{ pub value as E };\n"),
            ("main.gcl", "import pipeline.middle as middle;\n"),
        ],
        "main.gcl",
    );
    let reexport_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        reexport_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "E"
    ));

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type Choice { Only }\n"),
            ("middle.gcl", "import pipeline.lib::{ pub Only as sum };\n"),
            ("main.gcl", "import pipeline.middle as middle;\n"),
        ],
        "main.gcl",
    );
    let constructor_reexport_error =
        compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        constructor_reexport_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "sum"
    ));

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub type Choice { Only }\n\
                 pub dag consumer {\n\
                     import pipeline.lib::{ Only as E };\n\
                     pub node output: Dimensionless = 1.0;\n\
                 }\n",
            ),
            ("main.gcl", "include pipeline.lib.consumer()::{ output };\n"),
        ],
        "main.gcl",
    );
    let self_import_error =
        compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        self_import_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "E"
    ));
}

#[test]
fn repeated_include_producers_are_rejected_before_metadata_maps() {
    let producer = "pub node x: Dimensionless = 1.0;\n\
                    pub assert okay = @x == 2.0;\n\
                    pub plot chart = { mark: point, encode: { x: @x } };\n";
    let file_selectors = [
        "okay as first_check, okay as second_check",
        "#[expected_fail] okay as first_check, #[expected_fail] okay as second_check",
        "#[hidden] chart as first_chart, chart as second_chart",
        "x as first_value, x as second_value",
    ];
    for selectors in file_selectors {
        let main = format!(
            "include pipeline.lib()::{{ {selectors} }};\n\
             #[assumes(first_check)]\n\
             node dependent: Dimensionless = 1.0;\n"
        );
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", producer), ("main.gcl", &main)], "main.gcl");
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::DuplicateIncludeSelection { .. }),
                    ..
                }),
                ..
            })
        ));
    }

    for selectors in [
        "okay as first_check, okay as second_check",
        "#[hidden] chart as first_chart, chart as second_chart",
    ] {
        let source =
            format!("dag producer {{ {producer} }}\ninclude producer()::{{ {selectors} }};");
        let error = compile_and_eval_named(&source, "test.gcl").unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::DuplicateIncludeSelection { .. }),
                    ..
                }),
                ..
            })
        ));
    }
}

#[test]
fn unique_include_producers_remain_order_independent() {
    let producer = "dag producer {\n\
                        pub node output_a: Dimensionless = 1.0;\n\
                        pub node output_b: Dimensionless = 2.0;\n\
                    }\n";
    for selectors in [
        "output_a as x, output_b as y",
        "output_b as y, output_a as x",
    ] {
        let source = format!(
            "{producer}\n\
             include producer()::{{ {selectors} }};\n\
             node total: Dimensionless = @x + @y;"
        );
        compile_and_eval_named(&source, "test.gcl").unwrap();
    }
}

#[test]
fn lazy_attribute_is_rejected_on_declarations_and_include_items() {
    for source in [
        "#[lazy]\nnode output: Dimensionless = 1.0;",
        "#[lazy(guard)]\nnode output: Dimensionless = 1.0;",
        "#[lazy]\nassert output = true;",
        "#[lazy(guard)]\nassert output = true;",
    ] {
        let error = compile_and_eval_named(source, "test.gcl").unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                    ..
                }),
                ..
            })
        ));
    }

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub node value: Dimensionless = 1.0;\n"),
            ("main.gcl", "include pipeline.lib()::{ #[lazy] value };\n"),
        ],
        "main.gcl",
    );
    let file_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        file_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                ..
            }),
            ..
        })
    ));

    let inline_error = compile_and_eval_named(
        "dag producer { pub node value: Dimensionless = 1.0; }\n\
         include producer()::{ #[lazy(guard)] value };",
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        inline_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                ..
            }),
            ..
        })
    ));
}
