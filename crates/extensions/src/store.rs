use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use semver::Version;

use crate::{MAX_FILE_BYTES, Manifest, Package, PackageError};

const MANIFEST: &str = "extension.json";
const DISABLED: &str = ".cadcraft-disabled";
const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub enabled: bool,
    pub compatible: bool,
    pub entrypoint: String,
    pub directory: PathBuf,
}

#[derive(Debug)]
pub enum StoreError {
    Io { path: PathBuf, message: String },
    Package(PackageError),
    InvalidId(String),
    NotInstalled(String),
    Incompatible { id: String, requirement: String, actual: String },
    InvalidInstalled { path: PathBuf, message: String },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, message } => write!(f, "{}: {message}", path.display()),
            StoreError::Package(error) => write!(f, "{error}"),
            StoreError::InvalidId(id) => write!(f, "invalid extension id `{id}`"),
            StoreError::NotInstalled(id) => write!(f, "extension `{id}` is not installed"),
            StoreError::Incompatible { id, requirement, actual } => {
                write!(f, "extension `{id}` requires CADCraft {requirement}; this is {actual}")
            }
            StoreError::InvalidInstalled { path, message } => write!(f, "invalid installed extension {}: {message}", path.display()),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<PackageError> for StoreError {
    fn from(value: PackageError) -> Self {
        StoreError::Package(value)
    }
}

fn io(path: &Path, error: impl fmt::Display) -> StoreError {
    StoreError::Io { path: path.to_path_buf(), message: error.to_string() }
}

fn safe_id(id: &str) -> bool {
    id.len() <= 128
        && id.split('.').count() >= 2
        && id.split('.').all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn private_file(path: &Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| io(path, error))
}

fn unique_sibling(root: &Path, id: &str, role: &str) -> Result<PathBuf, StoreError> {
    for _ in 0..32 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(".{id}.{role}-{}-{sequence}", std::process::id()));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(io(root, "could not allocate a temporary extension directory"))
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn list(&self, cadcraft_version: &Version) -> Result<Vec<Installed>, StoreError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io(&self.root, error)),
        };
        let mut installed = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io(&self.root, error))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !entry.file_type().map_err(|error| io(&path, error))?.is_dir() || !safe_id(&name) {
                continue;
            }
            installed.push(read_installed(&path, &name, cadcraft_version)?);
        }
        installed
            .sort_by(|a, b| a.manifest.name.to_ascii_lowercase().cmp(&b.manifest.name.to_ascii_lowercase()).then(a.manifest.id.cmp(&b.manifest.id)));
        Ok(installed)
    }

    pub fn install_file(&self, path: &Path, cadcraft_version: &Version) -> Result<Installed, StoreError> {
        let file = File::open(path).map_err(|error| io(path, error))?;
        let mut bytes = Vec::new();
        file.take(MAX_ARCHIVE_BYTES + 1).read_to_end(&mut bytes).map_err(|error| io(path, error))?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(io(path, format!("extension archive exceeds {MAX_ARCHIVE_BYTES} bytes")));
        }
        self.install_bytes(&bytes, cadcraft_version)
    }

    pub fn install_bytes(&self, bytes: &[u8], cadcraft_version: &Version) -> Result<Installed, StoreError> {
        let package = Package::read(bytes)?;
        let id = &package.manifest.id;
        if !package.manifest.supports(cadcraft_version) {
            return Err(StoreError::Incompatible {
                id: id.clone(),
                requirement: package.manifest.cadcraft.to_string(),
                actual: cadcraft_version.to_string(),
            });
        }
        fs::create_dir_all(&self.root).map_err(|error| io(&self.root, error))?;
        let target = self.root.join(id);
        let stage = unique_sibling(&self.root, id, "install")?;
        fs::create_dir(&stage).map_err(|error| io(&stage, error))?;
        let write_result = (|| {
            for (name, data) in &package.files {
                let path = stage.join(name);
                let parent = path.parent().ok_or_else(|| io(&path, "file has no parent"))?;
                fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
                let mut file = private_file(&path)?;
                file.write_all(data).map_err(|error| io(&path, error))?;
                file.sync_all().map_err(|error| io(&path, error))?;
            }
            // Updating an explicitly disabled extension must not silently enable it.
            if target.join(DISABLED).is_file() {
                let marker = stage.join(DISABLED);
                private_file(&marker)?.sync_all().map_err(|error| io(&marker, error))?;
            }
            Ok::<(), StoreError>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
        let backup = if target.exists() {
            let backup = unique_sibling(&self.root, id, "backup")?;
            fs::rename(&target, &backup).map_err(|error| io(&target, error))?;
            Some(backup)
        } else {
            None
        };
        if let Err(error) = fs::rename(&stage, &target) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, &target);
            }
            let _ = fs::remove_dir_all(&stage);
            return Err(io(&target, error));
        }
        if let Some(backup) = backup {
            let _ = fs::remove_dir_all(backup);
        }
        read_installed(&target, id, cadcraft_version)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), StoreError> {
        let directory = self.directory(id)?;
        let marker = directory.join(DISABLED);
        if enabled {
            match fs::remove_file(&marker) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(io(&marker, error)),
            }
        } else if marker.exists() {
            Ok(())
        } else {
            private_file(&marker)?.sync_all().map_err(|error| io(&marker, error))
        }
    }

    pub fn uninstall(&self, id: &str) -> Result<(), StoreError> {
        let directory = self.directory(id)?;
        let removed = unique_sibling(&self.root, id, "removed")?;
        fs::rename(&directory, &removed).map_err(|error| io(&directory, error))?;
        fs::remove_dir_all(&removed).map_err(|error| io(&removed, error))
    }

    fn directory(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !safe_id(id) {
            return Err(StoreError::InvalidId(id.to_string()));
        }
        let directory = self.root.join(id);
        if !directory.is_dir() {
            return Err(StoreError::NotInstalled(id.to_string()));
        }
        Ok(directory)
    }
}

