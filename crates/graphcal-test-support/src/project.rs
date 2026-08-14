//! Typed project models, source rendering, transformations, and shrink-friendly strategies.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use graphcal_compiler::dimension::PreludeBaseDimension;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::{DimName, UnitName};
use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName};
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::names::NameAtom;
use proptest::prelude::*;

/// Hard bounds shared by Proptest and fuzz-derived project generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationLimits {
    /// Maximum expression-tree depth.
    pub expression_depth: u8,
    /// Maximum declarations rendered into one generated file.
    pub declarations_per_file: u8,
    /// Maximum source files in one generated project.
    pub files: u8,
    /// Maximum named variants in one index.
    pub index_variants: u8,
}

impl GenerationLimits {
    /// Conservative bounds suitable for pull-request tests and fuzz targets.
    pub const SMOKE: Self = Self {
        expression_depth: 4,
        declarations_per_file: 16,
        files: 3,
        index_variants: 4,
    };

    /// Deeper bounds for long local or nightly fuzz campaigns.
    pub const DEEP: Self = Self {
        expression_depth: 7,
        declarations_per_file: 32,
        files: 5,
        index_variants: 8,
    };
}

/// One generated Graphcal source project before rendering to text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedProject {
    package: PackageName,
    root: ModuleFile,
    modules: Vec<ModuleFile>,
    expected: ExpectedArtifact,
}

impl GeneratedProject {
    /// Build a valid single-file project containing dimensions, units, and an index.
    #[must_use]
    pub fn single_file(expression: DimensionlessExpr) -> Self {
        let result = expression.evaluate();
        Self::single_file_with_result(
            Declaration::Value {
                visibility: Visibility::Public,
                name: DeclName::expect_valid("result"),
                expression,
            },
            ExpectedArtifact::DimensionlessInteger {
                name: DeclName::expect_valid("result"),
                value: result,
            },
        )
    }

    /// Build a valid project whose generated value retains an explicit display unit.
    #[must_use]
    pub fn presented(expression: DimensionlessExpr) -> Self {
        let result = expression.evaluate();
        Self::single_file_with_result(
            Declaration::PresentedValue {
                visibility: Visibility::Public,
                name: DeclName::expect_valid("result"),
                dimension: DimensionRef::User(DimName::expect_valid("GeneratedMeasure")),
                expression,
                source_unit: UnitName::expect_valid("gm"),
                display_unit: UnitName::expect_valid("kgm"),
            },
            ExpectedArtifact::PresentedQuantity {
                name: DeclName::expect_valid("result"),
                si_value: result,
                display_unit: UnitName::expect_valid("kgm"),
                display_scale: 1_000,
            },
        )
    }

    fn single_file_with_result(result: Declaration, expected: ExpectedArtifact) -> Self {
        Self {
            package: PackageName::trusted("generated"),
            root: ModuleFile {
                path: ModulePath::root(PackageName::trusted("generated")),
                declarations: vec![
                    Declaration::BaseDimension {
                        name: DimName::expect_valid("GeneratedMeasure"),
                        unit: UnitName::expect_valid("gm"),
                    },
                    Declaration::DerivedDimension {
                        name: DimName::expect_valid("GeneratedRate"),
                        numerator: DimensionRef::User(DimName::expect_valid("GeneratedMeasure")),
                        denominator: DimensionRef::Prelude(PreludeBaseDimension::Time),
                    },
                    Declaration::ScaledUnit {
                        name: UnitName::expect_valid("kgm"),
                        dimension: DimensionRef::User(DimName::expect_valid("GeneratedMeasure")),
                        scale: 1_000,
                        base: UnitName::expect_valid("gm"),
                    },
                    Declaration::Index {
                        name: IndexName::expect_valid("GeneratedAxis"),
                        variants: vec![
                            IndexVariantName::expect_valid("First"),
                            IndexVariantName::expect_valid("Second"),
                        ],
                    },
                    Declaration::IndexedValues {
                        name: DeclName::expect_valid("samples"),
                        index: IndexName::expect_valid("GeneratedAxis"),
                        entries: vec![1, 2],
                    },
                    result,
                ],
            },
            modules: Vec::new(),
            expected,
        }
    }

