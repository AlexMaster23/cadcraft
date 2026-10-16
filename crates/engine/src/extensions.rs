//! Runtime extension catalog.
//!
//! The desktop/web hosts own installation and persistence. The engine receives only validated
//! manifests and UTF-8 entry-point source, so command discovery and execution are identical from
//! the command line, scripts, control channel, MCP and the UI.

use std::collections::HashSet;
use std::sync::Arc;

use cadcraft_extensions::{Manifest, Permission};

use crate::{CommandInfo, EngineError, Result, Session, cmd};

#[derive(Clone, Debug)]
pub struct Extension {
    pub manifest: Manifest,
    pub(crate) program: ExtensionProgram,
    pub enabled: bool,
}

/// Host-rendered UI attached to an enabled extension command.
#[derive(Clone, Debug)]
pub struct ExtensionCommandUi {
    pub command_id: String,
    pub extension_name: String,
    pub ui: cadcraft_extensions::CommandUi,
}

#[derive(Clone, Debug)]
pub(crate) enum ExtensionProgram {
    AutoLisp(String),
    WasmComponent(Arc<[u8]>),
}

impl Extension {
    pub fn new(manifest: Manifest, entrypoint: String, enabled: bool) -> Self {
        Self { manifest, program: ExtensionProgram::AutoLisp(entrypoint), enabled }
    }

    pub fn from_bytes(manifest: Manifest, entrypoint: Vec<u8>, enabled: bool) -> std::result::Result<Self, String> {
        manifest.validate().map_err(|error| error.to_string())?;
        let program = if manifest.is_wasm() {
            ExtensionProgram::WasmComponent(Arc::from(entrypoint))
        } else {
            ExtensionProgram::AutoLisp(String::from_utf8(entrypoint).map_err(|_| "AutoLISP extension entrypoint is not UTF-8".to_string())?)
        };
        Ok(Self { manifest, program, enabled })
    }

    pub(crate) fn runtime_available(&self) -> bool {
        match self.program {
            ExtensionProgram::AutoLisp(_) => true,
            ExtensionProgram::WasmComponent(_) => cfg!(not(target_arch = "wasm32")),
        }
    }
}

#[derive(Default)]
pub(crate) struct Registry {
    entries: Vec<Extension>,
}

impl Registry {
    fn command(&self, id: &str) -> Option<(&Extension, &cadcraft_extensions::Command)> {
        self.entries.iter().filter(|extension| extension.enabled).find_map(|extension| {
            extension.manifest.commands.iter().find(|command| command.id.eq_ignore_ascii_case(id)).map(|command| (extension, command))
        })
    }

    fn resolve(&self, name: &str) -> Option<String> {
        if let Some((_, command)) = self.command(name) {
            return Some(command.id.clone());
        }
        self.entries.iter().filter(|extension| extension.enabled).find_map(|extension| {
            extension
                .manifest
                .commands
                .iter()
                .find(|command| command.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(name)))
                .map(|command| command.id.clone())
        })
    }
}

impl Session {
    /// Replace the in-memory extension catalog after validating command ownership. A failed
    /// replacement leaves the previous catalog active.
    pub fn set_extensions(&mut self, mut extensions: Vec<Extension>) -> Result<()> {
        if self.lisp_running() {
            return Err(EngineError::Other("cannot change extensions while an AutoLISP routine is running".into()));
        }
        let mut ids = HashSet::new();
        let mut aliases = HashSet::new();
        extensions.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        for extension in &extensions {
            extension.manifest.validate().map_err(|error| EngineError::Other(format!("extension {}: {error}", extension.manifest.id)))?;
            if extension.manifest.is_wasm() != matches!(extension.program, ExtensionProgram::WasmComponent(_)) {
                return Err(EngineError::Other(format!("extension {} runtime does not match its manifest", extension.manifest.id)));
            }
            if !extension.enabled {
                continue;
            }
            for command in &extension.manifest.commands {
                let id = command.id.to_ascii_lowercase();
                if cmd::find_command(&id).is_some() || cmd::resolve_alias(&id) != id {
                    return Err(EngineError::Other(format!(
                        "extension {} command `{id}` conflicts with a built-in command or alias",
                        extension.manifest.id
                    )));
                }
                if aliases.contains(&id) || !ids.insert(id.clone()) {
                    return Err(EngineError::Other(format!("extension command `{id}` has more than one owner")));
                }
                for alias in &command.aliases {
                    let folded = alias.to_ascii_lowercase();
                    if cmd::find_command(&folded).is_some() || cmd::resolve_alias(&folded) != folded {
                        return Err(EngineError::Other(format!("extension alias `{alias}` conflicts with a built-in command or alias")));
                    }
                    if ids.contains(&folded) || !aliases.insert(folded.clone()) {
                        return Err(EngineError::Other(format!("extension alias `{alias}` conflicts with another extension command or alias")));
                    }
                }
            }
        }
        let old = self.extensions.entries.clone();
        self.lisp_replace_extensions(&old, &extensions);
        self.extensions.entries = extensions;
        Ok(())
    }

