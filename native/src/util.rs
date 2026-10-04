//! Small helpers shared by every module: platform identity, path sanitising, time.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Operating systems OmniDrop runs on. The numeric codes are part of the BLE beacon format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Android,
    Windows,
    Linux,
    Unknown,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "android") {
            Os::Android
        } else if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Unknown
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Os::Android => 1,
            Os::Windows => 2,
            Os::Linux => 3,
            Os::Unknown => 0,
        }
    }

    pub fn from_code(code: u8) -> Os {
        match code {
            1 => Os::Android,
            2 => Os::Windows,
            3 => Os::Linux,
            _ => Os::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::Android => "android",
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Os {
        match s {
            "android" => Os::Android,
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            _ => Os::Unknown,
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Host name used as the default device name on desktop systems.
pub fn hostname() -> String {
    #[cfg(windows)]
    {
        if let Ok(name) = std::env::var("COMPUTERNAME") {
            if !name.trim().is_empty() {
                return name.trim().to_string();
            }
        }
    }
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: buf is valid for writes of buf.len() bytes and gethostname NUL-terminates
        // the result when it fits.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            if !name.is_empty() && name != "localhost" {
                return name;
            }
        }
    }
    format!("OmniDrop-{}", Os::current().as_str())
}

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes one file-name component safe on every supported file system.
pub fn sanitize_component(name: &str) -> Option<String> {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let out = out.trim_start().to_string();
    if out.is_empty() || out == "." || out == ".." {
        return None;
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let mut out = if WINDOWS_RESERVED.contains(&stem.as_str()) {
        format!("_{out}")
    } else {
        out
    };
    // Keep each component below the common 255-byte limit without splitting a character.
    while out.len() > 240 {
        out.pop();
    }
    Some(out)
}

/// Converts a protocol path ("dir/sub/file.txt", always '/'-separated) into a relative
/// path that cannot escape the destination directory.
pub fn sanitize_rel_path(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        out.push(sanitize_component(part)?);
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    debug_assert!(out.components().all(|c| matches!(c, Component::Normal(_))));
    Some(out)
}

/// Returns `path` if free, otherwise "name (1).ext", "name (2).ext", ...
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10_000 {
        let candidate = parent.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem} ({}){ext}", now_ms()))
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("operating system random number generator unavailable");
    out
}

pub fn random_hex(len_bytes: usize) -> String {
    let mut buf = vec![0u8; len_bytes];
    getrandom::fill(&mut buf).expect("operating system random number generator unavailable");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        assert!(sanitize_rel_path("../etc/passwd").is_none());
        assert!(sanitize_rel_path("a/../../b").is_none());
        assert!(sanitize_rel_path("").is_none());
        assert_eq!(
            sanitize_rel_path("/abs/path.txt").unwrap(),
            PathBuf::from("abs").join("path.txt")
        );
    }

    #[test]
    fn cleans_windows_names() {
        assert_eq!(sanitize_component("CON.txt").unwrap(), "_CON.txt");
        assert_eq!(sanitize_component("a:b?.txt. ").unwrap(), "a_b_.txt");
        assert!(sanitize_component("..").is_none());
    }
}