    /// Build a valid multi-owner project with two same-leaf declarations.
    #[must_use]
    pub fn multi_owner(left: DimensionlessExpr, right: DimensionlessExpr) -> Self {
        let package = PackageName::trusted("generated");
        let left_path = ModulePath::module(package.clone(), ModuleName::trusted("left"));
        let right_path = ModulePath::module(package.clone(), ModuleName::trusted("right"));
        let left_alias = ModuleAliasName::expect_valid("left_owner");
        let right_alias = ModuleAliasName::expect_valid("right_owner");
        let result = left.evaluate() + right.evaluate();
        Self {
            package: package.clone(),
            root: ModuleFile {
                path: ModulePath::root(package),
                declarations: vec![
                    Declaration::Import {
                        target: left_path.clone(),
                        alias: left_alias.clone(),
                    },
                    Declaration::Import {
                        target: right_path.clone(),
                        alias: right_alias.clone(),
                    },
                    Declaration::QualifiedSum {
                        visibility: Visibility::Public,
                        name: DeclName::expect_valid("result"),
                        left: QualifiedValueRef {
                            alias: left_alias,
                            name: DeclName::expect_valid("shared"),
                        },
                        right: QualifiedValueRef {
                            alias: right_alias,
                            name: DeclName::expect_valid("shared"),
                        },
                    },
                ],
            },
            modules: vec![
                owner_module(left_path, left),
                owner_module(right_path, right),
            ],
            expected: ExpectedArtifact::DimensionlessInteger {
                name: DeclName::expect_valid("result"),
                value: result,
            },
        }
    }

    /// The bounded semantic artifact expected after evaluation.
    #[must_use]
    pub const fn expected(&self) -> &ExpectedArtifact {
        &self.expected
    }

    /// Render the project at the text/filesystem boundary.
    #[must_use]
    pub fn render(&self) -> RenderedProject {
        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("graphcal.toml"),
            format!("[package]\nname = \"{}\"\n", self.package.as_str()),
        );
        for module in std::iter::once(&self.root).chain(&self.modules) {
            files.insert(module.path.source_path(), module.render());
        }
        RenderedProject {
            root: self.root.path.source_path(),
            files,
        }
    }

    /// Rename one import alias and every typed reference to it.
    #[must_use]
    pub fn rename_alias(&self, from: &ModuleAliasName, to: &ModuleAliasName) -> Self {
        Self {
            package: self.package.clone(),
            root: self.root.rename_alias(from, to),
            modules: self.modules.clone(),
            expected: self.expected.clone(),
        }
    }

    /// Reverse independent owner modules without changing the root imports.
    #[must_use]
    pub fn reverse_module_storage(&self) -> Self {
        let mut modules = self.modules.clone();
        modules.reverse();
        Self {
            package: self.package.clone(),
            root: self.root.clone(),
            modules,
            expected: self.expected.clone(),
        }
    }

    /// Introduce one controlled dimension mismatch at the result declaration.
    #[must_use]
    pub fn with_dimension_mismatch(&self) -> InvalidGeneratedProject {
        InvalidGeneratedProject {
            project: self.clone(),
            mutation: InvalidMutation::ResultDimensionMismatch,
            expected_error: SemanticErrorClass::DimensionMismatch,
        }
    }
}

fn owner_module(path: ModulePath, expression: DimensionlessExpr) -> ModuleFile {
    ModuleFile {
        path,
        declarations: vec![Declaration::ConstValue {
            visibility: Visibility::Public,
            name: DeclName::expect_valid("shared"),
            expression,
        }],
    }
}

/// Rendered project files ready for an in-memory or real filesystem shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedProject {
    root: PathBuf,
    files: BTreeMap<PathBuf, String>,
}

impl RenderedProject {
    /// Root source path, relative to the project directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Deterministically ordered relative file paths and contents.
    pub fn files(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.files
            .iter()
            .map(|(path, source)| (path.as_path(), source.as_str()))
    }

