//! Native WebAssembly component host for schema 2 extensions.
//!
//! Guest drawing calls are staged in host memory. CADCraft applies them only after the guest
//! returns successfully with a committed transaction, so traps, fuel exhaustion and protocol
//! errors leave the drawing byte-for-byte unchanged.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadcraft_doc::{Drawing, Entity, EntityKind, HAlign, Handle, Line, Point, Text, VAlign};
use cadcraft_extensions::Permission;
use cadcraft_geom::Vec3;
use serde_json::Value;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::{EngineError, Result as EngineResult, Session, Snapshot};

wasmtime::component::bindgen!({
    path: "wit",
    world: "cadcraft-extension",
});

const MAX_FUEL: u64 = 10_000_000;
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_OPERATIONS: usize = 10_000;
const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_LABEL_BYTES: usize = 128;
const MAX_LAYER_BYTES: usize = 255;
const MAX_TEXT_BYTES: usize = 1_024;
const MAX_COORDINATE: f64 = 1.0e15;
const MAX_METADATA_KEY_BYTES: usize = 128;
const MAX_ENTITY_METADATA_VALUE_BYTES: usize = 4 * 1024;
const MAX_DRAWING_METADATA_VALUE_BYTES: usize = MAX_JSON_BYTES;
const MAX_QUERY_RESULTS: usize = 10_000;

#[derive(Clone, Debug)]
struct PendingLine {
    start: Vec3,
    end: Vec3,
    layer: String,
}

#[derive(Clone, Debug)]
struct PendingPoint {
    position: Vec3,
    layer: String,
}

#[derive(Clone, Debug)]
struct PendingText {
    position: Vec3,
    value: String,
    height: f64,
    rotation: f64,
    layer: String,
}

#[derive(Clone, Debug)]
enum PendingEntity {
    Line(PendingLine),
    Point(PendingPoint),
    Text(PendingText),
}

#[derive(Clone, Debug)]
enum PendingOperation {
    Create { id: u64, entity: PendingEntity, metadata: BTreeMap<String, String> },
    SetDrawingMetadata { key: String, value: Option<String> },
    SetEntityMetadata { id: u64, key: String, value: Option<String> },
    UpdatePoint { id: u64, point: PendingPoint },
    UpdateText { id: u64, text: PendingText },
    DeleteEntity { id: u64 },
}

#[derive(Debug)]
struct Transaction {
    id: u64,
    label: String,
    extension_id: String,
    operations: Vec<PendingOperation>,
}

struct HostState {
    drawing_read: bool,
    drawing_write: bool,
    extension_id: String,
    document: Arc<Drawing>,
    transaction: Option<Transaction>,
    committed: Option<Transaction>,
    next_entity_id: u64,
    limits: StoreLimits,
}

impl HostState {
    fn new(permissions: &[Permission], extension_id: &str, document: Arc<Drawing>) -> Self {
        let next_entity_id = document.handseed;
        Self {
            drawing_read: permissions.contains(&Permission::DrawingRead),
            drawing_write: permissions.contains(&Permission::DrawingWrite),
            extension_id: extension_id.to_owned(),
            document,
            transaction: None,
            committed: None,
            next_entity_id,
            limits: StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).table_elements(MAX_OPERATIONS).instances(4).tables(8).memories(4).build(),
        }
    }

    fn require_read(&self) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        if self.drawing_read {
            Ok(())
        } else {
            Err(cadcraft::extension::drawing::HostError::PermissionDenied("drawing-read permission is required".into()))
        }
    }

    fn require_write(&self) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        if self.drawing_write {
            Ok(())
        } else {
            Err(cadcraft::extension::drawing::HostError::PermissionDenied("drawing-write permission is required".into()))
        }
    }

    fn push_operation(&mut self, transaction: u64, operation: PendingOperation) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        let Some(active) = self.transaction.as_mut().filter(|active| active.id == transaction) else {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction is not active".into()));
        };
        if active.operations.len() >= MAX_OPERATIONS {
            return Err(cadcraft::extension::drawing::HostError::DrawingFailure(format!("transaction exceeds {MAX_OPERATIONS} drawing operations")));
        }
        active.operations.push(operation);
        Ok(())
    }

    fn push_entity(&mut self, transaction: u64, entity: PendingEntity) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        let entity_id = self.next_entity_id;
        if entity_id == u64::MAX {
            return Err(cadcraft::extension::drawing::HostError::DrawingFailure("drawing has no free entity handles".into()));
        }
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        self.push_operation(transaction, PendingOperation::Create { id: entity_id, entity, metadata: BTreeMap::new() })?;
        Ok(entity_id)
    }

    fn entity_exists(&self, id: u64) -> bool {
        self.document.entity(Handle(id)).is_some()
            || self.transaction.as_ref().is_some_and(|transaction| {
                transaction.operations.iter().any(|operation| matches!(operation, PendingOperation::Create { id: created, .. } if *created == id))
            })
    }
}

