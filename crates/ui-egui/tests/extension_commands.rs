use cadcraft_engine::{Extension, Session};
use cadcraft_extensions::{
    Command, CommandUi, Manifest, TableColumn, TableColumnKind, TableSelect, TableSelectChoice, Toolset, ToolsetIcon, ToolsetPosition, ToolsetSize,
    ToolsetTab, ToolsetTool,
};
use cadcraft_ui_egui::{CadApp, LoadedTextTable, Services, control, menus};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
                        selects: vec![TableSelect {
                            key: "convention".into(),
                            label: "Coordinate convention".into(),
                            default: "cad-xy".into(),
                            choices: vec![
                                TableSelectChoice { value: "cad-xy".into(), label: "CAD / GIS X-Y".into() },
                                TableSelectChoice { value: "survey-xy".into(), label: "Survey / TopoLT X-Y".into() },
                            ],
                        }],
                        columns: vec![
                            TableColumn {
                                key: "number".into(),
                                label: "Point number".into(),
                                kind: TableColumnKind::Text,
                                required: true,
                                unique: true,
                                aliases: vec!["point".into()],
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
                            TableColumn {
                                key: "elevation".into(),
                                label: "Elevation (Z)".into(),
                                kind: TableColumnKind::Number,
                                required: true,
                                unique: false,
                                aliases: vec!["z".into()],
                            },
                            TableColumn {
                                key: "code".into(),
                                label: "Code / description".into(),
                                kind: TableColumnKind::Text,
                                required: false,
                                unique: false,
                                aliases: vec!["description".into()],
                            },
                        ],
                    }),
                }],
                toolsets: vec![Toolset {
                    tab: ToolsetTab::Drafting,
                    group: "Survey tools".into(),
                    position: ToolsetPosition::Top,
                    accent: Some("#F47A20".into()),
                    tools: vec![ToolsetTool { command: "menutest".into(), icon: ToolsetIcon::Import, size: ToolsetSize::Large }],
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

#[test]
fn extension_commands_reach_tool_sets_and_keep_their_human_ui() {
    let mut harness = Harness::builder().with_size(vec2(360.0, 1600.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            cadcraft_ui_egui::palettes::toolsets(app, ui);
        },
        app(),
    );
    harness.run_steps(5);

    assert!(harness.query_by_label("Survey tools").is_some());
    harness.get_by_label("Extension Menu Test").click();
    harness.run_steps(3);
    assert_eq!(harness.state().ui.dialog.as_deref(), Some("extension-table"));
}

#[test]
fn extension_table_renders_its_declared_selector() {
    let mut app = app();
    app.services.pick_extension_table = Some(Box::new(|| {
        Ok(Some(LoadedTextTable { name: "points.csv".into(), text: "Point,X,Y,Z,Description\n201,500000.25,300000.5,100.125,STN\n".into() }))
    }));
    app.start("ZXQMENU");
    let directory = std::env::var_os("CADCRAFT_EXTENSION_TABLE_SHOTS").map(std::path::PathBuf::from);
    let builder = Harness::builder().with_size(vec2(980.0, 620.0));
    let builder = if directory.is_some() { builder.wgpu() } else { builder };
    let mut harness = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            let mut open = true;
            cadcraft_ui_egui::extension_table::dialog(app, ui.ctx(), &mut open);
        },
        app,
    );
    harness.run_steps(3);

    assert!(harness.query_by_label("Coordinate convention").is_some());
    harness.get_by_label("Open CSV / text...").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("1 point ready").is_some());
    if let Some(directory) = directory {
        std::fs::create_dir_all(&directory).unwrap();
        harness.render().unwrap().save(directory.join("extension-table-selector.png")).unwrap();
    }
}