    /// Source text of the root module.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "private rendering construction always inserts the root"
    )]
    pub fn root_source(&self) -> &str {
        self.files
            .get(&self.root)
            .expect("rendered project must contain its root")
    }
}

/// A generated valid project after one typed invalidating transform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidGeneratedProject {
    project: GeneratedProject,
    mutation: InvalidMutation,
    expected_error: SemanticErrorClass,
}

impl InvalidGeneratedProject {
    /// Render the intentionally invalid project.
    #[must_use]
    pub fn render(&self) -> RenderedProject {
        let mut rendered = self.project.render();
        match self.mutation {
            InvalidMutation::ResultDimensionMismatch => {
                let source = rendered
                    .files
                    .get_mut(&rendered.root)
                    .expect("rendered project must contain its root");
                *source = source.replacen(
                    "pub node result: Dimensionless =",
                    "pub node result: Length =",
                    1,
                );
            }
        }
        rendered
    }

    /// Semantic diagnostic class the controlled transform must produce.
    #[must_use]
    pub const fn expected_error(&self) -> SemanticErrorClass {
        self.expected_error
    }
}

/// Stable semantic class expected from an invalidating transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticErrorClass {
    /// A checked expression disagrees with its declared physical dimension.
    DimensionMismatch,
}

/// Small independent result oracle retained by the typed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedArtifact {
    /// One public dimensionless integer-valued node.
    DimensionlessInteger { name: DeclName, value: i64 },
    /// One physical quantity with an explicit checked display projection.
    PresentedQuantity {
        name: DeclName,
        si_value: i64,
        display_unit: UnitName,
        display_scale: i64,
    },
}

/// A bounded, dimensionless integer expression with an independent evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimensionlessExpr {
    /// Integer literal represented as a dimensionless quantity literal.
    Literal(i16),
    /// Addition of dimensionless values.
    Add(Box<Self>, Box<Self>),
    /// Multiplication of dimensionless values.
    Multiply(Box<Self>, Box<Self>),
}

impl DimensionlessExpr {
    /// Evaluate using integer arithmetic independent of Graphcal's evaluator.
    #[must_use]
    pub fn evaluate(&self) -> i64 {
        match self {
            Self::Literal(value) => i64::from(*value),
            Self::Add(left, right) => left.evaluate() + right.evaluate(),
            Self::Multiply(left, right) => left.evaluate() * right.evaluate(),
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Literal(value) => format!("{value}.0"),
            Self::Add(left, right) => format!("({} + {})", left.render(), right.render()),
            Self::Multiply(left, right) => format!("({} * {})", left.render(), right.render()),
        }
    }
}

/// Shrink-friendly strategy shared by ordinary property tests and fuzz targets.
pub fn dimensionless_expr_strategy(limits: GenerationLimits) -> BoxedStrategy<DimensionlessExpr> {
    (-8i16..=8)
        .prop_map(DimensionlessExpr::Literal)
        .prop_recursive(u32::from(limits.expression_depth), 64, 2, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                    DimensionlessExpr::Add(Box::new(left), Box::new(right))
                }),
                (inner.clone(), inner).prop_map(|(left, right)| {
                    DimensionlessExpr::Multiply(Box::new(left), Box::new(right))
                }),
            ]
        })
        .boxed()
}

/// Valid single-file project strategy.
pub fn single_file_project_strategy(limits: GenerationLimits) -> BoxedStrategy<GeneratedProject> {
    dimensionless_expr_strategy(limits)
        .prop_map(GeneratedProject::single_file)
        .boxed()
}

/// Valid project strategy with an independently checked display projection.
pub fn presentation_project_strategy(limits: GenerationLimits) -> BoxedStrategy<GeneratedProject> {
    dimensionless_expr_strategy(limits)
        .prop_map(GeneratedProject::presented)
        .boxed()
}

