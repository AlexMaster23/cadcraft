//! Native, bounded data managers supplied by extension manifests.
//!
//! The host owns rendering, validation, drawing selection and file export. Extension code receives
//! only explicit JSON actions through the same command route used by automation.

use std::collections::{BTreeMap, BTreeSet};

use cadcraft_doc::Handle;
use cadcraft_extensions::{CommandUi, ManagerColumn, TableColumnKind};
use egui::{Color32, RichText, vec2};
use serde_json::{Map, Value, json};

use crate::CadApp;

const MAX_ROWS: usize = 2_000;
const MAX_FIELD_BYTES: usize = 64 * 1024;
const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
struct Row {
    selected: bool,
    id: String,
    entities: Vec<u64>,
    values: Vec<String>,
    original_values: Vec<String>,
}

impl Row {
    fn changed(&self, columns: &[ManagerColumn]) -> bool {
        self.values.iter().zip(&self.original_values).zip(columns).any(|((value, original), column)| column.editable && value != original)
    }
}

/// Transient state for one host-rendered extension manager.
#[derive(Clone, Debug, Default)]
pub struct State {
    command_id: String,
    extension_name: String,
    title: String,
    description: String,
    accent: Option<String>,
    id_key: String,
    entity_ids_key: String,
    renumber_key: Option<String>,
    allow_delete: bool,
    allow_export: bool,
    columns: Vec<ManagerColumn>,
    rows: Vec<Row>,
    filter: String,
    summary: String,
    renumber_prefix: String,
    renumber_start: String,
    renumber_step: String,
    delete_armed: bool,
    notice: Option<Result<String, String>>,
}

/// Open the manager attached to an enabled extension command or alias.
pub fn open(app: &mut CadApp, name: &str) -> bool {
    let Some(command) = app.session.extension_command_ui(name) else {
        return false;
    };
    let CommandUi::Manager { title, description, accent, id_key, entity_ids_key, renumber_key, allow_delete, allow_export, columns } = command.ui
    else {
        return false;
    };
    let command_id = command.command_id;
    let result = app.run(&command_id, json!({"action": "list"}));
    let mut state = State {
        command_id,
        extension_name: command.extension_name,
        title,
        description,
        accent,
        id_key,
        entity_ids_key,
        renumber_key,
        allow_delete,
        allow_export,
        columns,
        rows: Vec::new(),
        filter: String::new(),
        summary: String::new(),
        renumber_prefix: String::new(),
        renumber_start: "1".into(),
        renumber_step: "1".into(),
        delete_armed: false,
        notice: None,
    };
    match result {
        Ok(value) => {
            if let Err(error) = replace_rows(&mut state, &value, &BTreeSet::new()) {
                state.notice = Some(Err(error));
            }
        }
        Err(error) => state.notice = Some(Err(error)),
    }
    app.extension_manager = state;
    app.ui.dialog = Some("extension-manager".into());
    true
}

enum Action {
    Refresh,
    Select,
    Locate,
    Save,
    Synchronize,
    Renumber,
    Delete,
    ExportAll,
    ExportSelected,
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut state = std::mem::take(&mut app.extension_manager);
    let mut action = None;
    let title = state.title.clone();
    egui::Window::new(title)
        .id(egui::Id::new("extension-manager"))
        .open(open)
        .default_size(vec2(1180.0, 620.0))
        .min_size(vec2(780.0, 420.0))
        .show(ctx, |ui| manager_ui(ui, &mut state, &mut action));
    app.extension_manager = state;

    match action {
        Some(Action::Refresh) => refresh(app),
        Some(Action::Select) => select_rows(app, false),
        Some(Action::Locate) => select_rows(app, true),
        Some(Action::Save) => save_changes(app),
        Some(Action::Synchronize) => run_selected_action(app, "synchronize", "Synchronized selected points"),
        Some(Action::Renumber) => renumber(app),
        Some(Action::Delete) => run_selected_action(app, "delete", "Deleted selected points"),
        Some(Action::ExportAll) => export(app, false),
        Some(Action::ExportSelected) => export(app, true),
        None => {}
    }
}

