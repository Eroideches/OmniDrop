//! Platform radios used for off-grid discovery and links.
//!
//! | Platform | BLE beacons                         | Wi-Fi Direct                                  |
//! |----------|-------------------------------------|-----------------------------------------------|
//! | Linux    | BlueZ over D-Bus (`linux_ble`)      | NetworkManager Wi-Fi P2P over D-Bus           |
//! | Windows  | WinRT Bluetooth LE advertisement    | WinRT `Windows.Devices.WiFiDirect`            |
//! | Android  | Kotlin host (`OffGridManager.kt`)   | Kotlin host (`WifiP2pManager`)                |
//!
//! On Android the host reports beacons through the `ble_seen` command and Wi-Fi Direct group
//! addresses through `probe`, so the Rust side only parses and merges them.

use crate::engine::Core;
use serde::Serialize;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux_ble;
#[cfg(target_os = "linux")]
mod linux_p2p;
#[cfg(windows)]
mod windows_ble;
#[cfg(windows)]
mod windows_p2p;

/// Stops the platform BLE advertiser/scanner when dropped through [`BleHandle::stop`].
pub struct BleHandle {
    stop: Box<dyn FnOnce() + Send>,
}

impl BleHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> BleHandle {
        BleHandle {
            stop: Box::new(stop),
        }
    }

    pub fn stop(self) {
        (self.stop)();
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct P2pPeer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub status: String,
}

pub fn start_ble(core: Arc<Core>) -> io::Result<BleHandle> {
    #[cfg(target_os = "linux")]
    {
        linux_ble::start(core)
    }
    #[cfg(windows)]
    {
        windows_ble::start(core)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = core;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "BLE is driven by the Android host on this platform",
        ))
    }
}

pub fn p2p_supported() -> bool {
    cfg!(any(target_os = "linux", windows))
}

pub async fn p2p_scan(core: &Arc<Core>) -> io::Result<Vec<P2pPeer>> {
    #[cfg(target_os = "linux")]
    {
        let _ = core;
        linux_p2p::scan().await
    }
    #[cfg(windows)]
    {
        windows_p2p::scan(core.clone()).await
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = core;
        Err(unsupported())
    }
}

pub async fn p2p_connect(core: &Arc<Core>, peer: &str) -> io::Result<Option<Ipv4Addr>> {
    #[cfg(target_os = "linux")]
    {
        let _ = core;
        linux_p2p::connect(peer).await
    }
    #[cfg(windows)]
    {
        windows_p2p::connect(core.clone(), peer.to_string()).await
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (core, peer);
        Err(unsupported())
    }
}

pub async fn p2p_disconnect(core: &Arc<Core>) -> io::Result<()> {
    let _ = core;
    #[cfg(target_os = "linux")]
    {
        linux_p2p::disconnect().await
    }
    #[cfg(windows)]
    {
        windows_p2p::disconnect().await
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(unsupported())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Wi-Fi Direct is driven by the Android host on this platform",
    )
}
