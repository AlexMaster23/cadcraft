//! Native, declarative table dialogs supplied by extension manifests.
//!
//! File access stays in the application shell and requires an explicit user pick. The extension
//! receives only the bounded JSON rows submitted by the user.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use cadcraft_extensions::{CommandUi, TableColumn, TableColumnKind, TableSelect};
use csv::{ReaderBuilder, Trim};
use egui::{Color32, RichText, vec2};
use serde_json::{Map, Value};

use crate::{CadApp, LoadedTextTable};

const MAX_ROWS: usize = 2_000;
const MAX_FIELD_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
struct Row {
    selected: bool,
    values: Vec<String>,
}

#[derive(Clone, Debug)]
struct SelectState {
    spec: TableSelect,
    value: String,
}

impl Row {
    fn empty(columns: usize) -> Self {
        Self { selected: false, values: vec![String::new(); columns] }
    }

    fn is_empty(&self) -> bool {
        self.values.iter().all(|value| value.trim().is_empty())
    }
}

/// Transient state for the one extension table dialog CADCraft can show at a time.
#[derive(Clone, Debug, Default)]
pub struct State {
    command_id: String,
    extension_name: String,
    title: String,
    description: String,
    submit_label: String,
    accent: Option<String>,
    selects: Vec<SelectState>,
    columns: Vec<TableColumn>,
    rows: Vec<Row>,
    source_name: Option<String>,
    notice: Option<Result<String, String>>,
}

/// Open the native dialog attached to an enabled extension command or alias.
pub fn open(app: &mut CadApp, name: &str) -> bool {
    let Some(command) = app.session.extension_command_ui(name) else { return false };
    let CommandUi::Table { title, description, submit_label, accent, selects, columns } = command.ui;
    let selects = selects.into_iter().map(|spec| SelectState { value: spec.default.clone(), spec }).collect();
    app.extension_table = State {
        command_id: command.command_id,
        extension_name: command.extension_name,
        title,
        description,
        submit_label,
        accent,
        selects,
        rows: vec![Row::empty(columns.len())],
        columns,
        source_name: None,
        notice: None,
    };
    app.ui.dialog = Some("extension-table".into());
    true
}

enum Action {
    Load,
    Submit,
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut state = std::mem::take(&mut app.extension_table);
    let mut action = None;
    let title = state.title.clone();
    egui::Window::new(title)
        .id(egui::Id::new("extension-table"))
        .open(open)
        .default_size(vec2(920.0, 460.0))
        .min_size(vec2(680.0, 360.0))
        .show(ctx, |ui| table_ui(ui, &mut state, &mut action));
    app.extension_table = state;

    match action {
        Some(Action::Load) => load_file(app),
        Some(Action::Submit) => submit(app, open),
        None => {}
    }
}

