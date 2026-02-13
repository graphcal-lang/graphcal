use zed::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result};

struct GraphcalExtension;

impl zed::Extension for GraphcalExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;

        if let Some(binary) = settings.binary {
            return Ok(zed::Command {
                command: binary.path.unwrap_or_else(|| "graphcal-lsp".to_string()),
                args: binary.arguments.unwrap_or_default(),
                env: Default::default(),
            });
        }

        let path = worktree.which("graphcal-lsp").ok_or_else(|| {
            "graphcal-lsp not found on PATH. \
             Install with: cargo install --path crates/graphcal-lsp"
                .to_string()
        })?;

        Ok(zed::Command {
            command: path,
            args: vec![],
            env: Default::default(),
        })
    }
}

zed::register_extension!(GraphcalExtension);
