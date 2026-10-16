use cadcraft_engine::{Extension, Session};
use cadcraft_extensions::{Command, Manifest};
use cadcraft_ui_egui::{CadApp, Services, control, menus};
use semver::{Version, VersionReq};

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
                entrypoint: "main.lsp".into(),
                permissions: Vec::new(),
                commands: vec![Command {
                    id: "menutest".into(),
                    label: "Extension Menu Test".into(),
                    menu: vec!["Survey".into(), "Run Test".into()],
                    aliases: vec!["ZXQMENU".into()],
                    params: "{}".into(),
                    function: "MENU:TEST".into(),
                    interactive: false,
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