fn table_ui(ui: &mut egui::Ui, state: &mut State, action: &mut Option<Action>) {
    let accent = state.accent.as_deref().and_then(parse_color).unwrap_or_else(|| ui.visuals().hyperlink_color);
    ui.horizontal(|ui| {
        ui.colored_label(accent, RichText::new("●").size(20.0));
        ui.vertical(|ui| {
            ui.strong(&state.extension_name);
            ui.label(&state.description);
        });
    });
    ui.add_space(8.0);
    for select in &mut state.selects {
        ui.horizontal(|ui| {
            ui.strong(&select.spec.label);
            let selected =
                select.spec.choices.iter().find(|choice| choice.value == select.value).map_or(select.value.as_str(), |choice| choice.label.as_str());
            egui::ComboBox::from_id_salt(("extension-table-select", &select.spec.key)).selected_text(selected).width(330.0).show_ui(ui, |ui| {
                for choice in &select.spec.choices {
                    ui.selectable_value(&mut select.value, choice.value.clone(), &choice.label);
                }
            });
        });
    }
    if !state.selects.is_empty() {
        ui.add_space(4.0);
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Open CSV / text...").clicked() {
            *action = Some(Action::Load);
        }
        if ui.button("Add row").clicked() && state.rows.len() < MAX_ROWS {
            state.rows.push(Row::empty(state.columns.len()));
        }
        let selected = state.rows.iter().any(|row| row.selected);
        if ui.add_enabled(selected, egui::Button::new("Remove selected")).clicked() {
            state.rows.retain(|row| !row.selected);
            ensure_blank_row(state);
        }
        if ui.button("Sort by point number").clicked() {
            state.rows.sort_by(compare_rows);
            ensure_blank_row(state);
        }
        if ui.button("Clear").clicked() {
            state.rows = vec![Row::empty(state.columns.len())];
            state.source_name = None;
            state.notice = None;
        }
        if let Some(name) = &state.source_name {
            ui.weak(format!("Source: {name}"));
        }
    });
    ui.separator();

    let validation = validate(state);
    let error_color = ui.visuals().error_fg_color;
    let mut changed_last = false;
    egui::ScrollArea::both().max_height(290.0).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("extension-table-grid").striped(true).spacing(vec2(8.0, 5.0)).show(ui, |ui| {
            ui.strong("");
            ui.strong("#");
            for column in &state.columns {
                let required = if column.required { " *" } else { "" };
                ui.strong(format!("{}{required}", column.label));
            }
            ui.end_row();

            let row_count = state.rows.len();
            for (row_index, row) in state.rows.iter_mut().enumerate() {
                ui.checkbox(&mut row.selected, "");
                ui.weak((row_index + 1).to_string());
                for (column_index, column) in state.columns.iter().enumerate() {
                    let Some(value) = row.values.get_mut(column_index) else { continue };
                    let invalid = validation.cell_errors.get(row_index).and_then(|errors| errors.get(column_index)).and_then(Option::as_deref);
                    let width = match column.kind {
                        TableColumnKind::Text => 150.0,
                        TableColumnKind::Number => 125.0,
                    };
                    let mut edit = egui::TextEdit::singleline(value).desired_width(width);
                    if invalid.is_some() {
                        edit = edit.text_color(error_color);
                    }
                    let response = ui.add(edit);
                    let changed = response.changed();
                    if let Some(message) = invalid {
                        response.on_hover_text(message);
                    }
                    if changed && row_index + 1 == row_count {
                        changed_last = true;
                    }
                }
                ui.end_row();
            }
        });
    });
    if changed_last && state.rows.last().is_some_and(|row| !row.is_empty()) && state.rows.len() < MAX_ROWS {
        state.rows.push(Row::empty(state.columns.len()));
    }

    ui.separator();
    ui.horizontal(|ui| {
        if validation.invalid_rows == 0 {
            ui.colored_label(accent, format!("{} point{} ready", validation.valid_rows, if validation.valid_rows == 1 { "" } else { "s" }));
        } else {
            ui.colored_label(
                error_color,
                format!("{} row{} need attention", validation.invalid_rows, if validation.invalid_rows == 1 { "" } else { "s" }),
            );
        }
        ui.separator();
        ui.weak("Required fields are marked with *");
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
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_submit = validation.valid_rows > 0 && validation.invalid_rows == 0;
        if ui.add_enabled(can_submit, egui::Button::new(&state.submit_label).fill(accent)).clicked() {
            *action = Some(Action::Submit);
        }
        ui.weak(format!("Maximum {MAX_ROWS} points per insertion"));
    });
}

fn load_file(app: &mut CadApp) {
    let result = app
        .services
        .pick_extension_table
        .as_ref()
        .ok_or_else(|| "CSV file selection is unavailable on this platform".to_string())
        .and_then(|pick| pick());
    match result {
        Ok(Some(source)) => match parse_table(&source, &app.extension_table.columns) {
            Ok(rows) => {
                let count = rows.iter().filter(|row| !row.is_empty()).count();
                app.extension_table.rows = rows;
                ensure_blank_row(&mut app.extension_table);
                app.extension_table.source_name = Some(source.name);
                app.extension_table.notice = Some(Ok(format!("Loaded {count} point{}", if count == 1 { "" } else { "s" })));
            }
            Err(error) => app.extension_table.notice = Some(Err(error)),
        },
        Ok(None) => {}
        Err(error) => app.extension_table.notice = Some(Err(error)),
    }
}

