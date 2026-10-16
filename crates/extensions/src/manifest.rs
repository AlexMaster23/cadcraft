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

/// Value accepted by one column of a host-rendered table dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TableColumnKind {
    Text,
    Number,
}

/// CADCraft-owned icon available to a command placed in the Tool Sets palette.
///
/// Extensions select a semantic icon from this bounded set rather than shipping executable UI
/// code or borrowing artwork from another application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolsetIcon {
    New,
    Open,
    Save,
    Import,
    Export,
    Attach,
    Share,
    Properties,
    Layers,
    Line,
    Polyline,
    Circle,
    Arc,
    Rectangle,
    Polygon,
    Ellipse,
    Spline,
    Point,
    Hatch,
    Region,
    Table,
    Text,
    Move,
    Copy,
    Rotate,
    Settings,
}

/// Existing CADCraft Tool Sets tab that receives an extension-owned group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolsetTab {
    Drafting,
    Modeling,
}

/// Visual size of one Tool Sets command button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolsetSize {
    #[default]
    Large,
    Small,
}

/// Placement of an extension group relative to CADCraft's built-in groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolsetPosition {
    Top,
    #[default]
    Bottom,
}

/// One command button inside an extension-owned Tool Sets group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolsetTool {
    pub command: String,
    pub icon: ToolsetIcon,
    #[serde(default)]
    pub size: ToolsetSize,
}

/// A bounded, declarative group inserted into one of CADCraft's native Tool Sets tabs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Toolset {
    pub tab: ToolsetTab,
    pub group: String,
    #[serde(default)]
    pub position: ToolsetPosition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    pub tools: Vec<ToolsetTool>,
}

/// One bounded, declarative column rendered by CADCraft rather than extension code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableColumn {
    pub key: String,
    pub label: String,
    pub kind: TableColumnKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    /// Case-insensitive CSV header names accepted in addition to `key` and `label`.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// One column in a host-rendered extension data manager.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagerColumn {
    pub key: String,
    pub label: String,
    pub kind: TableColumnKind,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default = "default_manager_column_width")]
    pub width: u16,
}

const fn default_manager_column_width() -> u16 {
    120
}

/// One value in a host-rendered table-dialog selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableSelectChoice {
    pub value: String,
    pub label: String,
}

/// A bounded selector rendered once above a table. Its selected value is submitted as a top-level
/// JSON field alongside `rows`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableSelect {
    pub key: String,
    pub label: String,
    pub default: String,
    pub choices: Vec<TableSelectChoice>,
}

/// Optional native UI presented only for a menu, toolbar or typed invocation. Programmatic
/// command execution still calls the guest directly and never opens a dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum CommandUi {
    Table {
        title: String,
        description: String,
        submit_label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accent: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selects: Vec<TableSelect>,
        columns: Vec<TableColumn>,
    },
    /// A native, virtualized editor for bounded records loaded and mutated through the command's
    /// JSON action protocol. Reserved row fields identify the record and its drawing entities.
    Manager {
        title: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accent: Option<String>,
        id_key: String,
        entity_ids_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renumber_key: Option<String>,
        #[serde(default)]
        allow_delete: bool,
        #[serde(default)]
        allow_export: bool,
        columns: Vec<ManagerColumn>,
    },
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
    /// A host-owned declarative dialog whose submitted object is passed to `function`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<CommandUi>,
}

/// Root `extension.json` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    /// Reverse-DNS identifier such as `org.example.drafting-tools`.
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
    /// Native Tool Sets groups contributed by this extension.
    #[serde(default)]
    pub toolsets: Vec<Toolset>,
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

