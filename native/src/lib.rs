//! OmniDrop core engine.
//!
//! * [`networking`]: discovery (mDNS, UDP broadcast, BLE, Wi-Fi Direct) and tuned sockets
//! * [`security`]: Noise_XX handshake, AES-256-GCM / ChaCha20-Poly1305 records, SAS
//! * [`transfer`]: parallel, resumable, block-verified file transfer
//! * [`ffi`]: C ABI for the Flutter UI

// Style-only lints: nested `if let` blocks read better than let-chains in this code base.
#![allow(
    clippy::collapsible_if,
    clippy::unnecessary_sort_by,
    clippy::type_complexity,
    clippy::needless_range_loop
)]

pub mod config;
pub mod diskio;
pub mod engine;
pub mod events;
pub mod ffi;
pub mod networking;
pub mod platform;
pub mod security;
pub mod transfer;
pub mod util;
