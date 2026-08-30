//! Path-safety validation for share IDs.

/// Path-safety validator for share IDs (re-exported by transport).
pub fn valid_share_id(id: &str) -> bool {
    // ':' — Windows drive-relative ids like "C:evil" escape share_dir via PathBuf::join.
    !id.is_empty()
        && !id.starts_with('.')
        && !id.contains(['/', '\\', ':'])
        && !id.contains("..")
        && !std::path::Path::new(id).is_absolute()
}
