//! Persistent identity (Curve25519 static key) and user settings.

use crate::security;
use crate::util::{self, Os};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Long-term device identity. The device id is the first 8 bytes of SHA-256(public key),
/// so a peer that completes the Noise handshake provably owns the id it advertises.
#[derive(Clone)]
pub struct Identity {
    pub private: [u8; 32],
    pub public: [u8; 32],
    pub id: String,
}

impl Identity {
    pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
        let path = dir.join("identity.key");
        if let Ok(bytes) = fs::read(&path) {
            if bytes.len() == 64 {
                let mut private = [0u8; 32];
                let mut public = [0u8; 32];
                private.copy_from_slice(&bytes[..32]);
                public.copy_from_slice(&bytes[32..]);
                return Ok(Identity::from_keys(private, public));
            }
        }
        let (private, public) = security::generate_static_keypair()
            .map_err(|e| io::Error::other(format!("key generation failed: {e}")))?;
        let mut blob = Vec::with_capacity(64);
        blob.extend_from_slice(&private);
        blob.extend_from_slice(&public);
        write_private_file(&path, &blob)?;
        Ok(Identity::from_keys(private, public))
    }

    fn from_keys(private: [u8; 32], public: [u8; 32]) -> Identity {
        Identity {
            id: device_id_for(&public),
            private,
            public,
        }
    }
}

pub fn device_id_for(public: &[u8]) -> String {
    let digest = Sha256::digest(public);
    hex::encode(&digest[..8])
}

fn write_private_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?
        };
        #[cfg(not(unix))]
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        io::Write::write_all(&mut file, data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub id: String,
    pub name: String,
    pub os: Os,
    /// Hex-encoded Curve25519 static public key verified through SAS.
    pub public_key: String,
    pub added: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub device_name: String,
    pub download_dir: String,
    pub mdns: bool,
    pub broadcast: bool,
    pub ble: bool,
    pub wifi_direct: bool,
    /// Parallel TCP data streams per transfer (1..=8).
    pub streams: u16,
    pub auto_accept_trusted: bool,
    pub trusted: Vec<TrustedPeer>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            device_name: util::hostname(),
            download_dir: String::new(),
            mdns: true,
            broadcast: true,
            ble: true,
            wifi_direct: true,
            streams: 4,
            auto_accept_trusted: false,
            trusted: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = settings_path(dir);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let path = settings_path(dir);
        let tmp = path.with_extension("tmp");
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &path)
    }

    pub fn normalized(mut self) -> Settings {
        self.streams = self.streams.clamp(1, 8);
        self.device_name = self.device_name.trim().chars().take(64).collect();
        if self.device_name.is_empty() {
            self.device_name = util::hostname();
        }
        self
    }

    pub fn trusted_key(&self, id: &str) -> Option<&TrustedPeer> {
        self.trusted.iter().find(|t| t.id == id)
    }

    pub fn download_path(&self) -> PathBuf {
        PathBuf::from(&self.download_dir)
    }
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}