/// Valid multi-owner project strategy with same-leaf declarations.
pub fn multi_owner_project_strategy(limits: GenerationLimits) -> BoxedStrategy<GeneratedProject> {
    (
        dimensionless_expr_strategy(limits),
        dimensionless_expr_strategy(limits),
    )
        .prop_map(|(left, right)| GeneratedProject::multi_owner(left, right))
        .boxed()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageName(NameAtom);

impl PackageName {
    fn trusted(name: &str) -> Self {
        Self(NameAtom::parse(name).expect("trusted package name"))
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleName(NameAtom);

impl ModuleName {
    fn trusted(name: &str) -> Self {
        Self(NameAtom::parse(name).expect("trusted module name"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModulePath {
    package: PackageName,
    module: ModuleName,
}

impl ModulePath {
    fn root(package: PackageName) -> Self {
        Self {
            package,
            module: ModuleName::trusted("main"),
        }
    }

    const fn module(package: PackageName, module: ModuleName) -> Self {
        Self { package, module }
    }

    fn source_path(&self) -> PathBuf {
        PathBuf::from("src")
            .join(self.package.as_str())
            .join(format!("{}.gcl", self.module.0.as_str()))
    }

    fn render(&self) -> String {
        format!("{}.{}", self.package.as_str(), self.module.0.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleFile {
    path: ModulePath,
    declarations: Vec<Declaration>,
}

impl ModuleFile {
    fn render(&self) -> String {
        self.declarations
            .iter()
            .map(Declaration::render)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn rename_alias(&self, from: &ModuleAliasName, to: &ModuleAliasName) -> Self {
        Self {
            path: self.path.clone(),
            declarations: self
                .declarations
                .iter()
                .map(|declaration| declaration.rename_alias(from, to))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Public,
}

impl Visibility {
    const fn source_prefix(self) -> &'static str {
        match self {
            Self::Public => "pub ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DimensionRef {
    Prelude(PreludeBaseDimension),
    User(DimName),
}

impl DimensionRef {
    fn render(&self) -> &str {
        match self {
            Self::Prelude(value) => value.as_str(),
            Self::User(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QualifiedValueRef {
    alias: ModuleAliasName,
    name: DeclName,
}

impl QualifiedValueRef {
    fn render(&self) -> String {
        format!("@{}.{}", self.alias, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Declaration {
    BaseDimension {
        name: DimName,
        unit: UnitName,
    },
    DerivedDimension {
        name: DimName,
        numerator: DimensionRef,
        denominator: DimensionRef,
    },
    ScaledUnit {
        name: UnitName,
        dimension: DimensionRef,
        scale: i64,
        base: UnitName,
    },
    Index {
        name: IndexName,
        variants: Vec<IndexVariantName>,
    },
    IndexedValues {
        name: DeclName,
        index: IndexName,
        entries: Vec<i64>,
    },
    Import {
        target: ModulePath,
        alias: ModuleAliasName,
    },
    Value {
        visibility: Visibility,
        name: DeclName,
        expression: DimensionlessExpr,
    },
    PresentedValue {
        visibility: Visibility,
        name: DeclName,
        dimension: DimensionRef,
        expression: DimensionlessExpr,
        source_unit: UnitName,
        display_unit: UnitName,
    },
    ConstValue {
        visibility: Visibility,
        name: DeclName,
        expression: DimensionlessExpr,
    },
    QualifiedSum {
        visibility: Visibility,
        name: DeclName,
        left: QualifiedValueRef,
        right: QualifiedValueRef,
    },
}

impl Declaration {
    fn render(&self) -> String {
        match self {
            Self::BaseDimension { name, unit } => {
                format!("pub base dim {name};\nbase unit {unit}: {name};")
            }
            Self::DerivedDimension {
                name,
                numerator,
                denominator,
            } => format!(
                "pub dim {name} = {} / {};",
                numerator.render(),
                denominator.render()
            ),
            Self::ScaledUnit {
                name,
                dimension,
                scale,
                base,
            } => format!(
                "const unit {name}: {} = {scale}.0 {base};",
                dimension.render()
            ),
            Self::Index { name, variants } => format!(
                "pub index {name} = {{ {} }};",
                variants
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::IndexedValues {
                name,
                index,
                entries,
            } => {
                let mut source = format!("param {name}: Dimensionless[{index}] = {{\n");
                for (position, value) in entries.iter().enumerate() {
                    let variant = match position {
                        0 => "First",
                        _ => "Second",
                    };
                    writeln!(source, "    {index}.{variant}: {value}.0,")
                        .expect("writing to String cannot fail");
                }
                source.push_str("};");
                source
            }
            Self::Import { target, alias } => {
                format!("import {} as {alias};", target.render())
            }
            Self::Value {
                visibility,
                name,
                expression,
            } => format!(
                "{}node {name}: Dimensionless = {};",
                visibility.source_prefix(),
                expression.render()
            ),
            Self::PresentedValue {
                visibility,
                name,
                dimension,
                expression,
                source_unit,
                display_unit,
            } => format!(
                "{}node {name}: {} = ({}) * 1.0 {source_unit} -> {display_unit};",
                visibility.source_prefix(),
                dimension.render(),
                expression.render()
            ),
            Self::ConstValue {
                visibility,
                name,
                expression,
            } => format!(
                "{}const node {name}: Dimensionless = {};",
                visibility.source_prefix(),
                expression.render()
            ),
            Self::QualifiedSum {
                visibility,
                name,
                left,
                right,
            } => format!(
                "{}node {name}: Dimensionless = {} + {};",
                visibility.source_prefix(),
                left.render(),
                right.render()
            ),
        }
    }

    fn rename_alias(&self, from: &ModuleAliasName, to: &ModuleAliasName) -> Self {
        match self {
            Self::Import { target, alias } if alias == from => Self::Import {
                target: target.clone(),
                alias: to.clone(),
            },
            Self::QualifiedSum {
                visibility,
                name,
                left,
                right,
            } => Self::QualifiedSum {
                visibility: *visibility,
                name: name.clone(),
                left: QualifiedValueRef {
                    alias: if &left.alias == from {
                        to.clone()
                    } else {
                        left.alias.clone()
                    },
                    name: left.name.clone(),
                },
                right: QualifiedValueRef {
                    alias: if &right.alias == from {
                        to.clone()
                    } else {
                        right.alias.clone()
                    },
                    name: right.name.clone(),
                },
            },
            other => other.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvalidMutation {
    ResultDimensionMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_renderer_keeps_typed_features() {
        let project = GeneratedProject::single_file(DimensionlessExpr::Add(
            Box::new(DimensionlessExpr::Literal(2)),
            Box::new(DimensionlessExpr::Literal(3)),
        ));
        let source = project.render().root_source().to_string();
        assert!(source.contains("base dim GeneratedMeasure"));
        assert!(source.contains("dim GeneratedRate"));
        assert!(source.contains("index GeneratedAxis"));
        assert!(source.contains("pub node result: Dimensionless"));
        assert_eq!(
            project.expected(),
            &ExpectedArtifact::DimensionlessInteger {
                name: DeclName::expect_valid("result"),
                value: 5,
            }
        );
    }

    #[test]
    fn alias_renaming_updates_import_and_typed_references() {
        let project = GeneratedProject::multi_owner(
            DimensionlessExpr::Literal(1),
            DimensionlessExpr::Literal(2),
        );
        let renamed = project.rename_alias(
            &ModuleAliasName::expect_valid("left_owner"),
            &ModuleAliasName::expect_valid("renamed_owner"),
        );
        let source = renamed.render().root_source().to_string();
        assert!(source.contains("as renamed_owner"));
        assert!(source.contains("@renamed_owner.shared"));
        assert!(!source.contains("left_owner"));
        assert_eq!(renamed.expected(), project.expected());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn generated_expressions_match_the_independent_oracle(
            expression in dimensionless_expr_strategy(GenerationLimits::SMOKE)
        ) {
            let project = GeneratedProject::single_file(expression.clone());
            prop_assert_eq!(
                project.expected(),
                &ExpectedArtifact::DimensionlessInteger {
                    name: DeclName::expect_valid("result"),
                    value: expression.evaluate(),
                }
            );
        }
    }
}
