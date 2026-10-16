use std::collections::HashSet;
use std::fmt;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;

pub const WASM_WORLD: &str = "cadcraft:extension/cadcraft-extension@0.1.0";

/// A capability an extension asks the user to grant.
///
/// Schema 1 AutoLISP drawing access is intrinsic. Schema 2 WebAssembly extensions must explicitly
/// request drawing capabilities; neither runtime exposes process launch or network access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    ReadFiles,
    WriteFiles,
    DrawingRead,
    DrawingWrite,
}

/// Portable executable selected by a schema 2 manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Runtime {
    WasmComponent { entrypoint: String, world: String },
}

/// One user-visible command supplied by an extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    /// Stable lower-case command id used by the command line, scripts, MCP and automation.
    pub id: String,
    pub label: String,
    /// Complete menu path. The first component may introduce an extension-owned top-level menu.
    #[serde(default)]
    pub menu: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Human-readable JSON parameter contract.
    #[serde(default)]
    pub params: String,
    /// AutoLISP function (schema 1) or guest command route (schema 2) called for a programmatic
    /// invocation. Both receive the JSON parameters and return a JSON value.
    pub function: String,
    /// Whether `C:<id>` implements an interactive prompt flow. When false, choosing the command
    /// calls `function` with null parameters.
    #[serde(default)]
    pub interactive: bool,
}

/// Root `extension.json` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    /// Reverse-DNS identifier such as `org.cadcraft.surveycraft`.
    pub id: String,
    pub name: String,
    pub version: Version,
    /// Semver requirement for the CADCraft release, for example `>=0.8.0, <1.0.0`.
    pub cadcraft: VersionReq,
    pub description: String,
    /// SPDX license expression or identifier shown before installation.
    pub license: String,
    /// Relative `.lsp` file loaded into every drawing (schema 1 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Portable executable and WIT world (schema 2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), message: message.into() }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

fn simple_component(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn command_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn alias_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
}

fn lisp_symbol(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '*' | '+' | '.' | '?' | '!'))
}

