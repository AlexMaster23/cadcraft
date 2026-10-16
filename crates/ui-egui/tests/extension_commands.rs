use cadcraft_engine::{Extension, Session};
use cadcraft_extensions::{Command, CommandUi, Manifest, TableColumn, TableColumnKind};
use cadcraft_ui_egui::{CadApp, Services, control, menus};
use semver::{Version, VersionReq};
use serde_json::json;

fn app() -> CadApp {
    let mut session = Session::new();
    session
        .set_extensions(vec![Extension::new(
            Manifest {
                schema_version: 1,
                id: "org.cadcraft.menu-test".into(),
                name: "Menu Test".into(),
                version: Version::new(1, 0, 0),
                cadcraft: VersionReq::STAR,
                description: "Tests extension command discovery.".into(),
                license: "MIT".into(),
                entrypoint: Some("main.lsp".into()),
                runtime: None,
                permissions: Vec::new(),
                commands: vec![Command {
                    id: "menutest".into(),
                    label: "Extension Menu Test".into(),
                    menu: vec!["Survey".into(), "Run Test".into()],
                    aliases: vec!["ZXQMENU".into()],
                    params: "{}".into(),
                    function: "MENU:TEST".into(),
                    interactive: false,
                    ui: Some(CommandUi::Table {
                        title: "Import points".into(),
                        description: "Type points or open a CSV file.".into(),
                        submit_label: "Insert points".into(),
                        accent: Some("#F47C20".into()),
                        columns: vec![TableColumn {
                            key: "number".into(),
                            label: "Point number".into(),
                            kind: TableColumnKind::Text,
                            required: true,
                            unique: true,
                            aliases: vec!["point".into()],
                        }],
                    }),
                }],
                homepage: None,
            },
            "(defun menu:test (params) 1)".into(),
            true,
        )])
        .unwrap();
    CadApp::new(session, Services::default())
}

#[test]
fn human_invocation_opens_the_native_form_but_json_execution_does_not() {
    let mut app = app();
    app.start("ZXQMENU");
    assert_eq!(app.ui.dialog.as_deref(), Some("extension-table"));

    app.ui.dialog = None;
    assert_eq!(app.run("menutest", json!({ "number": "1" })).ok(), Some(json!(1)));
    assert!(app.ui.dialog.is_none());
}

#[test]
fn extension_commands_reach_menus_and_control_catalog() {
    let app = app();
    let tree = menus::tree(&app);
    let survey = tree.iter().find(|(name, _)| name == "Survey").expect("extension top-level menu");
    assert!(survey.1.iter().any(|entry| matches!(entry, menus::Entry::Item { id, .. } if id == "menutest")));

    let commands = control::all_commands(&app);
    assert!(commands.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("id").and_then(serde_json::Value::as_str) == Some("menutest")
                && item.get("extension").and_then(serde_json::Value::as_str) == Some("org.cadcraft.menu-test")
        })
    }));
}
