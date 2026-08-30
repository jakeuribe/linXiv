//! The `.lxproj` zip read boundary: open, manifest parse, DB-free preview.

use std::io::{Read, Seek};
use std::path::Path;

use super::dto::{ImportPreview, Manifest};
use super::import::preview_from_manifest;
use crate::error::{CoreError, Result};

/// Read + parse `manifest.json` from an open archive. Mirrors Python `_read_manifest`:
/// a missing entry is a "not a valid .lxproj file" error.
pub(super) fn read_manifest<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    label: &str,
) -> Result<Manifest> {
    let mut entry = archive.by_name("manifest.json").map_err(|_| {
        CoreError::BadRequest(format!(
            "{label} is not a valid .lxproj file: manifest.json missing"
        ))
    })?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| CoreError::Internal(e.to_string()))?;
    serde_json::from_slice(&buf).map_err(|e| CoreError::Internal(e.to_string()))
}

/// Open a `.lxproj` archive at `zip_path`.
pub(super) fn open_archive(zip_path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let file = std::fs::File::open(zip_path).map_err(|e| CoreError::Internal(e.to_string()))?;
    zip::ZipArchive::new(file).map_err(|e| CoreError::Internal(e.to_string()))
}

/// Parse a `.lxproj` archive and return a preview without touching the DB.
pub fn preview_import(zip_path: &Path) -> Result<ImportPreview> {
    let mut archive = open_archive(zip_path)?;
    let manifest = read_manifest(&mut archive, &zip_path.display().to_string())?;
    Ok(preview_from_manifest(&manifest))
}
