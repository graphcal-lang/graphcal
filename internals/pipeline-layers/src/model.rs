use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Package {
    Compiler,
    Eval,
}

impl Package {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "compiler" | "graphcal_compiler" => Some(Self::Compiler),
            "eval" | "graphcal_eval" => Some(Self::Eval),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::Eval => "eval",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModuleId {
    pub package: Package,
    pub path: Vec<String>,
}

impl ModuleId {
    pub fn new(package: Package, path: Vec<String>) -> Self {
        Self { package, path }
    }

    pub fn display(&self) -> String {
        let mut out = self.package.name().to_owned();
        self.path.iter().for_each(|part| {
            out.push_str("::");
            out.push_str(part);
        });
        out
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Role {
    Contracts,
    Checking,
    Interpreter,
    Loading,
    Facade,
}

impl Role {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "contracts" => Some(Self::Contracts),
            "checking" => Some(Self::Checking),
            "interpreter" => Some(Self::Interpreter),
            "loading" => Some(Self::Loading),
            "facade" => Some(Self::Facade),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Contracts => "contracts",
            Self::Checking => "checking",
            Self::Interpreter => "interpreter",
            Self::Loading => "loading",
            Self::Facade => "facade",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceForm {
    ModuleDeclaration,
    UseImport,
    UseGlob,
    QualifiedPath,
    MacroInvocation,
    MacroTokenPath,
}

impl EvidenceForm {
    pub fn name(self) -> &'static str {
        match self {
            Self::ModuleDeclaration => "module declaration",
            Self::UseImport => "use import",
            Self::UseGlob => "use glob",
            Self::QualifiedPath => "qualified path",
            Self::MacroInvocation => "macro invocation",
            Self::MacroTokenPath => "macro token path",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EdgeKey {
    pub from: ModuleId,
    pub to: ModuleId,
    pub test_only: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Evidence {
    pub file: PathBuf,
    pub line: usize,
    pub form: EvidenceForm,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Edge {
    pub key: EdgeKey,
    pub evidence: Vec<Evidence>,
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}