fn submit(app: &mut CadApp, open: &mut bool) {
    let params = match parameters(&app.extension_table) {
        Ok(params) => params,
        Err(error) => {
            app.extension_table.notice = Some(Err(error));
            return;
        }
    };
    let command = app.extension_table.command_id.clone();
    match app.run(&command, params) {
        Ok(result) => {
            let inserted = result.get("inserted").and_then(Value::as_u64);
            let message = inserted.map_or_else(
                || "Extension command completed".to_string(),
                |count| format!("Inserted {count} survey point{}", if count == 1 { "" } else { "s" }),
            );
            app.session.echo(message.clone());
            app.set_status(message);
            app.canvas.zoom_pending = true;
            *open = false;
        }
        Err(error) => app.extension_table.notice = Some(Err(error)),
    }
}

#[derive(Debug)]
struct Validation {
    valid_rows: usize,
    invalid_rows: usize,
    cell_errors: Vec<Vec<Option<String>>>,
}

fn validate(state: &State) -> Validation {
    let mut errors = vec![vec![None; state.columns.len()]; state.rows.len()];
    for (row_index, row) in state.rows.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        for (column_index, column) in state.columns.iter().enumerate() {
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
    for (column_index, column) in state.columns.iter().enumerate().filter(|(_, column)| column.unique) {
        let mut seen: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (row_index, row) in state.rows.iter().enumerate().filter(|(_, row)| !row.is_empty()) {
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
    let mut valid_rows = 0;
    let mut invalid_rows = 0;
    for (row, row_errors) in state.rows.iter().zip(&errors) {
        if row.is_empty() {
            continue;
        }
        if row_errors.iter().any(Option::is_some) {
            invalid_rows += 1;
        } else {
            valid_rows += 1;
        }
    }
    Validation { valid_rows, invalid_rows, cell_errors: errors }
}

fn parameters(state: &State) -> Result<Value, String> {
    let validation = validate(state);
    if validation.valid_rows == 0 || validation.invalid_rows > 0 {
        return Err("Fix invalid rows before inserting points".into());
    }
    let mut rows = Vec::with_capacity(validation.valid_rows);
    for row in state.rows.iter().filter(|row| !row.is_empty()) {
        let mut object = Map::new();
        for (column_index, column) in state.columns.iter().enumerate() {
            let raw = row.values.get(column_index).map_or("", String::as_str).trim();
            let value = match column.kind {
                TableColumnKind::Text => Value::String(raw.to_owned()),
                TableColumnKind::Number => {
                    let number = parse_number(raw).ok_or_else(|| format!("{} is not a finite decimal number", column.label))?;
                    Value::from(number)
                }
            };
            object.insert(column.key.clone(), value);
        }
        rows.push(Value::Object(object));
    }
    let mut params = Map::new();
    for select in &state.selects {
        params.insert(select.spec.key.clone(), Value::String(select.value.clone()));
    }
    params.insert("rows".into(), Value::Array(rows));
    Ok(Value::Object(params))
}

fn parse_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let normalized = if trimmed.contains(',') && !trimmed.contains('.') { trimmed.replace(',', ".") } else { trimmed.to_owned() };
    normalized.parse::<f64>().ok().filter(|number| number.is_finite())
}

fn compare_rows(left: &Row, right: &Row) -> Ordering {
    if left.is_empty() != right.is_empty() {
        return left.is_empty().cmp(&right.is_empty());
    }
    let left = left.values.first().map_or("", String::as_str).trim();
    let right = right.values.first().map_or("", String::as_str).trim();
    match (parse_number(left), parse_number(right)) {
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        _ => left.to_lowercase().cmp(&right.to_lowercase()),
    }
}

fn ensure_blank_row(state: &mut State) {
    if (state.rows.is_empty() || state.rows.last().is_some_and(|row| !row.is_empty())) && state.rows.len() < MAX_ROWS {
        state.rows.push(Row::empty(state.columns.len()));
    }
}

fn parse_table(source: &LoadedTextTable, columns: &[TableColumn]) -> Result<Vec<Row>, String> {
    let first_line = source.text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default();
    let delimiter = [(b';', first_line.matches(';').count()), (b',', first_line.matches(',').count()), (b'\t', first_line.matches('\t').count())]
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count > 0)
        .map_or(b',', |(delimiter, _)| delimiter);
    let mut reader = ReaderBuilder::new().has_headers(false).delimiter(delimiter).trim(Trim::All).flexible(true).from_reader(source.text.as_bytes());
    let mut records = Vec::new();
    for record in reader.records() {
        if records.len() > MAX_ROWS {
            return Err(format!("File contains more than {MAX_ROWS} rows"));
        }
        let record = record.map_err(|error| format!("Cannot read {}: {error}", source.name))?;
        if record.iter().any(|field| field.len() > MAX_FIELD_BYTES) {
            return Err(format!("{} contains a field larger than {MAX_FIELD_BYTES} bytes", source.name));
        }
        if record.iter().any(|field| !field.trim().is_empty()) {
            records.push(record);
        }
    }
    let Some(first) = records.first() else { return Err(format!("{} contains no point rows", source.name)) };
    let mapping = header_mapping(first.iter(), columns)?;
    let start = usize::from(mapping.is_some());
    let indices: Vec<Option<usize>> = mapping.unwrap_or_else(|| (0..columns.len()).map(Some).collect());
    let mut rows = Vec::new();
    for record in records.into_iter().skip(start) {
        if rows.len() >= MAX_ROWS {
            return Err(format!("File contains more than {MAX_ROWS} point rows"));
        }
        let values = indices.iter().map(|index| index.and_then(|index| record.get(index)).unwrap_or_default().trim().to_owned()).collect();
        rows.push(Row { selected: false, values });
    }
    if rows.is_empty() {
        return Err(format!("{} contains a header but no point rows", source.name));
    }
    Ok(rows)
}

fn header_mapping<'a>(headers: impl Iterator<Item = &'a str>, columns: &[TableColumn]) -> Result<Option<Vec<Option<usize>>>, String> {
    let headers: Vec<String> = headers.map(normalize_header).collect();
    let mut mapping = Vec::with_capacity(columns.len());
    let mut matched = 0usize;
    for column in columns {
        let names =
            std::iter::once(column.key.as_str()).chain(std::iter::once(column.label.as_str())).chain(column.aliases.iter().map(String::as_str));
        let index = names.map(normalize_header).find_map(|name| headers.iter().position(|header| *header == name));
        if index.is_some() {
            matched += 1;
        }
        mapping.push(index);
    }
    if matched == 0 {
        return Ok(None);
    }
    for (column, index) in columns.iter().zip(&mapping) {
        if column.required && index.is_none() {
            return Err(format!("CSV header is missing required column '{}'", column.label));
        }
    }
    Ok(Some(mapping))
}