    pub fn extensions(&self) -> &[Extension] {
        &self.extensions.entries
    }

    /// Built-in and enabled extension commands in their display order.
    pub fn command_infos(&self) -> Vec<CommandInfo> {
        let mut infos: Vec<CommandInfo> = cmd::command_specs().iter().map(|command| command.info(self)).collect();
        for extension in self.extensions.entries.iter().filter(|extension| extension.enabled) {
            for command in &extension.manifest.commands {
                let has_document = self.state().is_ok();
                let runtime_available = extension.runtime_available();
                let enabled = has_document && runtime_available;
                infos.push(CommandInfo {
                    id: command.id.clone(),
                    label: command.label.clone(),
                    menu: command.menu.clone(),
                    shortcut: None,
                    aliases: command.aliases.clone(),
                    params: command.params.clone(),
                    interactive: command.interactive,
                    enabled,
                    disabled_reason: if !has_document {
                        Some("no drawing open".into())
                    } else if !runtime_available {
                        Some("WebAssembly extension runtime is unavailable in the browser".into())
                    } else {
                        None
                    },
                    extension: Some(extension.manifest.id.clone()),
                });
            }
        }
        infos
    }

    pub fn command_info(&self, id: &str) -> Option<CommandInfo> {
        if let Some(command) = cmd::find_command(id) {
            return Some(command.info(self));
        }
        let (extension, command) = self.extensions.command(id)?;
        let has_document = self.state().is_ok();
        let runtime_available = extension.runtime_available();
        let enabled = has_document && runtime_available;
        Some(CommandInfo {
            id: command.id.clone(),
            label: command.label.clone(),
            menu: command.menu.clone(),
            shortcut: None,
            aliases: command.aliases.clone(),
            params: command.params.clone(),
            interactive: command.interactive,
            enabled,
            disabled_reason: if !has_document {
                Some("no drawing open".into())
            } else if !runtime_available {
                Some("WebAssembly extension runtime is unavailable in the browser".into())
            } else {
                None
            },
            extension: Some(extension.manifest.id.clone()),
        })
    }

    /// Resolve an extension id or alias to the declarative UI a human invocation should open.
    /// Headless and JSON callers do not use this method.
    pub fn extension_command_ui(&self, name: &str) -> Option<ExtensionCommandUi> {
        let command_id = self.resolve_extension_command(name)?;
        let (extension, command) = self.extension_command(&command_id)?;
        let info = self.command_info(&command_id)?;
        if !info.enabled {
            return None;
        }
        Some(ExtensionCommandUi { command_id, extension_name: extension.manifest.name.clone(), ui: command.ui.clone()? })
    }

    pub(crate) fn resolve_extension_command(&self, name: &str) -> Option<String> {
        self.extensions.resolve(name)
    }

    /// Resolve a command or alias using built-ins first and enabled extensions second.
    pub fn resolve_command_name(&self, name: &str) -> String {
        let lower = name.to_ascii_lowercase();
        if cmd::find_command(&lower).is_some() || self.extensions.command(&lower).is_some() {
            return lower;
        }
        let builtin = cmd::resolve_alias(&lower);
        if builtin != lower {
            return builtin;
        }
        self.extensions.resolve(&lower).unwrap_or(lower)
    }

    pub(crate) fn extension_command(&self, id: &str) -> Option<(&Extension, &cadcraft_extensions::Command)> {
        self.extensions.command(id)
    }

    pub(crate) fn extension_has_permission(&self, extension_id: &str, permission: Permission) -> bool {
        self.extensions
            .entries
            .iter()
            .find(|extension| extension.enabled && extension.manifest.id == extension_id)
            .is_some_and(|extension| extension.manifest.permissions.contains(&permission))
    }
}