fn manager_ui(ui: &mut egui::Ui, state: &mut State, action: &mut Option<Action>) {
    let accent = state.accent.as_deref().and_then(parse_color).unwrap_or_else(|| ui.visuals().hyperlink_color);
    ui.horizontal(|ui| {
        ui.colored_label(accent, RichText::new("●").size(20.0));
        ui.vertical(|ui| {
            ui.strong(&state.extension_name);
            ui.label(&state.description);
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.strong("Filter");
        ui.add(egui::TextEdit::singleline(&mut state.filter).desired_width(280.0).hint_text("number, code, description or status"));
        if ui.button("Refresh").clicked() {
            *action = Some(Action::Refresh);
        }
        if !state.summary.is_empty() {
            ui.separator();
            ui.weak(&state.summary);
        }
    });

    let validation = validate(state);
    let selected = state.rows.iter().filter(|row| row.selected).count();
    let changed = state.rows.iter().filter(|row| row.changed(&state.columns)).count();
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(selected > 0, egui::Button::new("Select in drawing")).clicked() {
            *action = Some(Action::Select);
        }
        if ui.add_enabled(selected > 0, egui::Button::new("Locate")).clicked() {
            *action = Some(Action::Locate);
        }
        if ui.add_enabled(selected > 0, egui::Button::new("Synchronize selected")).clicked() {
            *action = Some(Action::Synchronize);
        }
        if ui.add_enabled(changed > 0 && validation.invalid_rows == 0, egui::Button::new("Save changes").fill(accent)).clicked() {
            *action = Some(Action::Save);
        }
        if state.allow_export {
            if ui.add_enabled(!state.rows.is_empty(), egui::Button::new("Export all...")).clicked() {
                *action = Some(Action::ExportAll);
            }
            if ui.add_enabled(selected > 0, egui::Button::new("Export selected...")).clicked() {
                *action = Some(Action::ExportSelected);
            }
        }
    });

    if state.renumber_key.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Batch renumber selected");
            ui.label("Prefix");
            ui.add(egui::TextEdit::singleline(&mut state.renumber_prefix).desired_width(80.0));
            ui.label("Start");
            ui.add(egui::TextEdit::singleline(&mut state.renumber_start).desired_width(70.0));
            ui.label("Step");
            ui.add(egui::TextEdit::singleline(&mut state.renumber_step).desired_width(60.0));
            if ui.add_enabled(selected > 0, egui::Button::new("Renumber")).clicked() {
                *action = Some(Action::Renumber);
            }
        });
    }

    if state.allow_delete {
        ui.horizontal(|ui| {
            let label = if state.delete_armed { format!("Confirm delete {selected}") } else { "Delete selected".to_owned() };
            if ui.add_enabled(selected > 0, egui::Button::new(label)).clicked() {
                if state.delete_armed {
                    *action = Some(Action::Delete);
                } else {
                    state.delete_armed = true;
                    state.notice =
                        Some(Ok("Deletion is armed. Click the confirmation button to remove the selected records and linked entities.".into()));
                }
            }
            if state.delete_armed && ui.button("Cancel delete").clicked() {
                state.delete_armed = false;
            }
        });
    }

    ui.separator();
    let filtered = filtered_indices(state);
    table_header(ui, state);
    let error_color = ui.visuals().error_fg_color;
    egui::ScrollArea::both().auto_shrink([false, false]).max_height(360.0).show_rows(ui, 28.0, filtered.len(), |ui, range| {
        for visible_index in range {
            let Some(row_index) = filtered.get(visible_index).copied() else {
                continue;
            };
            let Some(row) = state.rows.get_mut(row_index) else {
                continue;
            };
            ui.horizontal(|ui| {
                let before = row.selected;
                ui.add_sized([24.0, 22.0], egui::Checkbox::without_text(&mut row.selected));
                if before != row.selected {
                    state.delete_armed = false;
                }
                ui.add_sized([42.0, 22.0], egui::Label::new((row_index + 1).to_string()));
                for (column_index, column) in state.columns.iter().enumerate() {
                    let Some(value) = row.values.get_mut(column_index) else {
                        continue;
                    };
                    if column.editable {
                        let invalid = validation.cell_errors.get(row_index).and_then(|errors| errors.get(column_index)).and_then(Option::as_deref);
                        let mut edit = egui::TextEdit::singleline(value).desired_width(f32::from(column.width));
                        if invalid.is_some() {
                            edit = edit.text_color(error_color);
                        }
                        let response = ui.add_sized([f32::from(column.width), 22.0], edit);
                        if let Some(message) = invalid {
                            response.on_hover_text(message);
                        }
                    } else {
                        ui.add_sized([f32::from(column.width), 22.0], egui::Label::new(value.as_str()).truncate());
                    }
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        if validation.invalid_rows == 0 {
            ui.colored_label(accent, format!("{} shown · {selected} selected · {changed} changed", filtered.len()));
        } else {
            ui.colored_label(error_color, format!("{} row(s) need attention", validation.invalid_rows));
        }
        ui.separator();
        ui.weak(format!("{} total · virtualized view", state.rows.len()));
    });
    if let Some(notice) = &state.notice {
        match notice {
            Ok(message) => {
                ui.colored_label(accent, message);
            }
            Err(message) => {
                ui.colored_label(error_color, message);
            }
        }
    }
}

fn table_header(ui: &mut egui::Ui, state: &State) {
    ui.horizontal(|ui| {
        ui.add_sized([24.0, 22.0], egui::Label::new(""));
        ui.add_sized([42.0, 22.0], egui::Label::new(RichText::new("#").strong()));
        for column in &state.columns {
            let suffix = if column.required { " *" } else { "" };
            ui.add_sized([f32::from(column.width), 22.0], egui::Label::new(RichText::new(format!("{}{suffix}", column.label)).strong()));
        }
    });
}

#[derive(Debug)]
struct Validation {
    invalid_rows: usize,
    cell_errors: Vec<Vec<Option<String>>>,
}

fn validate(state: &State) -> Validation {
    let mut errors = vec![vec![None; state.columns.len()]; state.rows.len()];
    for (row_index, row) in state.rows.iter().enumerate() {
        for (column_index, column) in state.columns.iter().enumerate().filter(|(_, column)| column.editable) {
            let value = row.values.get(column_index).map_or("", String::as_str).trim();
            let error = if column.required && value.is_empty() {
                Some(format!("{} is required", column.label))
            } else if !value.is_empty() && column.kind == TableColumnKind::Number && parse_number(value).is_none() {
                Some(format!("{} must be a finite decimal number", column.label))
            } else if value.len() > MAX_FIELD_BYTES {
                Some(format!("{} is too long", column.label))
            } else {
                None
            };
            if let Some(cell) = errors.get_mut(row_index).and_then(|row_errors| row_errors.get_mut(column_index)) {
                *cell = error;
            }
        }
    }
    for (column_index, column) in state.columns.iter().enumerate().filter(|(_, column)| column.editable && column.unique) {
        let mut seen: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (row_index, row) in state.rows.iter().enumerate() {
            let value = row.values.get(column_index).map_or("", String::as_str).trim().to_lowercase();
            if !value.is_empty() {
                seen.entry(value).or_default().push(row_index);
            }
        }
        for duplicate in seen.values().filter(|rows| rows.len() > 1) {
            for row_index in duplicate {
                if let Some(cell) = errors.get_mut(*row_index).and_then(|row_errors| row_errors.get_mut(column_index)) {
                    *cell = Some(format!("{} must be unique", column.label));
                }
            }
        }
    }
    let invalid_rows = errors.iter().filter(|row| row.iter().any(Option::is_some)).count();
    Validation { invalid_rows, cell_errors: errors }
}

fn filtered_indices(state: &State) -> Vec<usize> {
    let needle = state.filter.trim().to_lowercase();
    state
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            needle.is_empty() || row.id.to_lowercase().contains(&needle) || row.values.iter().any(|value| value.to_lowercase().contains(&needle))
        })
        .map(|(index, _)| index)
        .collect()
}

fn selected_ids(state: &State) -> Vec<String> {
    state.rows.iter().filter(|row| row.selected).map(|row| row.id.clone()).collect()
}

fn selected_handles(app: &CadApp) -> Vec<Handle> {
    let Some(document) = app.session.doc().ok() else {
        return Vec::new();
    };
    app.extension_manager
        .rows
        .iter()
        .filter(|row| row.selected)
        .flat_map(|row| row.entities.iter().copied())
        .map(Handle)
        .filter(|handle| document.entity(*handle).is_some())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn select_rows(app: &mut CadApp, locate: bool) {
    let handles = selected_handles(app);
    if handles.is_empty() {
        app.extension_manager.notice = Some(Err("The selected records have no existing drawing entities".into()));
        return;
    }
    app.session.set_selection(handles.clone());
    if locate {
        let values = handles.iter().map(|handle| handle.hex()).collect::<Vec<_>>();
        match app.run("zoom.object", json!({"handles": values})) {
            Ok(_) => app.extension_manager.notice = Some(Ok("Selected and located drawing entities".into())),
            Err(error) => app.extension_manager.notice = Some(Err(error)),
        }
    } else {
        app.extension_manager.notice = Some(Ok(format!("Selected {} drawing entities", handles.len())));
    }
    app.canvas.zoom_pending = true;
}

fn save_changes(app: &mut CadApp) {
    let params = match update_parameters(&app.extension_manager) {
        Ok(params) => params,
        Err(error) => {
            app.extension_manager.notice = Some(Err(error));
            return;
        }
    };
    run_and_refresh(app, params, "Saved point changes");
}

fn update_parameters(state: &State) -> Result<Value, String> {
    let validation = validate(state);
    if validation.invalid_rows > 0 {
        return Err("Fix invalid rows before saving".into());
    }
    let changed = state.rows.iter().filter(|row| row.changed(&state.columns)).collect::<Vec<_>>();
    if changed.is_empty() {
        return Err("No records have changed".into());
    }
    let mut rows = Vec::with_capacity(changed.len());
    for row in changed {
        let mut object = Map::new();
        object.insert(state.id_key.clone(), Value::String(row.id.clone()));
        for (column_index, column) in state.columns.iter().enumerate().filter(|(_, column)| column.editable) {
            let raw = row.values.get(column_index).map_or("", String::as_str).trim();
            let value = match column.kind {
                TableColumnKind::Text => Value::String(raw.to_owned()),
                TableColumnKind::Number => Value::from(parse_number(raw).ok_or_else(|| format!("{} is not a finite decimal number", column.label))?),
            };
            object.insert(column.key.clone(), value);
        }
        rows.push(Value::Object(object));
    }
    Ok(json!({"action": "update", "rows": rows}))
}

fn run_selected_action(app: &mut CadApp, action: &str, success: &str) {
    let ids = selected_ids(&app.extension_manager);
    if ids.is_empty() {
        app.extension_manager.notice = Some(Err("Select at least one record".into()));
        return;
    }
    run_and_refresh(app, json!({"action": action, "ids": ids}), success);
}

fn renumber(app: &mut CadApp) {
    let ids = selected_ids(&app.extension_manager);
    if ids.is_empty() {
        app.extension_manager.notice = Some(Err("Select at least one record".into()));
        return;
    }
    let prefix = app.extension_manager.renumber_prefix.trim().to_owned();
    if prefix.len() > 64 || prefix.chars().any(char::is_control) {
        app.extension_manager.notice = Some(Err("Renumber prefix must contain at most 64 printable bytes".into()));
        return;
    }
    let start = match app.extension_manager.renumber_start.trim().parse::<u64>() {
        Ok(value) => value,
        Err(_) => {
            app.extension_manager.notice = Some(Err("Renumber start must be a non-negative integer".into()));
            return;
        }
    };
    let step = match app.extension_manager.renumber_step.trim().parse::<u64>() {
        Ok(value) if value > 0 => value,
        _ => {
            app.extension_manager.notice = Some(Err("Renumber step must be a positive integer".into()));
            return;
        }
    };
    run_and_refresh(app, json!({"action": "renumber", "ids": ids, "prefix": prefix, "start": start, "step": step}), "Renumbered selected points");
}

fn export(app: &mut CadApp, selected_only: bool) {
    let ids = if selected_only { selected_ids(&app.extension_manager) } else { Vec::new() };
    if selected_only && ids.is_empty() {
        app.extension_manager.notice = Some(Err("Select at least one record".into()));
        return;
    }
    let command = app.extension_manager.command_id.clone();
    let result = match app.run(&command, json!({"action": "export", "ids": ids})) {
        Ok(result) => result,
        Err(error) => {
            app.extension_manager.notice = Some(Err(error));
            return;
        }
    };
    let Some(file_name) = result.get("fileName").and_then(Value::as_str) else {
        app.extension_manager.notice = Some(Err("Extension export did not return fileName".into()));
        return;
    };
    let Some(content) = result.get("content").and_then(Value::as_str) else {
        app.extension_manager.notice = Some(Err("Extension export did not return text content".into()));
        return;
    };
    if file_name.is_empty()
        || file_name.len() > 128
        || file_name.chars().any(char::is_control)
        || file_name.contains('/')
        || file_name.contains('\\')
        || content.len() > MAX_EXPORT_BYTES
    {
        app.extension_manager.notice = Some(Err("Extension export name or content exceeds host limits".into()));
        return;
    }
    let Some(save) = app.services.save_extension_text.as_ref() else {
        app.extension_manager.notice = Some(Err("Text export is unavailable on this platform".into()));
        return;
    };
    match save(file_name, content) {
        Ok(Some(path)) => app.extension_manager.notice = Some(Ok(format!("Exported points to {path}"))),
        Ok(None) => {}
        Err(error) => app.extension_manager.notice = Some(Err(error)),
    }
}

fn run_and_refresh(app: &mut CadApp, params: Value, success: &str) {
    let command = app.extension_manager.command_id.clone();
    match app.run(&command, params) {
        Ok(_) => {
            app.canvas.zoom_pending = true;
            refresh_with_message(app, Some(success.to_owned()));
        }
        Err(error) => app.extension_manager.notice = Some(Err(error)),
    }
}

fn refresh(app: &mut CadApp) {
    refresh_with_message(app, None);
}

fn refresh_with_message(app: &mut CadApp, message: Option<String>) {
    let selected = selected_ids(&app.extension_manager).into_iter().collect::<BTreeSet<_>>();
    let command = app.extension_manager.command_id.clone();
    match app.run(&command, json!({"action": "list"})) {
        Ok(value) => match replace_rows(&mut app.extension_manager, &value, &selected) {
            Ok(()) => {
                app.extension_manager.delete_armed = false;
                app.extension_manager.notice = message.map(Ok);
            }
            Err(error) => app.extension_manager.notice = Some(Err(error)),
        },
        Err(error) => app.extension_manager.notice = Some(Err(error)),
    }
}

fn replace_rows(state: &mut State, value: &Value, selected: &BTreeSet<String>) -> Result<(), String> {
    let rows = value.get("rows").and_then(Value::as_array).ok_or_else(|| "Extension manager list must return a rows array".to_string())?;
    if rows.len() > MAX_ROWS {
        return Err(format!("Extension manager returned more than {MAX_ROWS} rows"));
    }
    let mut ids = BTreeSet::new();
    let mut parsed = Vec::with_capacity(rows.len());
    for source in rows {
        let object = source.as_object().ok_or_else(|| "Extension manager rows must be JSON objects".to_string())?;
        let id = object
            .get(&state.id_key)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control))
            .ok_or_else(|| format!("Every manager row needs a bounded string '{}'", state.id_key))?
            .to_owned();
        if !ids.insert(id.clone()) {
            return Err(format!("Extension manager returned duplicate record id '{id}'"));
        }
        let entity_values = object
            .get(&state.entity_ids_key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("Every manager row needs an '{}' array", state.entity_ids_key))?;
        if entity_values.len() > 32 {
            return Err("A manager row references more than 32 drawing entities".into());
        }
        let entities = entity_values
            .iter()
            .map(|entity| entity.as_u64().ok_or_else(|| "Manager entity ids must be unsigned integers".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut values = Vec::with_capacity(state.columns.len());
        for column in &state.columns {
            let value = match object.get(&column.key).unwrap_or(&Value::Null) {
                Value::Null => String::new(),
                Value::String(value) if value.len() <= MAX_FIELD_BYTES && !value.chars().any(char::is_control) => value.clone(),
                Value::Number(value) => value.to_string(),
                _ => return Err(format!("Manager column '{}' must contain bounded text or a number", column.key)),
            };
            values.push(value);
        }
        parsed.push(Row { selected: selected.contains(&id), id, entities, original_values: values.clone(), values });
    }
    state.rows = parsed;
    state.summary = value
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| summary.len() <= 500 && !summary.chars().any(char::is_control))
        .unwrap_or_default()
        .to_owned();
    Ok(())
}

fn parse_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let normalized = if trimmed.contains(',') && !trimmed.contains('.') { trimmed.replace(',', ".") } else { trimmed.to_owned() };
    normalized.parse::<f64>().ok().filter(|number| number.is_finite())
}

