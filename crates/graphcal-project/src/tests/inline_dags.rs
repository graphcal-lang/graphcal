//! Inline DAG declarations, body imports, includes, and recursion.

use super::*;

fn setup_inline_semantics_project(
    files: &[(&str, &str)],
    root_rel: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"sem\"\n",
    )
    .expect("write manifest");
    for (rel, source) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(path, source).expect("write fixture source");
    }
    let root = dir.path().join(root_rel);
    (dir, root)
}

#[test]
fn imported_generic_field_obligation_is_checked_in_the_consumer() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub type Box<D: Dim> { Box(x: D(min: 0.5 m)) }\n",
            ),
            (
                "src/sem/main.gcl",
                "import sem.lib as lib;\n\
                 node bad: lib::Box<Time> = lib::Box<Time>(x: 1.0 s);\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn generic_argument_constraint_in_default_is_rejected_end_to_end() {
    let error = compile_and_eval(
        r"
type Wrapper<T: Type> { Wrapper(value: T) }
type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn generic_argument_constraint_in_inline_dag_type_is_rejected_end_to_end() {
    let error = compile_and_eval(
        r"
dag nested {
    type Wrapper<T: Type> { Wrapper(value: T) }
    type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
    node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
}
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn selective_import_wrong_category_preserves_marker_and_alternatives() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node JPY: Length = 1.0 m;\n\
                 pub const unit JPY: Length = 1.0 m;\n",
            ),
            ("src/sem/main.gcl", "import sem.lib::{ dim JPY };\n"),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Module(ModuleError::ImportCategoryMismatch { mismatch, .. }),
                ..
            }),
        ..
    }) = error
    else {
        panic!("expected category mismatch, got {error:?}");
    };
    assert_eq!(mismatch.name().as_str(), "JPY");
    assert_eq!(
        mismatch.expected(),
        graphcal_compiler::syntax::ast::ImportItemNamespace::Dimension
    );
    assert_eq!(
        mismatch.alternatives().as_slice(),
        &[
            graphcal_compiler::syntax::ast::ImportItemNamespace::Term,
            graphcal_compiler::syntax::ast::ImportItemNamespace::Unit,
        ]
    );
}

#[test]
fn inline_dag_required_coordinate_index_rejects_dimension_mismatch() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include pass_through(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Index(IndexError::IndexBindingDimensionMismatch {
                            dep_index,
                            expected_dim,
                            bound_index,
                            found_dim,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.as_str(), "Step");
            assert_eq!(expected_dim.to_string(), "Time");
            assert_eq!(bound_index.to_string(), "DistanceStep");
            assert_eq!(found_dim.to_string(), "Length");
        }
        other => panic!("expected IndexBindingDimensionMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_coordinate_index_rejects_named_index() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index Phase = { Start, End };
include pass_through(
    index Step: Phase,
    samples: {
        Phase#Start: 1.0 m,
        Phase#End: 2.0 m,
    },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Module(ModuleError::IndexKindMismatch {
                            dep_index,
                            dep_kind,
                            bound_index,
                            bound_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.to_string(), "Step");
            assert_eq!(dep_kind.to_string(), "coordinate");
            assert_eq!(bound_index.to_string(), "Phase");
            assert_eq!(bound_kind.to_string(), "named");
        }
        other => panic!("expected IndexKindMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_index_missing_binding_uses_index_diagnostic() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

include pass_through(samples: 1.0 m) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Index(IndexError::RequiredStaticInputNotBound {
                            name, ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.as_str(), "Step");
        }
        other => panic!("expected RequiredStaticInputNotBound, got {other:?}"),
    }
}

#[test]
fn inline_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let source = r"
dag pass_through {
    pub(bind) dim AxisDim;
    pub(bind) index Step: AxisDim;
    param samples: AxisDim[Step];
    pub node result: AxisDim[Step] = @samples;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include pass_through(
    dim AxisDim: Time,
    index Step: TimeStep,
    samples: for time: TimeStep { coord(time) },
) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn nested_dag_can_forward_compatible_required_coordinate_index() {
    let source = r"
dag inner {
    pub(bind) index InnerStep: Time;
    pub node n: Int = count(for step: InnerStep { step });
}

dag wrapper {
    pub(bind) index OuterStep: Time;
    include inner(index InnerStep: OuterStep) as inner_run;
    pub node n: Int = 1;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include wrapper(index OuterStep: TimeStep) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn file_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub(bind) dim AxisDim;
pub(bind) index Step: AxisDim;
param samples: AxisDim[Step];
pub node result: AxisDim[Step] = @samples;
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include sem.library(
    dim AxisDim: Time,
    index Step: TimeStep,
    samples: for time: TimeStep { coord(time) },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
}

#[test]
fn cross_file_inline_dag_rejects_coordinate_dimension_mismatch() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library.pass_through(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        matches!(
            result,
            Err(CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(
                        IndexError::IndexBindingDimensionMismatch { .. }
                    ),
                    ..
                }),
                ..
            }))
        ),
        "expected coordinate dimension mismatch, got {result:?}"
    );
}

