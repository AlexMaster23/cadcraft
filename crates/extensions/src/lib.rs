//! CADCraft extension manifests, `.cadcraft-extension` packages and installed-extension storage.
//!
//! Packages are ordinary ZIP files with an `extension.json` manifest at their root. This crate
//! validates every name and size before exposing bytes to a host. It never executes extension
//! code. The engine is responsible for loading an enabled extension's entry-point through the
//! constrained AutoLISP runtime.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod manifest;
mod package;
#[cfg(not(target_arch = "wasm32"))]
mod store;

pub use manifest::{Command, Manifest, Permission, ValidationError};
pub use package::{Package, PackageError};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{Installed, Store, StoreError};

/// Current extension-package schema understood by this release.
pub const SCHEMA_VERSION: u32 = 1;

/// Maximum number of regular files in one package.
pub const MAX_FILES: usize = 128;
/// Maximum uncompressed size of one file.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum total uncompressed package size.
pub const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024;