fn parse_color(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
    let green = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
    let blue = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
    Some(Color32::from_rgb(red, green, blue))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            id_key: "id".into(),
            entity_ids_key: "entities".into(),
            columns: vec![
                ManagerColumn {
                    key: "number".into(),
                    label: "Number".into(),
                    kind: TableColumnKind::Text,
                    editable: true,
                    required: true,
                    unique: true,
                    width: 100,
                },
                ManagerColumn {
                    key: "status".into(),
                    label: "Status".into(),
                    kind: TableColumnKind::Text,
                    editable: false,
                    required: false,
                    unique: false,
                    width: 100,
                },
            ],
            ..State::default()
        }
    }

    #[test]
    fn parses_bounded_rows_and_validates_duplicates() -> Result<(), String> {
        let mut state = state();
        replace_rows(
            &mut state,
            &json!({"rows":[
                {"id":"a","entities":[1],"number":"1","status":"Healthy"},
                {"id":"b","entities":[2],"number":"1","status":"Healthy"}
            ]}),
            &BTreeSet::new(),
        )?;
        assert_eq!(state.rows.len(), 2);
        assert_eq!(validate(&state).invalid_rows, 2);
        Ok(())
    }

    #[test]
    fn update_payload_contains_only_changed_editable_values() -> Result<(), String> {
        let mut state = state();
        replace_rows(&mut state, &json!({"rows":[{"id":"a","entities":[1],"number":"1","status":"Healthy"}]}), &BTreeSet::new())?;
        if let Some(value) = state.rows.get_mut(0).and_then(|row| row.values.get_mut(0)) {
            *value = "2".into();
        }
        assert_eq!(update_parameters(&state)?, json!({"action":"update","rows":[{"id":"a","number":"2"}]}));
        Ok(())
    }
}