#[test]
fn file_dag_index_dimension_diagnostic_uses_importer_source_span() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                "pub(bind) index Step: Time;\nparam samples: Length[Step];\n",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    let diagnostic: &dyn miette::Diagnostic = &error;
    let mut rendered = String::new();
    miette::NarratableReportHandler::new()
        .render_report(&mut rendered, diagnostic)
        .unwrap();

    assert!(rendered.contains("index Step: DistanceStep"), "{rendered}");
    assert!(!rendered.contains("OutOfBounds"), "{rendered}");
}

#[test]
fn inline_dag_rejects_parent_type_without_import() {
    let source = "\
pub dim Speed = Length / Time;

// `Speed` is deliberately not imported by the DAG body.
dag analyze {
    param v: Speed;
    pub node out: Speed = @v;
}

param input: Speed = 1.0 m/s;
node result: Speed = @analyze(v: @input)::out;
";

    let result = compile_and_eval_named(source, "test.gcl");
    assert!(
        result.is_err(),
        "inline DAG body should not inherit parent type-system names: {result:?}"
    );
}

#[test]
fn inline_dag_body_import_drives_dependency_loading() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0)::out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    assert_eq!(
        project.files().len(),
        2,
        "inline DAG body import should load its dependency"
    );
}

#[test]
fn inline_dag_body_value_import_is_available_to_call() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib as lib;

dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0)::out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 6.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn include_inside_inline_dag_body_is_available_to_call() {
    let source = "\
dag inner {
    param x: Dimensionless;
    pub node y: Dimensionless = @x + 1.0;
}

dag wrapper {
    param z: Dimensionless;
    include inner(x: @z)::{ y };
    pub node out: Dimensionless = @y + 1.0;
}

node result: Dimensionless = @wrapper(z: 2.0)::out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 4.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn nested_inline_dag_is_compilable_from_parent_body() {
    let source = "\
dag wrapper {
    dag inner {
        pub node result: Dimensionless = 4.0;
    }
    pub node out: Dimensionless = @inner()::result + 1.0;
}

node result: Dimensionless = @wrapper()::out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 5.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn same_file_inline_dag_include_rejects_private_selected_output() {
    let source = "\
dag stage {
    param thrust: Force;
    node internal_scratch: Force = @thrust * 2.0;
    pub node reported: Force = @thrust;
}

include stage(thrust: 10.0 N)::{ internal_scratch };
node used: Force = @internal_scratch;
";

    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Visibility(VisibilityError::ImportPrivateItem { name, .. }), .. }), .. })
            if name.as_str() == "internal_scratch"
    ));
}

#[test]
fn same_file_inline_dag_include_alias_hides_private_output() {
    let source = "\
dag stage {
    param thrust: Force;
    node internal_scratch: Force = @thrust * 2.0;
    pub node reported: Force = @thrust;
}

include stage(thrust: 10.0 N) as inst;
node used: Force = @inst::internal_scratch;
";

    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Visibility(VisibilityError::ImportPrivateItem { name, .. }), .. }), .. })
            if name.as_str() == "internal_scratch"
    ));
}

#[test]
fn inline_dag_include_and_call_share_body_import_semantics() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 2.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}

include scaled(x: 3.0)::{ out as included };
node called: Dimensionless = @scaled(x: 3.0)::out;
node difference: Dimensionless = @included - @called;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let included = find_value(&result, "included");
    let called = find_value(&result, "called");
    let difference = find_value(&result, "difference");
    assert!((included - 6.0).abs() < 1e-10, "included = {included}");
    assert!((called - 6.0).abs() < 1e-10, "called = {called}");
    assert!(difference.abs() < 1e-10, "difference = {difference}");
}