fn point3(point: &cadcraft::extension::geometry::Point3) -> std::result::Result<Vec3, cadcraft::extension::drawing::HostError> {
    let coordinates = [point.x, point.y, point.z];
    if coordinates.iter().any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE) {
        Err(cadcraft::extension::drawing::HostError::InvalidInput("coordinates must be finite and within the supported drawing range".into()))
    } else {
        Ok(Vec3::new(point.x, point.y, point.z))
    }
}

fn layer(value: &str) -> std::result::Result<String, cadcraft::extension::drawing::HostError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_LAYER_BYTES || value.chars().any(char::is_control) {
        Err(cadcraft::extension::drawing::HostError::InvalidInput("layer must contain 1 to 255 printable bytes".into()))
    } else {
        Ok(value.to_owned())
    }
}

fn metadata_key(value: &str) -> std::result::Result<String, cadcraft::extension::drawing::HostError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_METADATA_KEY_BYTES || value.chars().any(char::is_control) {
        Err(cadcraft::extension::drawing::HostError::InvalidInput("metadata key must contain 1 to 128 printable bytes".into()))
    } else {
        Ok(value.to_owned())
    }
}

fn metadata_value(value: String, maximum: usize) -> std::result::Result<String, cadcraft::extension::drawing::HostError> {
    if value.len() > maximum || value.contains('\0') {
        Err(cadcraft::extension::drawing::HostError::InvalidInput(format!("metadata value exceeds {maximum} bytes or contains a null character")))
    } else {
        Ok(value)
    }
}

fn entity_view(entity: &Entity) -> cadcraft::extension::geometry::EntityView {
    use cadcraft::extension::geometry::EntityKind as ViewKind;

    let (kind, position, value) = match &entity.kind {
        EntityKind::Line(_) => (ViewKind::Line, None, None),
        EntityKind::Point(point) => (ViewKind::Point, Some(wit_point3(point.p)), None),
        EntityKind::Text(text) => (ViewKind::Text, Some(wit_point3(text.insert)), Some(text.value.clone())),
        _ => (ViewKind::Other, None, None),
    };
    cadcraft::extension::geometry::EntityView { id: entity.handle.0, kind, layer: entity.common.layer.clone(), position, value }
}

fn wit_point3(point: Vec3) -> cadcraft::extension::geometry::Point3 {
    cadcraft::extension::geometry::Point3 { x: point.x, y: point.y, z: point.z }
}

impl cadcraft::extension::geometry::Host for HostState {}

