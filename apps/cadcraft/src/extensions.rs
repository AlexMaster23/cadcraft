//! Desktop extension store and the portable UI service bridge.

use std::path::{Path, PathBuf};

use cadcraft_extensions::{Installed, Permission, Store};
use cadcraft_ui_egui::{ExtensionSnapshot, InstalledExtension, Services};
use semver::Version;

fn version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn snapshot(store: &Store, cadcraft: &Version) -> Result<ExtensionSnapshot, String> {
    let installed = store.list(cadcraft).map_err(|error| error.to_string())?;
    Ok(ExtensionSnapshot {
        installed: installed.iter().map(info).collect(),
        runtime: installed
            .into_iter()
            .map(|extension| cadcraft_engine::Extension::new(extension.manifest, extension.entrypoint, extension.enabled && extension.compatible))
            .collect(),
    })
}

fn info(extension: &Installed) -> InstalledExtension {
    InstalledExtension {
        id: extension.manifest.id.clone(),
        name: extension.manifest.name.clone(),
        version: extension.manifest.version.to_string(),
        description: extension.manifest.description.clone(),
        license: extension.manifest.license.clone(),
        permissions: extension
            .manifest
            .permissions
            .iter()
            .map(|permission| match permission {
                Permission::ReadFiles => "read files",
                Permission::WriteFiles => "write files",
            })
            .map(str::to_string)
            .collect(),
        commands: extension.manifest.commands.len(),
        enabled: extension.enabled,
        compatible: extension.compatible,
    }
}

pub fn install_services(services: &mut Services, root: PathBuf) {
    let store = Store::new(root);
    let cadcraft = version();

    let list_store = store.clone();
    let list_version = cadcraft.clone();
    services.extension_snapshot = Some(Box::new(move || snapshot(&list_store, &list_version)));

    let install_store = store.clone();
    let install_version = cadcraft.clone();
    services.install_extension = Some(Box::new(move |path| {
        install_store.install_file(Path::new(path), &install_version).map_err(|error| error.to_string())?;
        snapshot(&install_store, &install_version)
    }));

    let enable_store = store.clone();
    let enable_version = cadcraft.clone();
    services.enable_extension = Some(Box::new(move |id, enabled| {
        enable_store.set_enabled(id, enabled).map_err(|error| error.to_string())?;
        snapshot(&enable_store, &enable_version)
    }));

    let uninstall_store = store;
    let uninstall_version = cadcraft;
    services.uninstall_extension = Some(Box::new(move |id| {
        uninstall_store.uninstall(id).map_err(|error| error.to_string())?;
        snapshot(&uninstall_store, &uninstall_version)
    }));

    services.pick_extension = Some(Box::new(|| {
        rfd::FileDialog::new().add_filter("CADCraft extension", &["cadcraft-extension"]).pick_file().map(|path| path.to_string_lossy().to_string())
    }));
}
