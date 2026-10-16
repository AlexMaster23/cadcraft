use std::collections::{BTreeMap, HashSet};
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::io::{Cursor, Read};

use crate::manifest::package_path;
use crate::{MAX_FILE_BYTES, MAX_FILES, MAX_PACKAGE_BYTES, Manifest};

const MANIFEST: &str = "extension.json";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    /// Regular package files keyed by normalized forward-slash path.
    pub files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug)]
pub enum PackageError {
    Zip(String),
    MissingManifest,
    InvalidManifest(String),
    InvalidPath(String),
    DuplicatePath(String),
    TooManyFiles,
    FileTooLarge(String),
    PackageTooLarge,
    MissingEntrypoint(String),
    EntrypointNotUtf8,
    Io(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::Zip(message) => write!(f, "invalid extension archive: {message}"),
            PackageError::MissingManifest => write!(f, "extension archive has no root extension.json"),
            PackageError::InvalidManifest(message) => write!(f, "invalid extension.json: {message}"),
            PackageError::InvalidPath(path) => write!(f, "unsafe or non-portable package path `{path}`"),
            PackageError::DuplicatePath(path) => write!(f, "duplicate package path `{path}`"),
            PackageError::TooManyFiles => write!(f, "extension contains more than {MAX_FILES} files"),
            PackageError::FileTooLarge(path) => write!(f, "extension file `{path}` exceeds {MAX_FILE_BYTES} bytes"),
            PackageError::PackageTooLarge => write!(f, "extension exceeds {MAX_PACKAGE_BYTES} uncompressed bytes"),
            PackageError::MissingEntrypoint(path) => write!(f, "extension entrypoint `{path}` is missing"),
            PackageError::EntrypointNotUtf8 => write!(f, "extension entrypoint is not UTF-8 text"),
            PackageError::Io(message) => write!(f, "cannot pack extension: {message}"),
        }
    }
}