#[test]
fn inline_dag_include_selected_adt_output_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag make_payload {
    import sem.lib::{ type Payload, Payload };

    pub node out: Payload = Payload(x: 1.0);
}
",
            ),
            (
                "src/sem/main.gcl",
                "include sem.lib.make_payload()::{ out };\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn inline_dag_include_adt_param_default_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag use_default {
    import sem.lib::{ type Payload, Payload };

    param p: Payload = Payload(x: 1.0);
    pub node y: Dimensionless = @p.x;
}
",
            ),
            (
                "src/sem/main.gcl",
                "include sem.lib.use_default()::{ y };\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 1.0).abs() < 1e-10, "y = {y}");
}

#[test]
fn consumer_still_needs_import_for_explicit_adt_names() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}
",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib;
node p: Payload = Payload(x: 1.0);
",
            ),
        ],
        "src/sem/main.gcl",
    );

    assert!(
        compile_to_tir_project(&root, None, &fs()).is_err(),
        "consumer-written Payload names must still require an import"
    );
}

#[test]
fn inline_dag_basic_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_basic/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let val = find_value(&result, "final_result");
    assert!((val - 20.0).abs() < 1e-10, "expected 20.0, got {val}");
}

#[test]
fn inline_dag_include_selects_effective_param_value() {
    let source = "\
dag default_dag {
    param x: Dimensionless = 1.0;
}

dag bound_dag {
    param x: Dimensionless = 1.0;
}

include default_dag()::{ x as default_x };
include bound_dag(x: 2.0)::{ x as bound_x };
node combined: Dimensionless = @default_x + @bound_x;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_x") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_x") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "combined") - 3.0).abs() < 1e-10);
}

#[test]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "Graphcal source uses `{result}` as a brace-list selector, not a format arg"
)]
fn inline_dag_recursive_error() {
    // Direct recursion: dag includes itself.
    let source = r"
dag recursive {
    param x: Dimensionless;
    include recursive(x: 1.0)::{result};
    node result: Dimensionless = @x;
}
include recursive(x: 1.0)::{result};
";
    let result = compile_and_eval(source);
    assert!(result.is_err(), "recursive DAG should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("recursive DAG instantiation"),
        "error should mention recursive DAG: {err_msg}"
    );
}

#[test]
fn include_closure_cycles_are_rejected_at_the_including_declaration() {
    // Regression: a cycle through an include port binding used to pass
    // `check` and fail at evaluation with an internal error.
    let source = "dag lib { param x: Dimensionless; pub node out: Dimensionless = @x + 1.0; }\n\
                  include lib(x: @a) as inst;\n\
                  node a: Dimensionless = @inst::out;";
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { name, .. }),
                    primary: span,
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.to_string(), "a");
            assert_eq!(span.offset(), source.find("node a").unwrap());
        }
        other => panic!("expected a cyclic dependency, got {other:?}"),
    }
}

/// Message and label start of the G009 a recursive inline-DAG source reports.
fn recursive_dag_error(source: &str) -> (String, usize) {
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Graph(kind @ GraphError::RecursiveDagInstantiation { .. }),
                    primary: span,
                    ..
                }),
            ..
        })) => (kind.to_string(), span.offset()),
        other => panic!("expected a recursive DAG instantiation error, got {other:?}"),
    }
}

#[test]
fn mutually_recursive_inline_dags_report_the_cycle_from_the_first_template() {
    let source =
        "dag first { include second() as next; }\ndag second { include first() as next; }\n";
    assert_eq!(
        recursive_dag_error(source),
        (
            "recursive DAG instantiation: first -> second -> first".to_string(),
            0
        )
    );
}

#[test]
fn nested_recursive_inline_dag_is_named_by_its_path_in_the_file() {
    let source = "dag outer {\n  dag inner { include inner() as again; }\n}\n";
    assert_eq!(
        recursive_dag_error(source),
        (
            "recursive DAG instantiation: outer.inner -> outer.inner".to_string(),
            source.find("dag inner").unwrap()
        )
    );
}

#[test]
fn inline_dag_from_source() {
    // Test inline DAG from in-memory source.
    let source = "
dag add_velocities {
    param a: Velocity;
    param b: Velocity;
    pub node combined: Velocity = @a + @b;
}

param v1: Velocity = 10.0 m/s;
param v2: Velocity = 5.0 m/s;
include add_velocities(a: @v1, b: @v2)::{combined as total};
node result: Velocity = @total;
";
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!((val - 15.0).abs() < 1e-10, "expected 15.0, got {val}");
}
