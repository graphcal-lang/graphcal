use std::path::Path;

use super::{ModuleFilePath, ModuleFilePathError, ModuleNameError};

fn module_path(relative: &str) -> Result<String, ModuleFilePathError> {
    ModuleFilePath::parse(Path::new(relative)).map(|file| file.module_path().to_string())
}

#[test]
fn valid_module_files_name_their_module() {
    for (relative, expected) in [
        ("pkg.gcl", "pkg"),
        ("pkg/lib.gcl", "pkg.lib"),
        ("nasa/rocket/dynamics_2.gcl", "nasa.rocket.dynamics_2"),
    ] {
        assert_eq!(module_path(relative).as_deref(), Ok(expected), "{relative}");
    }
}

#[test]
fn invalid_names_are_rejected() {
    for (relative, name, reason) in [
        (
            "pkg/my-helpers.gcl",
            "my-helpers",
            ModuleNameError::NotIdentifier,
        ),
        ("pkg/2d.gcl", "2d", ModuleNameError::NotIdentifier),
        ("pkg/a.b.gcl", "a.b", ModuleNameError::NotIdentifier),
        (
            "pkg/MyModule.gcl",
            "MyModule",
            ModuleNameError::NotLowerSnakeCase,
        ),
        ("pkg/Sub/lib.gcl", "Sub", ModuleNameError::NotLowerSnakeCase),
        (
            "pkg/_private.gcl",
            "_private",
            ModuleNameError::NotIdentifier,
        ),
        ("pkg/match.gcl", "match", ModuleNameError::Keyword),
    ] {
        assert_eq!(
            module_path(relative),
            Err(ModuleFilePathError::InvalidName {
                name: name.to_owned(),
                reason,
            }),
            "{relative}"
        );
    }
}

#[test]
fn non_module_paths_are_rejected() {
    for (relative, error) in [
        ("pkg/notes.txt", ModuleFilePathError::NotGcl),
        ("pkg/lib", ModuleFilePathError::NotGcl),
        ("pkg/.gcl", ModuleFilePathError::NotGcl),
        ("pkg/lib.GCL", ModuleFilePathError::NotGcl),
        ("", ModuleFilePathError::NotRelative),
        ("../pkg.gcl", ModuleFilePathError::NotRelative),
        ("/pkg.gcl", ModuleFilePathError::NotRelative),
    ] {
        assert_eq!(module_path(relative), Err(error), "{relative:?}");
    }
}
