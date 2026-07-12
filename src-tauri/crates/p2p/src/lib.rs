//! linXiv P2P sharing: iroh transport + (flagged) keyhive capabilities + beelay sync.
//!
//! App code uses only this root interface; `auth`/`sync` internals never leak.

pub mod sync;

pub use sync::{ALPN, AccessCheckFn, DeviceIdentity, ShareNode, ShareTicket};

#[cfg(feature = "auth-keyhive")]
pub mod auth;

#[cfg(feature = "auth-keyhive")]
pub use auth::{AuthIdentity, DecryptError, DeviceBinding, MemberId, ProjectAuth, Role};

#[cfg(feature = "sync-beelay")]
pub mod beelay;

#[cfg(feature = "sync-beelay")]
pub use beelay::{
    BEELAY_ALPN, BeelayNode, ProjectInvite, SyncOutcome, bind_stack, bind_stack_local,
};

// vendor-edit: the crate's fallible surface returns `n0_error::AnyError`;
// re-exported so callers can name it (e.g. to downcast `beelay::BlobError`).
pub use n0_error::AnyError;