fn bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::InvalidInstalled { path: path.to_path_buf(), message: "expected a regular file".into() });
    }
    if metadata.len() > limit {
        return Err(StoreError::InvalidInstalled { path: path.to_path_buf(), message: format!("file exceeds {limit} bytes") });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path).map_err(|error| io(path, error))?.take(limit + 1).read_to_end(&mut bytes).map_err(|error| io(path, error))?;
    if bytes.len() as u64 > limit {
        return Err(StoreError::InvalidInstalled { path: path.to_path_buf(), message: format!("file exceeds {limit} bytes") });
    }
    Ok(bytes)
}

fn read_installed(path: &Path, directory_name: &str, cadcraft_version: &Version) -> Result<Installed, StoreError> {
    let manifest_path = path.join(MANIFEST);
    let bytes = bounded_file(&manifest_path, 64 * 1024)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::InvalidInstalled { path: manifest_path.clone(), message: format!("invalid JSON: {error}") })?;
    manifest.validate().map_err(|error| StoreError::InvalidInstalled { path: manifest_path, message: error.to_string() })?;
    if manifest.id != directory_name {
        return Err(StoreError::InvalidInstalled { path: path.to_path_buf(), message: "manifest id does not match its directory".into() });
    }
    let entry_path = path.join(&manifest.entrypoint);
    let entrypoint = String::from_utf8(bounded_file(&entry_path, MAX_FILE_BYTES)?)
        .map_err(|_| StoreError::InvalidInstalled { path: entry_path, message: "entrypoint is not UTF-8".into() })?;
    Ok(Installed {
        compatible: manifest.supports(cadcraft_version),
        enabled: !path.join(DISABLED).exists(),
        manifest,
        entrypoint,
        directory: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    fn temp_dir() -> PathBuf {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-extension-store-{}-{sequence}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn bundle(version: &str, body: &str) -> Vec<u8> {
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "id": "org.cadcraft.example",
            "name": "Example",
            "version": version,
            "cadcraft": ">=0.7, <1",
            "description": "An installed extension test.",
            "license": "MIT",
            "entrypoint": "main.lsp",
            "commands": [{"id":"examplehello", "label":"Hello", "function":"EXAMPLE:HELLO"}]
        });
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("extension.json", options).unwrap();
        writer.write_all(manifest.to_string().as_bytes()).unwrap();
        writer.start_file("main.lsp", options).unwrap();
        writer.write_all(body.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn install_update_disable_and_uninstall() {
        let root = temp_dir();
        let store = Store::new(root.join("extensions"));
        let cadcraft = Version::new(0, 7, 0);
        let installed = store.install_bytes(&bundle("1.0.0", "(defun example:hello (p) 1)"), &cadcraft).unwrap();
        assert!(installed.enabled && installed.compatible);
        store.set_enabled("org.cadcraft.example", false).unwrap();
        let installed = store.install_bytes(&bundle("1.1.0", "(defun example:hello (p) 2)"), &cadcraft).unwrap();
        assert!(!installed.enabled, "an update preserves the disabled state");
        assert_eq!(installed.manifest.version, Version::new(1, 1, 0));
        store.set_enabled("org.cadcraft.example", true).unwrap();
        assert!(store.list(&cadcraft).unwrap()[0].enabled);
        store.uninstall("org.cadcraft.example").unwrap();
        assert!(store.list(&cadcraft).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_incompatible_packages_without_writing() {
        let root = temp_dir();
        let store = Store::new(root.join("extensions"));
        let error = store.install_bytes(&bundle("1.0.0", "x"), &Version::new(1, 0, 0)).unwrap_err();
        assert!(matches!(error, StoreError::Incompatible { .. }));
        assert!(store.list(&Version::new(1, 0, 0)).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ids_cannot_escape_the_store() {
        let root = temp_dir();
        let store = Store::new(root.join("extensions"));
        for id in ["../outside", "C:/outside", "single"] {
            assert!(matches!(store.set_enabled(id, false), Err(StoreError::InvalidId(_)) | Err(StoreError::NotInstalled(_))), "{id}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