impl cadcraft::extension::drawing::Host for HostState {
    fn begin_transaction(&mut self, label: String) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if self.transaction.is_some() || self.committed.is_some() {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("a command may open exactly one transaction".into()));
        }
        let label = label.trim();
        if label.is_empty() || label.len() > MAX_LABEL_BYTES || label.chars().any(char::is_control) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction label must contain 1 to 128 printable bytes".into()));
        }
        self.transaction = Some(Transaction { id: 1, label: label.to_owned(), extension_id: self.extension_id.clone(), operations: Vec::new() });
        Ok(1)
    }

    fn create_line(
        &mut self,
        transaction: u64,
        request: cadcraft::extension::geometry::LineRequest,
    ) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let pending = PendingLine { start: point3(&request.start)?, end: point3(&request.end)?, layer: layer(&request.layer)? };
        self.push_entity(transaction, PendingEntity::Line(pending))
    }

    fn create_point(
        &mut self,
        transaction: u64,
        request: cadcraft::extension::geometry::PointRequest,
    ) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let pending = PendingPoint { position: point3(&request.position)?, layer: layer(&request.layer)? };
        self.push_entity(transaction, PendingEntity::Point(pending))
    }

    fn create_text(
        &mut self,
        transaction: u64,
        request: cadcraft::extension::geometry::TextRequest,
    ) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let value = request.value.trim();
        if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text must contain 1 to 1024 printable bytes".into()));
        }
        if !request.height.is_finite() || request.height <= 0.0 || request.height > MAX_COORDINATE {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text height must be finite and positive".into()));
        }
        if !request.rotation_degrees.is_finite() {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text rotation must be finite".into()));
        }
        let pending = PendingText {
            position: point3(&request.position)?,
            value: value.to_owned(),
            height: request.height,
            rotation: request.rotation_degrees.to_radians(),
            layer: layer(&request.layer)?,
        };
        self.push_entity(transaction, PendingEntity::Text(pending))
    }

    fn get_drawing_metadata(&mut self, key: String) -> std::result::Result<Option<String>, cadcraft::extension::drawing::HostError> {
        self.require_read()?;
        let key = metadata_key(&key)?;
        Ok(self.document.extension_data.get(&self.extension_id).and_then(|data| data.get(&key)).cloned())
    }

    fn set_drawing_metadata(
        &mut self,
        transaction: u64,
        key: String,
        value: String,
    ) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let key = metadata_key(&key)?;
        let value = metadata_value(value, MAX_DRAWING_METADATA_VALUE_BYTES)?;
        self.push_operation(transaction, PendingOperation::SetDrawingMetadata { key, value: Some(value) })
    }

    fn remove_drawing_metadata(&mut self, transaction: u64, key: String) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let key = metadata_key(&key)?;
        self.push_operation(transaction, PendingOperation::SetDrawingMetadata { key, value: None })
    }

    fn get_entity(
        &mut self,
        entity: u64,
    ) -> std::result::Result<Option<cadcraft::extension::geometry::EntityView>, cadcraft::extension::drawing::HostError> {
        self.require_read()?;
        Ok(self.document.entity(Handle(entity)).map(|entity| entity_view(entity)))
    }

    fn get_entity_metadata(&mut self, entity: u64, key: String) -> std::result::Result<Option<String>, cadcraft::extension::drawing::HostError> {
        self.require_read()?;
        let key = metadata_key(&key)?;
        Ok(self
            .document
            .entity(Handle(entity))
            .and_then(|entity| entity.common.extension_data.get(&self.extension_id))
            .and_then(|data| data.get(&key))
            .cloned())
    }

    fn find_entities_by_metadata(
        &mut self,
        key: String,
        value: String,
    ) -> std::result::Result<Vec<cadcraft::extension::geometry::EntityView>, cadcraft::extension::drawing::HostError> {
        self.require_read()?;
        let key = metadata_key(&key)?;
        let value = metadata_value(value, MAX_ENTITY_METADATA_VALUE_BYTES)?;
        let matches = self
            .document
            .model
            .iter()
            .chain(self.document.layouts.iter().flat_map(|layout| layout.entities.iter()))
            .filter(|entity| {
                entity.common.extension_data.get(&self.extension_id).and_then(|data| data.get(&key)).is_some_and(|stored| stored == &value)
            })
            .take(MAX_QUERY_RESULTS.saturating_add(1))
            .map(|entity| entity_view(entity))
            .collect::<Vec<_>>();
        if matches.len() > MAX_QUERY_RESULTS {
            Err(cadcraft::extension::drawing::HostError::DrawingFailure(format!("metadata query exceeds {MAX_QUERY_RESULTS} entities")))
        } else {
            Ok(matches)
        }
    }

    fn set_entity_metadata(
        &mut self,
        transaction: u64,
        entity: u64,
        key: String,
        value: String,
    ) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if !self.entity_exists(entity) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("entity does not exist".into()));
        }
        let key = metadata_key(&key)?;
        let value = metadata_value(value, MAX_ENTITY_METADATA_VALUE_BYTES)?;
        if let Some(PendingOperation::Create { metadata, .. }) =
            self.transaction.as_mut().filter(|active| active.id == transaction).and_then(|active| {
                active.operations.iter_mut().find(|operation| matches!(operation, PendingOperation::Create { id: created, .. } if *created == entity))
            })
        {
            metadata.insert(key, value);
            return Ok(());
        }
        self.push_operation(transaction, PendingOperation::SetEntityMetadata { id: entity, key, value: Some(value) })
    }

    fn remove_entity_metadata(
        &mut self,
        transaction: u64,
        entity: u64,
        key: String,
    ) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if !self.entity_exists(entity) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("entity does not exist".into()));
        }
        let key = metadata_key(&key)?;
        if let Some(PendingOperation::Create { metadata, .. }) =
            self.transaction.as_mut().filter(|active| active.id == transaction).and_then(|active| {
                active.operations.iter_mut().find(|operation| matches!(operation, PendingOperation::Create { id: created, .. } if *created == entity))
            })
        {
            metadata.remove(&key);
            return Ok(());
        }
        self.push_operation(transaction, PendingOperation::SetEntityMetadata { id: entity, key, value: None })
    }

    fn update_point(
        &mut self,
        transaction: u64,
        entity: u64,
        request: cadcraft::extension::geometry::PointRequest,
    ) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if !self.entity_exists(entity) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("entity does not exist".into()));
        }
        let point = PendingPoint { position: point3(&request.position)?, layer: layer(&request.layer)? };
        self.push_operation(transaction, PendingOperation::UpdatePoint { id: entity, point })
    }

    fn update_text(
        &mut self,
        transaction: u64,
        entity: u64,
        request: cadcraft::extension::geometry::TextRequest,
    ) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if !self.entity_exists(entity) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("entity does not exist".into()));
        }
        let value = request.value.trim();
        if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text must contain 1 to 1024 printable bytes".into()));
        }
        if !request.height.is_finite() || request.height <= 0.0 || request.height > MAX_COORDINATE {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text height must be finite and positive".into()));
        }
        if !request.rotation_degrees.is_finite() {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("text rotation must be finite".into()));
        }
        let text = PendingText {
            position: point3(&request.position)?,
            value: value.to_owned(),
            height: request.height,
            rotation: request.rotation_degrees.to_radians(),
            layer: layer(&request.layer)?,
        };
        self.push_operation(transaction, PendingOperation::UpdateText { id: entity, text })
    }

    fn delete_entity(&mut self, transaction: u64, entity: u64) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        if !self.entity_exists(entity) {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("entity does not exist".into()));
        }
        self.push_operation(transaction, PendingOperation::DeleteEntity { id: entity })
    }

    fn commit_transaction(&mut self, transaction: u64) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let Some(active) = self.transaction.take() else {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction is not active".into()));
        };
        if active.id != transaction {
            self.transaction = Some(active);
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction id does not match".into()));
        }
        self.committed = Some(active);
        Ok(())
    }

    fn rollback_transaction(&mut self, transaction: u64) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        self.require_write()?;
        let Some(active) = self.transaction.take() else {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction is not active".into()));
        };
        if active.id != transaction {
            self.transaction = Some(active);
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction id does not match".into()));
        }
        Ok(())
    }
}

