//! Derived dimensions over bindable dimension ports.

use super::*;

const DERIVED_FROM_BINDABLE_DIM_DAG: &str = "dag blib {\n\
                                                   pub(bind) dim Q;\n\
                                                   pub dim QR = Q / Time;\n\
                                                   param q: Q;\n\
                                                   pub node rate: QR = @q / 2.0 s;\n\
                                               }\n";

fn expect_annotation_mismatch(source: &str) {
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation for\n{source}\ngot {other:?}"),
    }
}

#[test]
fn derived_dimension_over_bindable_dimension_is_accepted() {
    compile_and_eval(DERIVED_FROM_BINDABLE_DIM_DAG)
        .expect("a derived dimension may be defined over a required bindable dimension");

    let with_default = "dag blib {\n\
                            pub(bind) dim Q = Length;\n\
                            pub dim QR = Q / Time;\n\
                            param q: QR = 1.5 m / s;\n\
                            pub node rate: QR = @q;\n\
                        }\n\
                        include blib()::{rate, dim QR};\n\
                        node x: QR = @rate;";
    let result = compile_and_eval(with_default)
        .expect("a derived dimension over a defaulted bindable dimension is accepted");
    assert_quantity_value(&result, "x", 1.5);
}

#[test]
fn derived_dimension_over_bindable_dimension_follows_the_include_binding() {
    for include in [
        "include blib(dim Q: Length, q: 4.0 m)::{dim QR, rate};",
        "include blib(dim Q: Length, q: 4.0 m)::{rate};",
    ] {
        let source = format!(
            "{DERIVED_FROM_BINDABLE_DIM_DAG}{include}\n\
             node y: Length / Time = @rate;"
        );
        let result = compile_and_eval(&source)
            .unwrap_or_else(|error| panic!("`{include}` was rejected: {error:?}"));
        assert_quantity_value(&result, "y", 2.0);
    }

    for (projection, local) in [("dim QR", "QR"), ("dim QR as Speed", "Speed")] {
        let prefix = format!(
            "{DERIVED_FROM_BINDABLE_DIM_DAG}\
             include blib(dim Q: Length, q: 4.0 m)::{{{projection}}};\n"
        );
        let result = compile_and_eval(&format!("{prefix}node x: {local} = 1.0 m/s;"))
            .unwrap_or_else(|error| panic!("`{projection}` was not specialized: {error:?}"));
        assert_quantity_value(&result, "x", 1.0);
        expect_annotation_mismatch(&format!("{prefix}node x: {local} = 1.0 m;"));
    }
}

#[test]
fn derived_dimension_projection_from_file_module_follows_the_include_binding() {
    let library = "pub(bind) dim Q;\n\
                   pub dim QR = Q / Time;\n\
                   param q: Q;\n\
                   pub node rate: QR = @q / 2.0 s;\n";
    for (main, expected) in [
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{dim QR, rate};\n\
             node x: QR = @rate;\n\
             node y: Mass / Time = @x;\n",
            Some(2.0),
        ),
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{dim QR};\n\
             node y: QR = 1.0 m/s;\n",
            None,
        ),
    ] {
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", library), ("main.gcl", main)], "main.gcl");
        let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
        match (result, expected) {
            (Ok(result), Some(value)) => assert_quantity_value(&result, "y", value),
            (
                Err(CompileError::Eval(RenderedSemanticError {
                    error:
                        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                            kind:
                                SemanticErrorKind::Dimension(
                                    DimensionError::DimensionMismatchInAnnotation { .. },
                                ),
                            ..
                        }),
                    ..
                })),
                None,
            ) => {}
            (other, _) => panic!("unexpected result for\n{main}\n{other:?}"),
        }
    }
}

/// A template whose bindable dimension port has a default, with a derived
/// dimension over it.
const DEFAULTED_BINDABLE_DIM_LIBRARY: &str = "pub(bind) dim Q = Length;\n\
                                              pub dim QR = Q / Time;\n\
                                              param q: Q;\n\
                                              pub node doubled: Q = @q * 2.0;\n\
                                              pub node rate: QR = @q / 2.0 s;\n";

