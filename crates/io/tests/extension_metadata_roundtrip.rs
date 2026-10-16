//! Extension-owned drawing and entity metadata survives a normal DXF save/reopen cycle while
//! remaining grouped by extension id.

use cadcraft_doc::{Common, Drawing, EntityKind, Point, Space};
use cadcraft_geom::Vec3;
use cadcraft_io::{read_dxf, write_dxf};

#[test]
fn namespaced_extension_metadata_roundtrips() {
    let mut drawing = Drawing::new_metric();
    drawing.extension_data.entry("org.example.survey".into()).or_default().insert("project".into(), r#"{"schemaVersion":2}"#.into());

    let mut common = Common::default();
    common.extension_data.entry("org.example.survey".into()).or_default().insert("point-id".into(), "3df89c21-61af-5ca5-b096-b20c8bb91492".into());
    common.extension_data.entry("org.example.other".into()).or_default().insert("role".into(), "control".into());
    let handle = drawing.add(&Space::Model, common, EntityKind::Point(Point { p: Vec3::new(10.0, 20.0, 3.0), angle: 0.0 })).unwrap_or_default();

    let text = write_dxf(&drawing);
    assert!(text.contains("CADCRAFT_EXTENSIONS"));
    let reopened = read_dxf(text.as_bytes()).unwrap_or_else(|_| Drawing::new_metric());

    assert_eq!(
        reopened.extension_data.get("org.example.survey").and_then(|data| data.get("project")).map(String::as_str),
        Some(r#"{"schemaVersion":2}"#)
    );
    let metadata = reopened.entity(handle).map(|entity| &entity.common.extension_data);
    assert_eq!(
        metadata.and_then(|data| data.get("org.example.survey")).and_then(|data| data.get("point-id")).map(String::as_str),
        Some("3df89c21-61af-5ca5-b096-b20c8bb91492")
    );
    assert_eq!(metadata.and_then(|data| data.get("org.example.other")).and_then(|data| data.get("role")).map(String::as_str), Some("control"));
}
