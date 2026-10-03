use std::sync::Arc;

use crate::function_signature::FunctionSignature;
use crate::ir::lower::{LoweredTestFile, lower_file_with_inline_dags_for_test};
use crate::ir::model::HirDag;
use crate::semantic_error::SemanticError;
use crate::semantic_error::SemanticErrorKind;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::plugin::PluginError;

use super::ExternStructResult;

fn lower(source: &str) -> Result<LoweredTestFile, SemanticError> {
    let parsed = crate::syntax::parser::Parser::new(source)
        .parse_file()
        .expect("source parses");
    let ast = crate::desugar::desugared_ast::File::from(parsed);
    let src = crate::source_registry::SourceRegistry::new()
        .register("main.gcl", Arc::new(source.to_string()));
    lower_file_with_inline_dags_for_test(&ast, "main.gcl", src)
}

fn only_signature(dag: &HirDag) -> &FunctionSignature<ExternStructResult> {
    let [(_, entry)] = dag.extern_functions().iter().collect::<Vec<_>>()[..] else {
        panic!("expected exactly one extern function");
    };
    &entry.signature
}

fn expect_invalid_signature(source: &str, fragment: &str) {
    match lower(source) {
        Err(SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Plugin(PluginError::InvalidExternSignature { error, .. }),
            ..
        })) => {
            let message = error.to_string();
            assert!(message.contains(fragment), "{message}");
        }
        Err(other) => panic!("expected an invalid extern signature, got {other:?}"),
        Ok(_) => panic!("expected an invalid extern signature"),
    }
}

/// Dimensions resolve through the declaring module's resolver scope, so a
/// renamed selective import (`dim Rate as R`) and a module-qualified name
/// denote the same canonical dimension as the spelled-out expression.
#[test]
fn imported_dimensions_resolve_canonically() {
    let lowered = lower(
        "pub dim Rate = Length / Time;\n\
         import plugin \"graphcal:demo\" as direct { fn f(x: Length / Time) -> Rate; }\n\
         dag renamed {\n\
           import p.main::{dim Rate as R};\n\
           import plugin \"graphcal:demo\" as demo { fn f(x: R) -> R; }\n\
         }\n\
         dag qualified {\n\
           import p.main as m;\n\
           import plugin \"graphcal:demo\" as demo { fn f(x: m::Rate) -> m::Rate; }\n\
         }\n",
    )
    .unwrap();
    let direct = only_signature(&lowered.root);
    for inline in &lowered.inline_dags {
        assert!(
            only_signature(inline).structurally_equivalent(direct),
            "{:?}",
            inline.dag_id()
        );
    }
}

/// `Dimensionless` is the identity term in extern signatures, matching the
/// plugin SDK's dimension vocabulary.
#[test]
fn dimensionless_terms_are_the_identity_dimension() {
    let with_identity = lower(
        "import plugin \"graphcal:demo\" as demo {\n\
           fn f<D: Dim>(x: Dimensionless / Time, y: D * Dimensionless) -> Dimensionless^2;\n\
         }\n",
    )
    .unwrap();
    let without_identity = lower(
        "import plugin \"graphcal:demo\" as demo {\n\
           fn f<D: Dim>(x: Time^-1, y: D) -> Dimensionless;\n\
         }\n",
    )
    .unwrap();
    assert!(
        only_signature(&with_identity.root)
            .structurally_equivalent(only_signature(&without_identity.root))
    );
}

#[test]
fn binders_are_generic_parameters_of_the_function() {
    let lowered = lower(
        "import plugin \"graphcal:demo\" as demo {\n\
           fn scale<D: Dim, I: Index>(x: D[I], s: D / Time) -> D[I];\n\
         }\n",
    )
    .unwrap();
    let signature = only_signature(&lowered.root);
    assert_eq!(
        signature
            .dim_vars()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["D"]
    );
    assert_eq!(
        signature
            .index_vars()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["I"]
    );
}

#[test]
fn binders_share_one_namespace_and_keep_their_sort() {
    expect_invalid_signature(
        "import plugin \"graphcal:demo\" as demo { fn f<D: Dim, D: Index>(x: D) -> D; }\n",
        "generic binder `D` is declared more than once",
    );
    expect_invalid_signature(
        "import plugin \"graphcal:demo\" as demo { fn f<D: Dim>(x: Length[D]) -> Length; }\n",
        "extern array axes must name the signature's `Index` binders",
    );
}

#[test]
fn unknown_dimensions_are_reported_at_the_term() {
    assert!(matches!(
        lower("import plugin \"graphcal:demo\" as demo { fn f(x: Missing * Length) -> Length; }\n"),
        Err(SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. })) if name.to_string() == "Missing"
    ));
}
