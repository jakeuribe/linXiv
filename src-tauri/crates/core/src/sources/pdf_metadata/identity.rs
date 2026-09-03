//! Crypto identity — the deterministic `local:<sha256[:16]>` source id.

use crate::models::local_source_id;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Local source id:  local:<sha256(bytes)[:16 hex]>
// ---------------------------------------------------------------------------

pub(crate) fn pdf_source_id(bytes: &[u8]) -> String {
    let h = sha256(bytes);
    let mut hex = String::new();
    for byte in &h[..8] {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    local_source_id(&hex)
}

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // sha256("") = e3b0c442...; pdf id keeps the first 16 hex chars.
        assert_eq!(&pdf_source_id(b"")[..22], "local:e3b0c44298fc1c14");
        assert_eq!(
            pdf_source_id(b"stable pdf bytes"),
            // sha256("stable pdf bytes")[:16]
            format!("local:{}", &hex(&sha256(b"stable pdf bytes"))[..16])
        );
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
