//! p2p node config resolved from the OS keychain + on-disk user settings:
//! the at-rest DEK and the relay override (Settings → Sharing). Shared by
//! `main.rs` (initial bind at startup) and `route::share`'s relay-reconnect
//! command (rebinding without an app restart), so both paths agree on what
//! "the configured relay" means.

use linxiv_core::config;

/// 32-byte DEK for the p2p key store (write-enforcement spec §8): the at-rest
/// encryption key for `device.key` / `auth.key` / keyhive `state.bin`.
///
/// Resolution order (spec §8): fetch-or-create in the OS keychain (service
/// "linXiv", account "p2p-dek") is primary; keychain unavailable
/// (headless/CI) → Argon2id-derive from `LINXIV_P2P_PASSPHRASE` if set; else
/// `None` with one logged warning, keeping today's plaintext key store. The
/// passphrase is inert whenever the keychain works.
pub fn p2p_dek() -> Option<[u8; 32]> {
    let unavailable = |e: &dyn std::fmt::Display| match passphrase_dek() {
        Some(dek) => Some(dek),
        None => {
            eprintln!("warning: OS keychain unavailable, p2p key store stays plaintext: {e}");
            None
        }
    };
    let entry = match keyring::Entry::new("linXiv", "p2p-dek") {
        Ok(entry) => entry,
        Err(e) => return unavailable(&e),
    };
    let parse = |hex: &str| -> Option<[u8; 32]> {
        let bytes: Option<Vec<u8>> = (hex.len() == 64 && hex.is_ascii())
            .then(|| {
                (0..64)
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect()
            })
            .flatten();
        bytes.and_then(|b| <[u8; 32]>::try_from(b).ok())
    };
    let malformed = || {
        // Never clobber an entry we can't read — regenerating would orphan
        // any files sealed under the old DEK.
        eprintln!("warning: keychain p2p-dek entry is malformed, p2p key store stays plaintext");
        None
    };
    match entry.get_password() {
        Ok(hex) => match parse(&hex) {
            Some(dek) => Some(dek),
            None => malformed(),
        },
        Err(keyring::Error::NoEntry) => {
            let mut dek = [0u8; 32];
            if let Err(e) = getrandom::fill(&mut dek) {
                return unavailable(&e);
            }
            let hex: String = dek.iter().map(|b| format!("{b:02x}")).collect();
            if let Err(e) = entry.set_password(&hex) {
                // A DEK that isn't persisted must not encrypt anything; the
                // passphrase fallback is fine here — it's re-derivable.
                return unavailable(&e);
            }
            // First-run mint race: two instances launched together can both
            // see NoEntry and set different DEKs — the keychain keeps the
            // last write. Seal files only under the READ-BACK value, never
            // the local mint, or state.bin can end up sealed under a key the
            // keychain no longer holds (unrecoverable from the next launch).
            match entry.get_password() {
                Ok(stored) => match parse(&stored) {
                    Some(dek) => Some(dek),
                    None => malformed(),
                },
                Err(e) => unavailable(&e),
            }
        }
        Err(e) => unavailable(&e),
    }
}

/// Keychain-unavailable fallback (spec §8): derive the DEK from
/// `LINXIV_P2P_PASSPHRASE` via Argon2id, default params. Fixed app-level
/// salt, documented as such: the passphrase is per-deployment; the salt only
/// domain-separates this derivation (there is no per-install salt to store).
fn passphrase_dek() -> Option<[u8; 32]> {
    let pass = std::env::var("LINXIV_P2P_PASSPHRASE").ok()?;
    let mut dek = [0u8; 32];
    match argon2::Argon2::default().hash_password_into(
        pass.as_bytes(),
        b"linxiv-p2p-dek-v1",
        &mut dek,
    ) {
        Ok(()) => Some(dek),
        Err(e) => {
            eprintln!("warning: LINXIV_P2P_PASSPHRASE derivation failed, p2p key store stays plaintext: {e}");
            None
        }
    }
}

/// Resolved relay config from Settings → Sharing (`p2p_relay_url` /
/// `p2p_relay_auth_token` / `p2p_relay_only`; TODO.md "Expose Node selection
/// via gui"). `RequireCustomButMissing` must never resolve to n0's public
/// relay — the caller refuses to bind the node instead.
pub enum RelaySetting {
    /// No custom relay configured (or "only" isn't set): n0 public defaults.
    Default,
    /// Bind with this relay only.
    Custom(linxiv_share::CustomRelay),
    /// "Only use this relay" is on but no valid relay is configured.
    RequireCustomButMissing,
}

/// Reads the relay settings straight off the on-disk file.
pub fn relay_setting() -> RelaySetting {
    let Ok(settings) = config::UserSettings::load() else {
        return RelaySetting::Default;
    };
    let relay_only = settings
        .get("p2p_relay_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let missing_or_default = || {
        if relay_only {
            RelaySetting::RequireCustomButMissing
        } else {
            RelaySetting::Default
        }
    };
    let Some(url) = settings
        .get("p2p_relay_url")
        .and_then(|v| v.as_str())
        .filter(|u| !u.is_empty())
    else {
        return missing_or_default();
    };
    let token = settings
        .get("p2p_relay_auth_token")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    match linxiv_share::CustomRelay::parse(url, token) {
        Ok(relay) => RelaySetting::Custom(relay),
        Err(e) => {
            eprintln!("warning: invalid p2p_relay_url {url:?}: {e}");
            missing_or_default()
        }
    }
}