#[cfg(test)]
mod tests {
    use semver::{Version, VersionReq};
    use serde_json::json;

    use super::*;

    fn extension(id: &str, command: &str) -> Extension {
        Extension::new(
            Manifest {
                schema_version: 1,
                id: id.into(),
                name: "Example".into(),
                version: Version::new(1, 0, 0),
                cadcraft: VersionReq::STAR,
                description: "An engine extension registry test.".into(),
                license: "MIT".into(),
                entrypoint: Some("main.lsp".into()),
                runtime: None,
                permissions: Vec::new(),
                commands: vec![cadcraft_extensions::Command {
                    id: command.into(),
                    label: "Example command".into(),
                    menu: vec!["Example".into(), "Run".into()],
                    aliases: vec!["ZXQEXT".into()],
                    params: "{}".into(),
                    function: "EXAMPLE:RUN".into(),
                    interactive: true,
                    ui: None,
                }],
                homepage: None,
            },
            "(defun example:run (params) 42)".into(),
            true,
        )
    }

    #[test]
    fn catalog_exposes_commands_and_rejects_collisions() {
        let mut session = Session::new();
        session.set_extensions(vec![extension("org.cadcraft.example", "examplecmd")]).unwrap();
        let info = session.command_info("examplecmd").unwrap();
        assert_eq!(info.extension.as_deref(), Some("org.cadcraft.example"));
        assert_eq!(session.resolve_extension_command("zxqext").as_deref(), Some("examplecmd"));
        assert!(session.set_extensions(vec![extension("org.cadcraft.bad", "line")]).is_err());
        assert!(session.command_info("examplecmd").is_some(), "a rejected replacement keeps the old catalog");
    }

    #[test]
    fn programmatic_and_interactive_extension_commands_run() {
        let mut session = Session::new();
        let mut extension = extension("org.cadcraft.example", "examplecmd");
        extension.program = ExtensionProgram::AutoLisp(
            r#"
            (defun example:run (params)
              (+ (cdr (assoc "a" params)) (cdr (assoc "b" params))))
            (defun c:examplecmd () (setq example:interactive-ran 1) (princ))
        "#
            .into(),
        );
        session.set_extensions(vec![extension]).unwrap();

        assert_eq!(session.execute("examplecmd", &json!({"a": 2, "b": 5})).unwrap(), json!(7));
        session.start("ZXQEXT").unwrap();
        assert_eq!(session.lisp_var("EXAMPLE:INTERACTIVE-RAN").as_number(), Some(1.0));
    }

    #[test]
    fn entrypoint_loads_in_every_drawing_and_disable_removes_commands() {
        let mut session = Session::new();
        let mut extension = extension("org.cadcraft.example", "examplecmd");
        extension.program = ExtensionProgram::AutoLisp("(setq example:loaded 1) (defun example:run (params) example:loaded)".into());
        session.set_extensions(vec![extension.clone()]).unwrap();
        assert_eq!(session.execute("examplecmd", &json!(null)).unwrap(), json!(1));

        session.new_drawing(false);
        assert_eq!(session.execute("examplecmd", &json!(null)).unwrap(), json!(1));
        extension.enabled = false;
        session.set_extensions(vec![extension]).unwrap();
        assert!(matches!(session.execute("examplecmd", &json!(null)), Err(EngineError::UnknownCommand(_))));
    }

    #[test]
    fn file_access_requires_a_declared_permission() {
        let mut session = Session::new();
        let path = std::env::temp_dir().join(format!("cadcraft-extension-permission-{}.txt", std::process::id()));
        std::fs::write(&path, "test").unwrap();
        let shown = path.to_string_lossy().replace('\\', "/");
        let mut extension = extension("org.cadcraft.example", "examplecmd");
        extension.program = ExtensionProgram::AutoLisp(format!("(defun example:run (params) (findfile \"{shown}\"))"));
        session.set_extensions(vec![extension.clone()]).unwrap();
        let error = session.execute("examplecmd", &json!(null)).unwrap_err().to_string();
        assert!(error.contains("read-files"), "{error}");

        extension.manifest.permissions.push(Permission::ReadFiles);
        session.set_extensions(vec![extension]).unwrap();
        assert_eq!(session.execute("examplecmd", &json!(null)).unwrap(), json!(shown));
        let _ = std::fs::remove_file(path);
    }
}