impl std::error::Error for PackageError {}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self, PackageError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| PackageError::Zip(error.to_string()))?;
        if archive.len() > MAX_FILES {
            return Err(PackageError::TooManyFiles);
        }
        let mut files = BTreeMap::new();
        let mut folded = HashSet::new();
        let mut total = 0_u64;
        for index in 0..archive.len() {
            let file = archive.by_index(index).map_err(|error| PackageError::Zip(error.to_string()))?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_string();
            if !package_path(&name) {
                return Err(PackageError::InvalidPath(name));
            }
            if !folded.insert(name.to_ascii_lowercase()) {
                return Err(PackageError::DuplicatePath(name));
            }
            let declared = file.size();
            let limit = if name == MANIFEST { MAX_MANIFEST_BYTES } else { MAX_FILE_BYTES };
            if declared > limit {
                return Err(PackageError::FileTooLarge(name));
            }
            let capacity = usize::try_from(declared).unwrap_or(0).min(limit as usize);
            let mut data = Vec::with_capacity(capacity);
            file.take(limit + 1).read_to_end(&mut data).map_err(|error| PackageError::Zip(error.to_string()))?;
            if data.len() as u64 > limit {
                return Err(PackageError::FileTooLarge(name));
            }
            total = total.checked_add(data.len() as u64).ok_or(PackageError::PackageTooLarge)?;
            if total > MAX_PACKAGE_BYTES {
                return Err(PackageError::PackageTooLarge);
            }
            files.insert(name, data);
        }
        let manifest_bytes = files.get(MANIFEST).ok_or(PackageError::MissingManifest)?;
        let manifest: Manifest = serde_json::from_slice(manifest_bytes).map_err(|error| PackageError::InvalidManifest(error.to_string()))?;
        manifest.validate().map_err(|error| PackageError::InvalidManifest(error.to_string()))?;
        let entrypoint = files.get(&manifest.entrypoint).ok_or_else(|| PackageError::MissingEntrypoint(manifest.entrypoint.clone()))?;
        std::str::from_utf8(entrypoint).map_err(|_| PackageError::EntrypointNotUtf8)?;
        Ok(Package { manifest, files })
    }

    pub fn entrypoint(&self) -> &str {
        self.files.get(&self.manifest.entrypoint).and_then(|bytes| std::str::from_utf8(bytes).ok()).unwrap_or("")
    }

    /// Build a deterministic package from every regular file below `root`. Symlinks and special
    /// files are rejected so the bundle cannot accidentally capture data outside the project.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pack_directory(root: &std::path::Path) -> Result<Vec<u8>, PackageError> {
        fn collect(
            root: &std::path::Path,
            dir: &std::path::Path,
            files: &mut BTreeMap<String, Vec<u8>>,
            folded: &mut HashSet<String>,
            total: &mut u64,
        ) -> Result<(), PackageError> {
            let entries = std::fs::read_dir(dir).map_err(|error| PackageError::Io(format!("{}: {error}", dir.display())))?;
            for entry in entries {
                let entry = entry.map_err(|error| PackageError::Io(format!("{}: {error}", dir.display())))?;
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))?;
                if metadata.file_type().is_symlink() {
                    return Err(PackageError::Io(format!("{} is a symlink", path.display())));
                }
                if metadata.is_dir() {
                    collect(root, &path, files, folded, total)?;
                    continue;
                }
                if !metadata.is_file() {
                    return Err(PackageError::Io(format!("{} is not a regular file", path.display())));
                }
                if files.len() >= MAX_FILES {
                    return Err(PackageError::TooManyFiles);
                }
                let relative = path.strip_prefix(root).map_err(|error| PackageError::Io(error.to_string()))?;
                let name = relative
                    .components()
                    .map(|part| part.as_os_str().to_str().ok_or_else(|| PackageError::InvalidPath(relative.display().to_string())))
                    .collect::<Result<Vec<_>, _>>()?
                    .join("/");
                if !package_path(&name) {
                    return Err(PackageError::InvalidPath(name));
                }
                if !folded.insert(name.to_ascii_lowercase()) {
                    return Err(PackageError::DuplicatePath(name));
                }
                let limit = if name == MANIFEST { MAX_MANIFEST_BYTES } else { MAX_FILE_BYTES };
                if metadata.len() > limit {
                    return Err(PackageError::FileTooLarge(name));
                }
                *total = total.checked_add(metadata.len()).ok_or(PackageError::PackageTooLarge)?;
                if *total > MAX_PACKAGE_BYTES {
                    return Err(PackageError::PackageTooLarge);
                }
                let bytes = std::fs::read(&path).map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))?;
                files.insert(name, bytes);
            }
            Ok(())
        }

        let metadata = std::fs::metadata(root).map_err(|error| PackageError::Io(format!("{}: {error}", root.display())))?;
        if !metadata.is_dir() {
            return Err(PackageError::Io(format!("{} is not a directory", root.display())));
        }
        let mut files = BTreeMap::new();
        let mut folded = HashSet::new();
        let mut total = 0;
        collect(root, root, &mut files, &mut folded, &mut total)?;
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            writer.start_file(name, options).map_err(|error| PackageError::Zip(error.to_string()))?;
            writer.write_all(&bytes).map_err(|error| PackageError::Zip(error.to_string()))?;
        }
        let bytes = writer.finish().map_err(|error| PackageError::Zip(error.to_string()))?.into_inner();
        Self::read(&bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn manifest(entrypoint: &str) -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "id": "org.cadcraft.example",
            "name": "Example",
            "version": "1.0.0",
            "cadcraft": ">=0.7, <1",
            "description": "A package parser test.",
            "license": "MIT",
            "entrypoint": entrypoint,
            "commands": [{
                "id": "examplehello",
                "label": "Hello",
                "function": "EXAMPLE:HELLO"
            }]
        })
        .to_string()
    }

    fn bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_a_valid_bundle() {
        let json = manifest("main.lsp");
        let bytes = bundle(&[("extension.json", json.as_bytes()), ("main.lsp", b"(defun example:hello (p) 42)")]);
        let package = Package::read(&bytes).unwrap();
        assert_eq!(package.manifest.id, "org.cadcraft.example");
        assert!(package.entrypoint().contains("example:hello"));
    }

    #[test]
    fn rejects_traversal_and_case_collisions() {
        let json = manifest("main.lsp");
        let traversal = bundle(&[("extension.json", json.as_bytes()), ("../main.lsp", b"x")]);
        assert!(matches!(Package::read(&traversal), Err(PackageError::InvalidPath(_))));
        let collision = bundle(&[("extension.json", json.as_bytes()), ("main.lsp", b"x"), ("MAIN.LSP", b"y")]);
        assert!(matches!(Package::read(&collision), Err(PackageError::DuplicatePath(_))));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn packs_and_revalidates_a_source_directory() {
        let root = std::env::temp_dir().join(format!("cadcraft-extension-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("extension.json"), manifest("main.lsp")).unwrap();
        std::fs::write(root.join("main.lsp"), "(defun example:hello (p) (load \"lib/helper\"))").unwrap();
        std::fs::write(root.join("lib/helper.lsp"), "42").unwrap();
        let bytes = Package::pack_directory(&root).unwrap();
        let package = Package::read(&bytes).unwrap();
        assert!(package.files.contains_key("lib/helper.lsp"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