#[test]
fn defaulted_bindable_dimension_follows_the_include_binding_in_inline_dags() {
    let library = format!("dag blib {{\n{DEFAULTED_BINDABLE_DIM_LIBRARY}}}\n");
    for (include, output, value) in [
        // Rebinding the defaulted port retypes every use and derived dimension.
        (
            "include blib(dim Q: Mass, q: 4.0 kg)::{doubled, rate};",
            "node x: Mass = @doubled;\nnode y: Mass / Time = @rate;",
            (8.0, 2.0),
        ),
        // Without a binding the default applies.
        (
            "include blib(q: 4.0 m)::{doubled, rate};",
            "node x: Length = @doubled;\nnode y: Length / Time = @rate;",
            (8.0, 2.0),
        ),
    ] {
        let source = format!("{library}{include}\n{output}");
        let result = compile_and_eval(&source)
            .unwrap_or_else(|error| panic!("`{include}` was rejected: {error:?}"));
        assert_quantity_value(&result, "x", value.0);
        assert_quantity_value(&result, "y", value.1);
    }
    // The rebound port rejects a value of the default dimension.
    assert!(
        compile_and_eval(&format!(
            "{library}include blib(dim Q: Mass, q: 4.0 m)::{{doubled}};\nnode x: Mass = @doubled;"
        ))
        .is_err()
    );
    // A projected derived dimension follows the rebinding too.
    let prefix = format!("{library}include blib(dim Q: Mass, q: 4.0 kg)::{{dim QR}};\n");
    let result = compile_and_eval(&format!("{prefix}node x: QR = 1.0 kg/s;"))
        .expect("the projected QR is Mass / Time");
    assert_quantity_value(&result, "x", 1.0);
    expect_annotation_mismatch(&format!("{prefix}node x: QR = 1.0 m/s;"));
}

#[test]
fn defaulted_bindable_dimension_follows_the_include_binding_in_file_modules() {
    for (main, expected) in [
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{doubled, rate};\n\
             node x: Mass = @doubled;\n\
             node y: Mass / Time = @rate;\n",
            Some((8.0, 2.0)),
        ),
        (
            "include pipeline.lib(q: 4.0 m)::{doubled, rate};\n\
             node x: Length = @doubled;\n\
             node y: Length / Time = @rate;\n",
            Some((8.0, 2.0)),
        ),
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{doubled};\n\
             node x: Length = @doubled;\n",
            None,
        ),
    ] {
        let (_directory, root) = write_pipeline_project(
            &[
                ("lib.gcl", DEFAULTED_BINDABLE_DIM_LIBRARY),
                ("main.gcl", main),
            ],
            "main.gcl",
        );
        let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
        match (result, expected) {
            (Ok(result), Some((x, y))) => {
                assert_quantity_value(&result, "x", x);
                assert_quantity_value(&result, "y", y);
            }
            (
                Err(CompileError::Eval(RenderedSemanticError {
                    error:
                        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                            kind:
                                SemanticErrorKind::Dimension(
                                    DimensionError::DimensionMismatchInAnnotation { .. },
                                ),
                            ..
                        }),
                    ..
                })),
                None,
            ) => {}
            (other, _) => panic!("unexpected result for\n{main}\n{other:?}"),
        }
    }
}

#[test]
fn defaulted_bindable_dimension_rebinding_passes_through_nested_includes() {
    let source = "dag inner {\n\
                      pub(bind) dim P = Length;\n\
                      param p: P;\n\
                      pub node twice: P = @p * 2.0;\n\
                  }\n\
                  dag wrap {\n\
                      pub(bind) dim Q = Length;\n\
                      param q: Q;\n\
                      include inner(dim P: Q, p: @q)::{twice};\n\
                      pub node out: Q = @twice;\n\
                  }\n\
                  include wrap(dim Q: Mass, q: 3.0 kg)::{out};\n\
                  node r: Mass = @out;";
    let result = compile_and_eval(source).expect("the rebinding reaches the nested include");
    assert_quantity_value(&result, "r", 6.0);
}

