use zed_extension_api::{self as zed, LanguageServerId, Result};

struct GraphcalExtension;

impl zed::Extension for GraphcalExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let path = worktree
            .which("graphcal-lsp")
            .ok_or_else(|| "graphcal-lsp not found on PATH. Install with: cargo install --path crates/graphcal-lsp".to_string())?;

        Ok(zed::Command {
            command: path,
            args: vec![],
            env: Default::default(),
        })
    }
}

zed::register_extension!(GraphcalExtension);
