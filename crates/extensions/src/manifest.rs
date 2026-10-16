use std::collections::HashSet;
use std::fmt;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;

/// A capability an extension asks the user to grant.
///
/// Reading and changing the active drawing is intrinsic to an extension command. File access is
/// separate because it crosses the drawing boundary. CADCraft's extension runtime has no process
/// launch or network primitive in schema 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    ReadFiles,
    WriteFiles,
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
    /// AutoLISP function called for a programmatic invocation. It receives one value containing
    /// the JSON parameters and returns a value converted back to JSON.
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
    /// Relative `.lsp` file loaded into every drawing while the extension is enabled.
    pub entrypoint: String,
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
        if self.schema_version != SCHEMA_VERSION {
            return Err(ValidationError::new("schemaVersion", format!("expected {SCHEMA_VERSION}, got {}", self.schema_version)));
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
        if !package_path(&self.entrypoint) || !self.entrypoint.to_ascii_lowercase().ends_with(".lsp") {
            return Err(ValidationError::new("entrypoint", "must be a portable relative .lsp path"));
        }
        if self.permissions.len() > 2 || self.permissions.iter().copied().collect::<HashSet<_>>().len() != self.permissions.len() {
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
            if !lisp_symbol(&command.function) {
                return Err(ValidationError::new(format!("{base}.function"), "must be an AutoLISP symbol name"));
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
            entrypoint: "main.lsp".into(),
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
            manifest.entrypoint = path.into();
            assert!(manifest.validate().is_err(), "{path}");
        }
        let mut manifest = good();
        manifest.commands.push(manifest.commands[0].clone());
        assert!(manifest.validate().is_err());
    }
}