#[test]
fn rebound_defaulted_dimension_port_types_projected_plot_channels() {
    let source = "dag lib {\n\
                      pub(bind) dim Q = Length;\n\
                      pub index I = { A, B };\n\
                      param q: Q;\n\
                      pub node xs: Q[I] = for i: I { @q };\n\
                      pub node ys: Dimensionless[I] = for i: I { 1.0 };\n\
                      pub plot p = { mark: line, encode: { x: @xs, y: @ys } };\n\
                  }\n\
                  include lib(dim Q: Mass, q: 3.0 kg)::{p};";
    let result = compile_and_eval(source).expect("the projected plot checks");
    let [plot] = result.plots.as_slice() else {
        panic!("expected one projected plot, got {:?}", result.plots);
    };
    let x = plot
        .encoding_meta
        .iter()
        .find(|(channel, _)| *channel == graphcal_compiler::syntax::ast::EncodingChannel::X)
        .map(|(_, metadata)| metadata);
    assert_eq!(
        x.and_then(|metadata| metadata.dimension_label.as_deref()),
        Some("Mass")
    );
}

#[test]
fn projected_type_fields_follow_bound_ports_through_unprojected_derived_dimensions() {
    // `QR` is not projected, yet the projected `Rate` names it in a field:
    // the field follows the include's binding of the port `QR` is defined over.
    for (port, binding, accepted, rejected) in [
        ("pub(bind) dim Q;", "dim Q: Length", "1.0 m/s", "1.0 kg/s"),
        (
            "pub(bind) dim Q = Length;",
            "dim Q: Mass",
            "1.0 kg/s",
            "1.0 m/s",
        ),
    ] {
        let program = |value: &str| {
            format!(
                "dag blib {{\n\
                     {port}\n\
                     pub dim QR = Q / Time;\n\
                     pub type Rate {{ Rate(v: QR) }}\n\
                 }}\n\
                 include blib({binding})::{{type Rate, Rate}};\n\
                 node r: Rate = Rate(v: {value});"
            )
        };
        compile_and_eval(&program(accepted))
            .unwrap_or_else(|error| panic!("`{port}` with `{binding}` rejected: {error:?}"));
        assert!(
            compile_and_eval(&program(rejected)).is_err(),
            "`{port}` with `{binding}` accepted {rejected}"
        );
    }
}

#[test]
fn template_bodies_may_use_derived_dimensions_over_defaulted_ports() {
    // A body typed by a derived dimension over a defaulted port does not
    // observe the port's default (no V007), whether or not it is bound.
    let library = format!("dag blib {{\n{DEFAULTED_BINDABLE_DIM_LIBRARY}}}\n");
    let source =
        format!("{library}include blib(q: 4.0 m)::{{rate}};\nnode y: Length / Time = @rate;");
    let result = compile_and_eval(&source).expect("no false V007 for a derived dimension");
    assert_quantity_value(&result, "y", 2.0);
}

#[test]
fn included_dag_dimensions_do_not_shadow_importer_dimensions() {
    let prefix = "dag blib {\n\
                      pub dim R = Length;\n\
                      param q: R;\n\
                      pub node rate: R = @q;\n\
                  }\n\
                  dim R = Mass;\n\
                  include blib(q: 4.0 m) as b;\n";
    let result = compile_and_eval(&format!("{prefix}node x: R = 1.0 kg;"))
        .expect("the importer's own dimension keeps its definition");
    assert_quantity_value(&result, "x", 1.0);
    expect_annotation_mismatch(&format!("{prefix}node x: R = 1.0 m;"));
}

#[test]
fn constructor_empty_parentheses_are_rejected() {
    for (source, expected_constructor) in [
        (
            "type Status { Nominal, Coast }\nnode status: Status = Coast();",
            "Coast",
        ),
        (
            "type Status { Nominal, Coast }\n\
             node status: Status = Coast;\n\
             node result: Dimensionless = match @status {\n\
                 Nominal => 0.0,\n\
                 Coast() => 1.0,\n\
             };",
            "Coast",
        ),
        (
            "type Reading { Missing, Present(value: Length) }\n\
             node reading: Reading = Present();",
            "Present",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(StructError::EmptyParenthesizedConstructor { constructor, .. }), .. }), .. }) if constructor.as_str() == expected_constructor
            ),
            "unexpected error: {error:?}"
        );
    }
}