struct WasmOutcome {
    value: Value,
    transaction: Transaction,
}

fn execute_component(
    bytes: &[u8],
    command: &str,
    parameters: &Value,
    permissions: &[Permission],
    extension_id: &str,
    document: Arc<Drawing>,
) -> EngineResult<WasmOutcome> {
    let parameters = serde_json::to_string(parameters).map_err(|error| EngineError::Other(format!("cannot encode extension parameters: {error}")))?;
    if parameters.len() > MAX_JSON_BYTES {
        return Err(EngineError::Other(format!("extension parameters exceed {MAX_JSON_BYTES} bytes")));
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    let engine = Engine::new(&config).map_err(|error| EngineError::Other(format!("cannot initialize WebAssembly runtime: {error}")))?;
    let component = Component::new(&engine, bytes).map_err(|error| EngineError::Other(format!("invalid WebAssembly component: {error}")))?;
    let mut linker = Linker::<HostState>::new(&engine);
    CadcraftExtension::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
        .map_err(|error| EngineError::Other(format!("cannot link WebAssembly component: {error}")))?;
    let mut store = Store::new(&engine, HostState::new(permissions, extension_id, document));
    store.limiter(|state| &mut state.limits);
    store.set_fuel(MAX_FUEL).map_err(|error| EngineError::Other(format!("cannot limit WebAssembly component: {error}")))?;
    let bindings = CadcraftExtension::instantiate(&mut store, &component, &linker)
        .map_err(|error| EngineError::Other(format!("cannot instantiate WebAssembly component: {error}")))?;
    let guest_result = bindings
        .call_run_command(&mut store, command, &parameters)
        .map_err(|error| EngineError::Other(format!("WebAssembly extension trapped: {error}")))?;
    let output = guest_result.map_err(|message| EngineError::Other(format!("WebAssembly extension failed: {message}")))?;
    if output.len() > MAX_JSON_BYTES {
        return Err(EngineError::Other(format!("extension result exceeds {MAX_JSON_BYTES} bytes")));
    }
    let value = serde_json::from_str(&output).map_err(|error| EngineError::Other(format!("extension returned invalid JSON: {error}")))?;
    let state = store.into_data();
    if state.transaction.is_some() {
        return Err(EngineError::Other("WebAssembly extension returned with an open transaction".into()));
    }
    let transaction = state.committed.ok_or_else(|| EngineError::Other("WebAssembly extension returned without committing a transaction".into()))?;
    Ok(WasmOutcome { value, transaction })
}

impl Session {
    pub(crate) fn wasm_execute_extension(
        &mut self,
        extension_id: &str,
        bytes: &[u8],
        command: &str,
        params: &Value,
        permissions: &[Permission],
    ) -> EngineResult<Value> {
        if self.state()?.block_edit.is_some() {
            return Err(EngineError::Other("WebAssembly extension commands are not available inside the Block Editor yet".into()));
        }
        let document = self.state()?.doc.clone();
        let outcome = execute_component(bytes, command, params, permissions, extension_id, document)?;
        self.apply_wasm_transaction(outcome.transaction)?;
        Ok(outcome.value)
    }

    fn apply_wasm_transaction(&mut self, transaction: Transaction) -> EngineResult<()> {
        let Transaction { label, extension_id, operations, .. } = transaction;
        if operations.is_empty() {
            return Ok(());
        }
        let state = self.state()?;
        let before = state.doc.clone();
        let selection = state.selection.clone();
        let uid = state.uid;
        let space = self.space();
        let base_common = self.current_common();
        let mut drawing = (*before).clone();
        let text_style = drawing.header.str("TEXTSTYLE", "Standard");
        for operation in operations {
            match operation {
                PendingOperation::Create { id, entity, metadata } => {
                    let mut common = base_common.clone();
                    if !metadata.is_empty() {
                        common.extension_data.insert(extension_id.clone(), metadata);
                    }
                    let kind = match entity {
                        PendingEntity::Line(line) => {
                            common.layer = line.layer;
                            EntityKind::Line(Line { a: line.start, b: line.end })
                        }
                        PendingEntity::Point(point) => {
                            common.layer = point.layer;
                            EntityKind::Point(Point { p: point.position, angle: 0.0 })
                        }
                        PendingEntity::Text(text) => {
                            common.layer = text.layer;
                            EntityKind::Text(Text {
                                insert: text.position,
                                align_pt: None,
                                height: text.height,
                                value: text.value,
                                rotation: text.rotation,
                                width_factor: 1.0,
                                oblique: 0.0,
                                style: text_style.clone(),
                                halign: HAlign::Left,
                                valign: VAlign::Baseline,
                            })
                        }
                    };
                    let actual = drawing.add(&space, common, kind)?;
                    if actual.0 != id {
                        return Err(EngineError::Other("drawing changed while reserving extension entity handles".into()));
                    }
                }
                PendingOperation::SetDrawingMetadata { key, value } => {
                    let data = drawing.extension_data.entry(extension_id.clone()).or_default();
                    match value {
                        Some(value) => {
                            data.insert(key, value);
                        }
                        None => {
                            data.remove(&key);
                        }
                    }
                    if data.is_empty() {
                        drawing.extension_data.remove(&extension_id);
                    }
                }
                PendingOperation::SetEntityMetadata { id, key, value } => {
                    drawing.modify_entity(Handle(id), |entity| {
                        let data = entity.common.extension_data.entry(extension_id.clone()).or_default();
                        match value {
                            Some(value) => {
                                data.insert(key, value);
                            }
                            None => {
                                data.remove(&key);
                            }
                        }
                        if data.is_empty() {
                            entity.common.extension_data.remove(&extension_id);
                        }
                    })?;
                }
                PendingOperation::UpdatePoint { id, point } => {
                    if !matches!(drawing.entity(Handle(id)).map(|entity| &entity.kind), Some(EntityKind::Point(_))) {
                        return Err(EngineError::Other(format!("extension cannot update entity {id:X} as a point")));
                    }
                    drawing.ensure_layer(&point.layer);
                    drawing.modify_entity(Handle(id), |entity| {
                        entity.common.layer = point.layer;
                        if let EntityKind::Point(existing) = &mut entity.kind {
                            existing.p = point.position;
                        }
                    })?;
                }
                PendingOperation::UpdateText { id, text } => {
                    if !matches!(drawing.entity(Handle(id)).map(|entity| &entity.kind), Some(EntityKind::Text(_))) {
                        return Err(EngineError::Other(format!("extension cannot update entity {id:X} as text")));
                    }
                    drawing.ensure_layer(&text.layer);
                    drawing.modify_entity(Handle(id), |entity| {
                        entity.common.layer = text.layer;
                        if let EntityKind::Text(existing) = &mut entity.kind {
                            existing.insert = text.position;
                            existing.value = text.value;
                            existing.height = text.height;
                            existing.rotation = text.rotation;
                        }
                    })?;
                }
                PendingOperation::DeleteEntity { id } => {
                    if drawing.remove_entity(Handle(id)).is_none() {
                        return Err(EngineError::Other(format!("extension cannot delete missing entity {id:X}")));
                    }
                }
            }
        }
        let state = self.state_mut()?;
        state.doc = Arc::new(drawing);
        state.push_undo(Snapshot { label, doc: before, selection });
        state.revision = state.revision.saturating_add(1);
        if state.uid != uid {
            return Err(EngineError::Other("active drawing changed while applying an extension transaction".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cadcraft_doc::Drawing;
    use cadcraft_extensions::Permission;

    use super::{HostState, MAX_OPERATIONS, PendingEntity, PendingLine, PendingOperation, PendingPoint, PendingText, Transaction, cadcraft};
    use crate::Session;
    use crate::wasm_runtime::cadcraft::extension::drawing::Host as _;

    fn host(permissions: &[Permission]) -> HostState {
        HostState::new(permissions, "org.example.extension", Arc::new(Drawing::new_metric()))
    }

    #[test]
    fn host_requires_permission_and_a_matching_transaction() {
        let mut denied = host(&[]);
        let denied_result = denied.begin_transaction("Test".into());
        assert!(matches!(denied_result, Err(cadcraft::extension::drawing::HostError::PermissionDenied(_))));

        let mut host = host(&[Permission::DrawingWrite]);
        assert!(matches!(host.begin_transaction("Test".into()), Ok(1)));
        assert!(matches!(host.commit_transaction(2), Err(cadcraft::extension::drawing::HostError::InvalidInput(_))));
        assert!(matches!(host.rollback_transaction(1), Ok(())));
    }

    #[test]
    fn host_rejects_non_finite_lines_and_bounds_operations() {
        let mut host = host(&[Permission::DrawingWrite]);
        assert!(matches!(host.begin_transaction("Test".into()), Ok(1)));
        let invalid = cadcraft::extension::geometry::LineRequest {
            start: cadcraft::extension::geometry::Point3 { x: f64::NAN, y: 0.0, z: 0.0 },
            end: cadcraft::extension::geometry::Point3 { x: 1.0, y: 0.0, z: 0.0 },
            layer: "0".into(),
        };
        assert!(matches!(host.create_line(1, invalid), Err(cadcraft::extension::drawing::HostError::InvalidInput(_))));
        if let Some(transaction) = host.transaction.as_mut() {
            transaction.operations = Vec::with_capacity(MAX_OPERATIONS);
            transaction.operations.resize_with(MAX_OPERATIONS, || PendingOperation::DeleteEntity { id: 1 });
        }
        let bounded = cadcraft::extension::geometry::LineRequest {
            start: cadcraft::extension::geometry::Point3 { x: 0.0, y: 0.0, z: 0.0 },
            end: cadcraft::extension::geometry::Point3 { x: 1.0, y: 0.0, z: 0.0 },
            layer: "0".into(),
        };
        assert!(matches!(host.create_line(1, bounded), Err(cadcraft::extension::drawing::HostError::DrawingFailure(_))));
    }

    #[test]
    fn committed_operations_apply_as_one_undo_step() {
        let mut session = Session::new();
        let before = session.state().map(|state| state.doc.clone());
        assert!(before.is_ok());
        let transaction = Transaction {
            id: 1,
            label: "Extension reference entities".into(),
            extension_id: "org.example.extension".into(),
            operations: vec![
                PendingOperation::Create {
                    id: 0x100,
                    entity: PendingEntity::Line(PendingLine {
                        start: cadcraft_geom::Vec3::ZERO,
                        end: cadcraft_geom::Vec3::new(10.0, 0.0, 0.0),
                        layer: "SURVEY_REFERENCE".into(),
                    }),
                    metadata: Default::default(),
                },
                PendingOperation::Create {
                    id: 0x101,
                    entity: PendingEntity::Point(PendingPoint { position: cadcraft_geom::Vec3::new(5.0, 2.0, 1.0), layer: "SURVEY_POINTS".into() }),
                    metadata: Default::default(),
                },
                PendingOperation::Create {
                    id: 0x102,
                    entity: PendingEntity::Text(PendingText {
                        position: cadcraft_geom::Vec3::new(5.25, 2.25, 1.0),
                        value: "101".into(),
                        height: 0.25,
                        rotation: 0.0,
                        layer: "SURVEY_LABELS".into(),
                    }),
                    metadata: Default::default(),
                },
            ],
        };
        assert!(session.apply_wasm_transaction(transaction).is_ok());
        assert_eq!(session.state().map(|state| state.undo.len()).ok(), Some(1));
        assert_eq!(session.doc().map(|drawing| drawing.entity_count()).ok(), Some(3));
        assert!(session.undo().is_ok());
        assert_eq!(session.doc().map(|drawing| drawing.entity_count()).ok(), Some(0));
    }

    #[test]
    fn metadata_is_scoped_and_entity_mutations_are_transactional() {
        let mut session = Session::new();
        let document = session.state().map(|state| state.doc.clone()).unwrap_or_else(|_| Arc::new(Drawing::new_metric()));
        let mut extension = HostState::new(&[Permission::DrawingRead, Permission::DrawingWrite], "org.example.extension", document);
        let transaction = extension.begin_transaction("Persistent extension data".into()).unwrap_or_default();
        let point = cadcraft::extension::geometry::PointRequest {
            position: cadcraft::extension::geometry::Point3 { x: 5.0, y: 2.0, z: 1.0 },
            layer: "SURVEY_POINTS".into(),
        };
        let entity = extension.create_point(transaction, point).unwrap_or_default();
        assert!(extension.set_entity_metadata(transaction, entity, "point-id".into(), "point-101".into()).is_ok());
        assert!(extension.set_drawing_metadata(transaction, "project".into(), "{\"version\":2}".into()).is_ok());
        assert!(extension.commit_transaction(transaction).is_ok());
        let Some(transaction) = extension.committed.take() else { return };
        assert!(session.apply_wasm_transaction(transaction).is_ok());

        let document = session.state().map(|state| state.doc.clone()).unwrap_or_else(|_| Arc::new(Drawing::new_metric()));
        let mut reopened = HostState::new(&[Permission::DrawingRead], "org.example.extension", document);
        assert_eq!(reopened.get_drawing_metadata("project".into()).ok().flatten().as_deref(), Some("{\"version\":2}"));
        let found = reopened.find_entities_by_metadata("point-id".into(), "point-101".into()).unwrap_or_default();
        assert_eq!(found.len(), 1);
        assert_eq!(found.first().map(|item| item.id), Some(entity));

        let mut editor = HostState::new(&[Permission::DrawingRead, Permission::DrawingWrite], "org.example.extension", reopened.document.clone());
        let edit = editor.begin_transaction("Update and delete extension entities".into()).unwrap_or_default();
        let moved = cadcraft::extension::geometry::PointRequest {
            position: cadcraft::extension::geometry::Point3 { x: 8.0, y: 4.0, z: 2.0 },
            layer: "UPDATED_POINTS".into(),
        };
        assert!(editor.update_point(edit, entity, moved).is_ok());
        let temporary = cadcraft::extension::geometry::TextRequest {
            position: cadcraft::extension::geometry::Point3 { x: 8.0, y: 4.0, z: 2.0 },
            value: "temporary".into(),
            height: 0.25,
            rotation_degrees: 0.0,
            layer: "UPDATED_LABELS".into(),
        };
        let temporary = editor.create_text(edit, temporary).unwrap_or_default();
        assert!(editor.delete_entity(edit, temporary).is_ok());
        assert!(editor.commit_transaction(edit).is_ok());
        let Some(edit) = editor.committed.take() else { return };
        assert!(session.apply_wasm_transaction(edit).is_ok());
        let moved =
            session.doc().and_then(|drawing| drawing.entity(cadcraft_doc::Handle(entity)).ok_or(crate::EngineError::Other("point missing".into())));
        assert!(
            matches!(moved.map(|entity| &entity.kind), Ok(cadcraft_doc::EntityKind::Point(point)) if point.p == cadcraft_geom::Vec3::new(8.0, 4.0, 2.0))
        );
        assert!(session.doc().ok().and_then(|drawing| drawing.entity(cadcraft_doc::Handle(temporary))).is_none());

        let other_document = session.state().map(|state| state.doc.clone()).unwrap_or_else(|_| Arc::new(Drawing::new_metric()));
        let other = HostState::new(&[Permission::DrawingRead], "org.other.extension", other_document);
        assert!(other.document.entity(cadcraft_doc::Handle(entity)).is_some());
        assert!(!other.document.extension_data.contains_key("org.other.extension"));
    }
}