fn printable(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
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
            if let Some(CommandUi::Table { title, description, submit_label, accent, selects, columns }) = &command.ui {
                if command.interactive {
                    return Err(ValidationError::new(format!("{base}.ui"), "a host-rendered dialog cannot also use an interactive command prompt"));
                }
                if !printable(title, 100) || !printable(description, 500) || !printable(submit_label, 80) {
                    return Err(ValidationError::new(format!("{base}.ui"), "title, description and submitLabel must be bounded printable text"));
                }
                if accent.as_deref().is_some_and(|value| !color(value)) {
                    return Err(ValidationError::new(format!("{base}.ui.accent"), "must be a #RRGGBB colour"));
                }
                if selects.len() > 8 {
                    return Err(ValidationError::new(format!("{base}.ui.selects"), "must contain at most 8 selectors"));
                }
                let mut select_keys = HashSet::new();
                for (select_index, select) in selects.iter().enumerate() {
                    let field = format!("{base}.ui.selects[{select_index}]");
                    if select.key.eq_ignore_ascii_case("rows")
                        || select.key.len() > 64
                        || !command_name(&select.key)
                        || !select_keys.insert(select.key.to_ascii_lowercase())
                    {
                        return Err(ValidationError::new(format!("{field}.key"), "must be a unique lower-case field name"));
                    }
                    if !printable(&select.label, 80) {
                        return Err(ValidationError::new(format!("{field}.label"), "must contain 1 to 80 printable characters"));
                    }
                    if select.choices.len() < 2 || select.choices.len() > 16 {
                        return Err(ValidationError::new(format!("{field}.choices"), "must contain between 2 and 16 choices"));
                    }
                    let mut values = HashSet::new();
                    for (choice_index, choice) in select.choices.iter().enumerate() {
                        if choice.value.len() > 64 || !command_name(&choice.value) || !values.insert(choice.value.to_ascii_lowercase()) {
                            return Err(ValidationError::new(format!("{field}.choices[{choice_index}].value"), "must be a unique lower-case value"));
                        }
                        if !printable(&choice.label, 120) {
                            return Err(ValidationError::new(
                                format!("{field}.choices[{choice_index}].label"),
                                "must contain 1 to 120 printable characters",
                            ));
                        }
                    }
                    if !select.choices.iter().any(|choice| choice.value == select.default) {
                        return Err(ValidationError::new(format!("{field}.default"), "must match one declared choice value"));
                    }
                }
                if columns.is_empty() || columns.len() > 12 {
                    return Err(ValidationError::new(format!("{base}.ui.columns"), "must contain between 1 and 12 columns"));
                }
                let mut keys = HashSet::new();
                for (column_index, column) in columns.iter().enumerate() {
                    let field = format!("{base}.ui.columns[{column_index}]");
                    if column.key.len() > 64 || !command_name(&column.key) || !keys.insert(column.key.to_ascii_lowercase()) {
                        return Err(ValidationError::new(format!("{field}.key"), "must be a unique lower-case field name"));
                    }
                    if !printable(&column.label, 80) {
                        return Err(ValidationError::new(format!("{field}.label"), "must contain 1 to 80 printable characters"));
                    }
                    if column.aliases.len() > 16 || column.aliases.iter().any(|alias| !printable(alias, 80)) {
                        return Err(ValidationError::new(format!("{field}.aliases"), "must contain at most 16 bounded printable aliases"));
                    }
                }
            }
            if let Some(CommandUi::Manager {
                title,
                description,
                accent,
                id_key,
                entity_ids_key,
                renumber_key,
                allow_delete: _,
                allow_export: _,
                columns,
            }) = &command.ui
            {
                if command.interactive {
                    return Err(ValidationError::new(format!("{base}.ui"), "a host-rendered manager cannot also use an interactive command prompt"));
                }
                if !printable(title, 100) || !printable(description, 500) {
                    return Err(ValidationError::new(format!("{base}.ui"), "title and description must be bounded printable text"));
                }
                if accent.as_deref().is_some_and(|value| !color(value)) {
                    return Err(ValidationError::new(format!("{base}.ui.accent"), "must be a #RRGGBB colour"));
                }
                if !command_name(id_key) || !command_name(entity_ids_key) || id_key == entity_ids_key {
                    return Err(ValidationError::new(format!("{base}.ui"), "idKey and entityIdsKey must be different lower-case field names"));
                }
                if columns.is_empty() || columns.len() > 16 {
                    return Err(ValidationError::new(format!("{base}.ui.columns"), "must contain between 1 and 16 columns"));
                }
                let mut keys = HashSet::from([id_key.to_ascii_lowercase(), entity_ids_key.to_ascii_lowercase()]);
                for (column_index, column) in columns.iter().enumerate() {
                    let field = format!("{base}.ui.columns[{column_index}]");
                    if column.key.len() > 64 || !command_name(&column.key) || !keys.insert(column.key.to_ascii_lowercase()) {
                        return Err(ValidationError::new(format!("{field}.key"), "must be a unique lower-case field name"));
                    }
                    if !printable(&column.label, 80) {
                        return Err(ValidationError::new(format!("{field}.label"), "must contain 1 to 80 printable characters"));
                    }
                    if !(60..=400).contains(&column.width) {
                        return Err(ValidationError::new(format!("{field}.width"), "must be between 60 and 400 pixels"));
                    }
                    if !column.editable && (column.required || column.unique) {
                        return Err(ValidationError::new(field, "read-only columns cannot declare required or unique editing constraints"));
                    }
                }
                if !columns.iter().any(|column| column.editable) {
                    return Err(ValidationError::new(format!("{base}.ui.columns"), "must contain at least one editable column"));
                }
                if let Some(key) = renumber_key
                    && !columns.iter().any(|column| column.key == *key && column.editable && column.kind == TableColumnKind::Text)
                {
                    return Err(ValidationError::new(format!("{base}.ui.renumberKey"), "must name an editable text column"));
                }
            }
        }
        if self.toolsets.len() > 16 {
            return Err(ValidationError::new("toolsets", "contains more than 16 groups"));
        }
        let mut toolset_groups = HashSet::new();
        let mut placed_commands = HashSet::new();
        for (index, toolset) in self.toolsets.iter().enumerate() {
            let base = format!("toolsets[{index}]");
            if !printable(&toolset.group, 80) {
                return Err(ValidationError::new(format!("{base}.group"), "must contain 1 to 80 printable characters"));
            }
            let group_key = (toolset.tab, toolset.group.to_ascii_lowercase());
            if !toolset_groups.insert(group_key) {
                return Err(ValidationError::new(format!("{base}.group"), "duplicates another group on the same tab"));
            }
            if toolset.accent.as_deref().is_some_and(|value| !color(value)) {
                return Err(ValidationError::new(format!("{base}.accent"), "must be a #RRGGBB colour"));
            }
            if toolset.tools.is_empty() || toolset.tools.len() > 32 {
                return Err(ValidationError::new(format!("{base}.tools"), "must contain between 1 and 32 tools"));
            }
            for (tool_index, tool) in toolset.tools.iter().enumerate() {
                let command = tool.command.to_ascii_lowercase();
                if !ids.contains(&command) {
                    return Err(ValidationError::new(
                        format!("{base}.tools[{tool_index}].command"),
                        "must reference a command declared by this extension",
                    ));
                }
                if !placed_commands.insert(command) {
                    return Err(ValidationError::new(format!("{base}.tools[{tool_index}].command"), "is already placed in another Tool Sets group"));
                }
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
                ui: None,
            }],
            toolsets: vec![],
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
        manifest.runtime = Some(Runtime::WasmComponent { entrypoint: "extension.wasm".into(), world: WASM_WORLD.into() });
        manifest.permissions = vec![Permission::DrawingRead, Permission::DrawingWrite];
        manifest.commands[0].function = "reference-line".into();
        manifest.commands[0].interactive = false;
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.entrypoint_path(), Some("extension.wasm"));
        assert!(manifest.is_wasm());
    }

    #[test]
    fn accepts_a_bounded_native_table_dialog() {
        let mut manifest = good();
        manifest.commands[0].interactive = false;
        manifest.commands[0].ui = Some(CommandUi::Table {
            title: "Import points".into(),
            description: "Type rows or open a CSV file.".into(),
            submit_label: "Insert points".into(),
            accent: Some("#F47C20".into()),
            selects: vec![TableSelect {
                key: "coordinate-convention".into(),
                label: "Coordinate convention".into(),
                default: "cad-xy".into(),
                choices: vec![
                    TableSelectChoice { value: "cad-xy".into(), label: "CAD / GIS X-Y".into() },
                    TableSelectChoice { value: "survey-xy".into(), label: "Survey X-Y".into() },
                ],
            }],
            columns: vec![TableColumn {
                key: "number".into(),
                label: "Point number".into(),
                kind: TableColumnKind::Text,
                required: true,
                unique: true,
                aliases: vec!["point".into()],
            }],
        });
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn accepts_a_bounded_native_data_manager() {
        let mut manifest = good();
        manifest.commands[0].interactive = false;
        manifest.commands[0].ui = Some(CommandUi::Manager {
            title: "Point manager".into(),
            description: "Edit linked survey points.".into(),
            accent: Some("#F47A20".into()),
            id_key: "id".into(),
            entity_ids_key: "entity-ids".into(),
            renumber_key: Some("number".into()),
            allow_delete: true,
            allow_export: true,
            columns: vec![ManagerColumn {
                key: "number".into(),
                label: "Point number".into(),
                kind: TableColumnKind::Text,
                editable: true,
                required: true,
                unique: true,
                width: 120,
            }],
        });
        assert!(manifest.validate().is_ok());

        if let Some(CommandUi::Manager { columns, .. }) = &mut manifest.commands[0].ui {
            columns[0].width = 20;
        }
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn accepts_toolsets_and_rejects_unknown_or_duplicate_commands() {
        let mut manifest = good();
        manifest.toolsets.push(Toolset {
            tab: ToolsetTab::Drafting,
            group: "My Extension".into(),
            position: ToolsetPosition::Top,
            accent: Some("#F47A20".into()),
            tools: vec![ToolsetTool { command: "examplehello".into(), icon: ToolsetIcon::Import, size: ToolsetSize::Large }],
        });
        assert!(manifest.validate().is_ok());

        manifest.toolsets[0].tools.push(ToolsetTool { command: "missing".into(), icon: ToolsetIcon::Point, size: ToolsetSize::Small });
        assert!(manifest.validate().is_err());
        manifest.toolsets[0].tools[1].command = "examplehello".into();
        assert!(manifest.validate().is_err());
    }
}