/// A portable relative package path: forward slashes, no empty/dot/parent component.
pub(crate) fn package_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && !value.starts_with('/')
        && !value.contains('\\')
        && value.split('/').all(|part| !part.is_empty() && !matches!(part, "." | "..") && part.len() <= 120)
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(1..=SCHEMA_VERSION).contains(&self.schema_version) {
            return Err(ValidationError::new("schemaVersion", format!("expected 1 through {SCHEMA_VERSION}, got {}", self.schema_version)));
        }
        if self.id.len() > 128 || self.id.split('.').count() < 2 || !self.id.split('.').all(simple_component) {
            return Err(ValidationError::new("id", "must be a lower-case reverse-DNS identifier"));
        }
        let name = self.name.trim();
        if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err(ValidationError::new("name", "must contain 1 to 80 printable characters"));
        }
        let description = self.description.trim();
        if description.is_empty() || description.len() > 500 || description.chars().any(char::is_control) {
            return Err(ValidationError::new("description", "must contain 1 to 500 printable characters"));
        }
        if self.license.trim().is_empty() || self.license.len() > 80 || self.license.chars().any(char::is_control) {
            return Err(ValidationError::new("license", "must be a short SPDX license identifier or expression"));
        }
        match self.schema_version {
            1 => {
                let entrypoint = self.entrypoint.as_deref().ok_or_else(|| ValidationError::new("entrypoint", "is required by schema 1"))?;
                if !package_path(entrypoint) || !entrypoint.to_ascii_lowercase().ends_with(".lsp") {
                    return Err(ValidationError::new("entrypoint", "must be a portable relative .lsp path"));
                }
                if self.runtime.is_some() {
                    return Err(ValidationError::new("runtime", "is available only in schema 2"));
                }
                if self.permissions.iter().any(|permission| matches!(permission, Permission::DrawingRead | Permission::DrawingWrite)) {
                    return Err(ValidationError::new("permissions", "schema 1 does not declare drawing permissions"));
                }
            }
            2 => {
                if self.entrypoint.is_some() {
                    return Err(ValidationError::new("entrypoint", "schema 2 declares its entrypoint inside runtime"));
                }
                let runtime = self.runtime.as_ref().ok_or_else(|| ValidationError::new("runtime", "is required by schema 2"))?;
                match runtime {
                    Runtime::WasmComponent { entrypoint, world } => {
                        if !package_path(entrypoint) || !entrypoint.to_ascii_lowercase().ends_with(".wasm") {
                            return Err(ValidationError::new("runtime.entrypoint", "must be a portable relative .wasm path"));
                        }
                        if world != WASM_WORLD {
                            return Err(ValidationError::new("runtime.world", format!("must be `{WASM_WORLD}`")));
                        }
                    }
                }
                if self.permissions.iter().any(|permission| matches!(permission, Permission::ReadFiles | Permission::WriteFiles)) {
                    return Err(ValidationError::new("permissions", "schema 2 does not expose file-system capabilities"));
                }
            }
            _ => return Err(ValidationError::new("schemaVersion", "is unsupported")),
        }
        if self.permissions.len() > 4 || self.permissions.iter().copied().collect::<HashSet<_>>().len() != self.permissions.len() {
            return Err(ValidationError::new("permissions", "contains a duplicate or unsupported permission"));
        }
        if self.commands.is_empty() || self.commands.len() > 256 {
            return Err(ValidationError::new("commands", "must contain between 1 and 256 commands"));
        }
        let mut ids = HashSet::new();
        let mut aliases = HashSet::new();
        for (index, command) in self.commands.iter().enumerate() {
            let base = format!("commands[{index}]");
            if command.id.len() > 64 || !command_name(&command.id) {
                return Err(ValidationError::new(format!("{base}.id"), "must be a lower-case command identifier"));
            }
            if !ids.insert(command.id.clone()) {
                return Err(ValidationError::new(format!("{base}.id"), "duplicates another command"));
            }
            if command.label.trim().is_empty() || command.label.len() > 100 || command.label.chars().any(char::is_control) {
                return Err(ValidationError::new(format!("{base}.label"), "must contain 1 to 100 printable characters"));
            }
            if command.menu.len() > 8
                || command.menu.iter().any(|segment| segment.trim().is_empty() || segment.len() > 80 || segment.chars().any(char::is_control))
            {
                return Err(ValidationError::new(format!("{base}.menu"), "must contain at most 8 non-empty printable segments"));
            }
            if command.aliases.len() > 16 {
                return Err(ValidationError::new(format!("{base}.aliases"), "contains more than 16 aliases"));
            }
            for alias in &command.aliases {
                let folded = alias.to_ascii_lowercase();
                if !alias_name(alias) || !aliases.insert(folded) {
                    return Err(ValidationError::new(format!("{base}.aliases"), "contains an invalid or duplicate alias"));
                }
            }
            if command.params.len() > 2_000 || command.params.chars().any(char::is_control) {
                return Err(ValidationError::new(format!("{base}.params"), "must contain at most 2000 printable characters"));
            }
            let function_is_valid = if self.schema_version == 1 { lisp_symbol(&command.function) } else { command_name(&command.function) };
            if !function_is_valid {
                let expected = if self.schema_version == 1 { "an AutoLISP symbol name" } else { "a lower-case guest command route" };
                return Err(ValidationError::new(format!("{base}.function"), format!("must be {expected}")));
            }
            if self.schema_version == 2 && command.interactive {
                return Err(ValidationError::new(format!("{base}.interactive"), "schema 2 commands are programmatic in this release"));
            }
        }
        if let Some(homepage) = &self.homepage
            && (homepage.len() > 500 || !(homepage.starts_with("https://") || homepage.starts_with("http://")))
        {
            return Err(ValidationError::new("homepage", "must be an http(s) URL of at most 500 characters"));
        }
        Ok(())
    }

    pub fn supports(&self, cadcraft_version: &Version) -> bool {
        self.cadcraft.matches(cadcraft_version)
    }

    pub fn entrypoint_path(&self) -> Option<&str> {
        match (&self.entrypoint, &self.runtime) {
            (Some(path), None) => Some(path),
            (None, Some(Runtime::WasmComponent { entrypoint, .. })) => Some(entrypoint),
            _ => None,
        }
    }

    pub fn is_wasm(&self) -> bool {
        matches!(self.runtime, Some(Runtime::WasmComponent { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Manifest {
        Manifest {
            schema_version: 1,
            id: "org.cadcraft.example".into(),
            name: "Example".into(),
            version: Version::new(1, 2, 3),
            cadcraft: VersionReq::parse(">=0.7, <1").unwrap(),
            description: "A small extension used by the tests.".into(),
            license: "MIT".into(),
            entrypoint: Some("main.lsp".into()),
            runtime: None,
            permissions: vec![],
            commands: vec![Command {
                id: "examplehello".into(),
                label: "Hello".into(),
                menu: vec!["Example".into(), "Hello".into()],
                aliases: vec!["EH".into()],
                params: "{name?}".into(),
                function: "EXAMPLE:HELLO".into(),
                interactive: true,
            }],
            homepage: Some("https://example.invalid/extension".into()),
        }
    }

    #[test]
    fn accepts_a_bounded_manifest() {
        let manifest = good();
        assert!(manifest.validate().is_ok());
        assert!(manifest.supports(&Version::new(0, 7, 0)));
        assert!(!manifest.supports(&Version::new(1, 0, 0)));
    }

    #[test]
    fn rejects_paths_and_duplicate_commands() {
        for path in ["../main.lsp", "/main.lsp", "dir\\main.lsp", "main.txt"] {
            let mut manifest = good();
            manifest.entrypoint = Some(path.into());
            assert!(manifest.validate().is_err(), "{path}");
        }
        let mut manifest = good();
        manifest.commands.push(manifest.commands[0].clone());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn accepts_a_schema_two_wasm_component() {
        let mut manifest = good();
        manifest.schema_version = 2;
        manifest.entrypoint = None;
        manifest.runtime = Some(Runtime::WasmComponent { entrypoint: "surveycraft.wasm".into(), world: WASM_WORLD.into() });
        manifest.permissions = vec![Permission::DrawingRead, Permission::DrawingWrite];
        manifest.commands[0].function = "reference-line".into();
        manifest.commands[0].interactive = false;
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.entrypoint_path(), Some("surveycraft.wasm"));
        assert!(manifest.is_wasm());
    }
}
