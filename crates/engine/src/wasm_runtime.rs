//! Native WebAssembly component host for schema 2 extensions.
//!
//! Guest drawing calls are staged in host memory. CADCraft applies them only after the guest
//! returns successfully with a committed transaction, so traps, fuel exhaustion and protocol
//! errors leave the drawing byte-for-byte unchanged.

use std::sync::Arc;

use cadcraft_doc::{EntityKind, HAlign, Line, Point, Text, VAlign};
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

#[derive(Debug)]
struct Transaction {
    id: u64,
    label: String,
    entities: Vec<PendingEntity>,
}

struct HostState {
    drawing_write: bool,
    transaction: Option<Transaction>,
    committed: Option<Transaction>,
    next_entity_id: u64,
    limits: StoreLimits,
}

impl HostState {
    fn new(permissions: &[Permission]) -> Self {
        Self {
            drawing_write: permissions.contains(&Permission::DrawingWrite),
            transaction: None,
            committed: None,
            next_entity_id: 1,
            limits: StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).table_elements(MAX_OPERATIONS).instances(4).tables(8).memories(4).build(),
        }
    }

    fn require_write(&self) -> std::result::Result<(), cadcraft::extension::drawing::HostError> {
        if self.drawing_write {
            Ok(())
        } else {
            Err(cadcraft::extension::drawing::HostError::PermissionDenied("drawing-write permission is required".into()))
        }
    }

    fn push_entity(&mut self, transaction: u64, entity: PendingEntity) -> std::result::Result<u64, cadcraft::extension::drawing::HostError> {
        let Some(active) = self.transaction.as_mut().filter(|active| active.id == transaction) else {
            return Err(cadcraft::extension::drawing::HostError::InvalidInput("transaction is not active".into()));
        };
        if active.entities.len() >= MAX_OPERATIONS {
            return Err(cadcraft::extension::drawing::HostError::DrawingFailure(format!("transaction exceeds {MAX_OPERATIONS} drawing operations")));
        }
        let entity_id = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        active.entities.push(entity);
        Ok(entity_id)
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
        self.transaction = Some(Transaction { id: 1, label: label.to_owned(), entities: Vec::new() });
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

fn execute_component(bytes: &[u8], command: &str, parameters: &Value, permissions: &[Permission]) -> EngineResult<WasmOutcome> {
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
    let mut store = Store::new(&engine, HostState::new(permissions));
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
    pub(crate) fn wasm_execute_extension(&mut self, bytes: &[u8], command: &str, params: &Value, permissions: &[Permission]) -> EngineResult<Value> {
        if self.state()?.block_edit.is_some() {
            return Err(EngineError::Other("WebAssembly extension commands are not available inside the Block Editor yet".into()));
        }
        let outcome = execute_component(bytes, command, params, permissions)?;
        self.apply_wasm_transaction(outcome.transaction)?;
        Ok(outcome.value)
    }

    fn apply_wasm_transaction(&mut self, transaction: Transaction) -> EngineResult<()> {
        let state = self.state()?;
        let before = state.doc.clone();
        let selection = state.selection.clone();
        let uid = state.uid;
        let space = self.space();
        let base_common = self.current_common();
        let mut drawing = (*before).clone();
        let text_style = drawing.header.str("TEXTSTYLE", "Standard");
        for pending in transaction.entities {
            let mut common = base_common.clone();
            let kind = match pending {
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
            drawing.add(&space, common, kind)?;
        }
        let state = self.state_mut()?;
        state.doc = Arc::new(drawing);
        state.push_undo(Snapshot { label: transaction.label, doc: before, selection });
        state.revision = state.revision.saturating_add(1);
        if state.uid != uid {
            return Err(EngineError::Other("active drawing changed while applying an extension transaction".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_extensions::Permission;

    use super::{HostState, MAX_OPERATIONS, PendingEntity, PendingLine, PendingPoint, PendingText, Transaction, cadcraft};
    use crate::Session;
    use crate::wasm_runtime::cadcraft::extension::drawing::Host as _;

    #[test]
    fn host_requires_permission_and_a_matching_transaction() {
        let mut denied = HostState::new(&[]);
        let denied_result = denied.begin_transaction("Test".into());
        assert!(matches!(denied_result, Err(cadcraft::extension::drawing::HostError::PermissionDenied(_))));

        let mut host = HostState::new(&[Permission::DrawingWrite]);
        assert!(matches!(host.begin_transaction("Test".into()), Ok(1)));
        assert!(matches!(host.commit_transaction(2), Err(cadcraft::extension::drawing::HostError::InvalidInput(_))));
        assert!(matches!(host.rollback_transaction(1), Ok(())));
    }

    #[test]
    fn host_rejects_non_finite_lines_and_bounds_operations() {
        let mut host = HostState::new(&[Permission::DrawingWrite]);
        assert!(matches!(host.begin_transaction("Test".into()), Ok(1)));
        let invalid = cadcraft::extension::geometry::LineRequest {
            start: cadcraft::extension::geometry::Point3 { x: f64::NAN, y: 0.0, z: 0.0 },
            end: cadcraft::extension::geometry::Point3 { x: 1.0, y: 0.0, z: 0.0 },
            layer: "0".into(),
        };
        assert!(matches!(host.create_line(1, invalid), Err(cadcraft::extension::drawing::HostError::InvalidInput(_))));
        if let Some(transaction) = host.transaction.as_mut() {
            transaction.entities = Vec::with_capacity(MAX_OPERATIONS);
            transaction.entities.resize_with(MAX_OPERATIONS, || {
                PendingEntity::Line(super::PendingLine {
                    start: cadcraft_geom::Vec3::ZERO,
                    end: cadcraft_geom::Vec3::new(1.0, 0.0, 0.0),
                    layer: "0".into(),
                })
            });
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
            entities: vec![
                PendingEntity::Line(PendingLine {
                    start: cadcraft_geom::Vec3::ZERO,
                    end: cadcraft_geom::Vec3::new(10.0, 0.0, 0.0),
                    layer: "SURVEY_REFERENCE".into(),
                }),
                PendingEntity::Point(PendingPoint { position: cadcraft_geom::Vec3::new(5.0, 2.0, 1.0), layer: "SURVEY_POINTS".into() }),
                PendingEntity::Text(PendingText {
                    position: cadcraft_geom::Vec3::new(5.25, 2.25, 1.0),
                    value: "101".into(),
                    height: 0.25,
                    rotation: 0.0,
                    layer: "SURVEY_LABELS".into(),
                }),
            ],
        };
        assert!(session.apply_wasm_transaction(transaction).is_ok());
        assert_eq!(session.state().map(|state| state.undo.len()).ok(), Some(1));
        assert_eq!(session.doc().map(|drawing| drawing.entity_count()).ok(), Some(3));
        assert!(session.undo().is_ok());
        assert_eq!(session.doc().map(|drawing| drawing.entity_count()).ok(), Some(0));
    }
}