fn normalize_header(value: &str) -> String {
    value.trim_start_matches('\u{feff}').chars().filter(|character| character.is_alphanumeric()).flat_map(char::to_lowercase).collect()
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
    use cadcraft_extensions::{TableColumn, TableColumnKind, TableSelect, TableSelectChoice};
    use serde_json::json;

    use super::{LoadedTextTable, Row, SelectState, State, parameters, parse_table, validate};

    fn columns() -> Vec<TableColumn> {
        vec![
            TableColumn {
                key: "number".into(),
                label: "Point number".into(),
                kind: TableColumnKind::Text,
                required: true,
                unique: true,
                aliases: vec!["point".into()],
            },
            TableColumn {
                key: "northing".into(),
                label: "Northing".into(),
                kind: TableColumnKind::Number,
                required: true,
                unique: false,
                aliases: vec!["n".into(), "y".into()],
            },
            TableColumn {
                key: "easting".into(),
                label: "Easting".into(),
                kind: TableColumnKind::Number,
                required: true,
                unique: false,
                aliases: vec!["e".into(), "x".into()],
            },
        ]
    }

    #[test]
    fn semicolon_csv_headers_map_columns_and_decimal_commas_validate() {
        let source = LoadedTextTable { name: "points.csv".into(), text: "Point;E;N\n1;500000,25;300000,5\n".into() };
        let rows = parse_table(&source, &columns());
        assert!(rows.is_ok());
        let state = State { columns: columns(), rows: rows.ok().unwrap_or_default(), ..State::default() };
        let validation = validate(&state);
        assert_eq!((validation.valid_rows, validation.invalid_rows), (1, 0));
        assert_eq!(state.rows.first().and_then(|row| row.values.get(1)).map(String::as_str), Some("300000,5"));
    }

    #[test]
    fn duplicate_point_numbers_block_submission() {
        let source = LoadedTextTable { name: "points.csv".into(), text: "Point,N,E\nP1,1,2\np1,3,4\n".into() };
        let rows = parse_table(&source, &columns());
        assert!(rows.is_ok());
        let state = State { columns: columns(), rows: rows.ok().unwrap_or_default(), ..State::default() };
        let validation = validate(&state);
        assert_eq!((validation.valid_rows, validation.invalid_rows), (0, 2));
    }

    #[test]
    fn selector_values_are_submitted_next_to_rows() {
        let columns = columns();
        let state = State {
            selects: vec![SelectState {
                spec: TableSelect {
                    key: "convention".into(),
                    label: "Coordinate convention".into(),
                    default: "cad-xy".into(),
                    choices: vec![
                        TableSelectChoice { value: "cad-xy".into(), label: "CAD / GIS".into() },
                        TableSelectChoice { value: "survey-xy".into(), label: "Survey / TopoLT".into() },
                    ],
                },
                value: "survey-xy".into(),
            }],
            rows: vec![Row { selected: false, values: vec!["L1".into(), "337275.053".into(), "595286.485".into()] }],
            columns,
            ..State::default()
        };

        let params = parameters(&state);

        assert_eq!(
            params.ok(),
            Some(json!({
                "convention": "survey-xy",
                "rows": [{"number": "L1", "northing": 337275.053, "easting": 595286.485}]
            }))
        );
    }

    #[test]
    fn literal_xy_headers_are_mapped_by_name_even_when_the_file_lists_y_first() {
        let columns = vec![
            TableColumn {
                key: "number".into(),
                label: "Point number".into(),
                kind: TableColumnKind::Text,
                required: true,
                unique: true,
                aliases: vec!["code".into()],
            },
            TableColumn {
                key: "x".into(),
                label: "Input X".into(),
                kind: TableColumnKind::Number,
                required: true,
                unique: false,
                aliases: vec!["easting".into()],
            },
            TableColumn {
                key: "y".into(),
                label: "Input Y".into(),
                kind: TableColumnKind::Number,
                required: true,
                unique: false,
                aliases: vec!["northing".into()],
            },
        ];
        let source = LoadedTextTable { name: "topolt.csv".into(), text: "Y,X,Code\n595286.485,337275.053,L1\n".into() };

        let rows = parse_table(&source, &columns);

        assert!(rows.is_ok());
        assert_eq!(
            rows.ok().and_then(|rows| rows.first().cloned()).map(|row| row.values),
            Some(vec!["L1".into(), "337275.053".into(), "595286.485".into()])
        );
    }
}
