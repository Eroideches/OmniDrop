//! C ABI consumed by Dart FFI (`lib/core/native_bridge.dart`).
//!
//! Every function exchanges UTF-8 JSON strings. Strings returned by the library must be released
//! with [`omnidrop_free_string`]. Panics never cross the FFI boundary.

use crate::{engine, events};
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

fn to_c(v: Value) -> *mut c_char {
    CString::new(v.to_string())
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"nul byte\"}").unwrap())
        .into_raw()
}

unsafe fn parse(ptr: *const c_char) -> Result<Value, Value> {
    if ptr.is_null() {
        return Err(json!({"ok": false, "error": "null argument"}));
    }
    // SAFETY: the caller passes a valid NUL-terminated string (see the function contracts).
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    serde_json::from_str(&text).map_err(|e| json!({"ok": false, "error": format!("bad json: {e}")}))
}

fn guarded(f: impl FnOnce() -> Value) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => to_c(v),
        Err(_) => to_c(json!({"ok": false, "error": "internal error (panic)"})),
    }
}

/// Starts the engine.
///
/// # Safety
/// `config` must be a valid NUL-terminated UTF-8 JSON string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omnidrop_start(config: *const c_char) -> *mut c_char {
    // SAFETY: forwarded caller contract.
    match unsafe { parse(config) } {
        Ok(cfg) => guarded(|| engine::start(&cfg)),
        Err(e) => to_c(e),
    }
}

/// Executes a JSON command (`{"cmd": "...", ...}`) and returns the JSON result.
///
/// # Safety
/// `command` must be a valid NUL-terminated UTF-8 JSON string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omnidrop_command(command: *const c_char) -> *mut c_char {
    // SAFETY: forwarded caller contract.
    match unsafe { parse(command) } {
        Ok(cmd) => guarded(|| engine::command(&cmd)),
        Err(e) => to_c(e),
    }
}

/// Waits up to `timeout_ms` for the next engine event; returns NULL on timeout.
#[unsafe(no_mangle)]
pub extern "C" fn omnidrop_poll_event(timeout_ms: u32) -> *mut c_char {
    let result = catch_unwind(|| events::poll(Duration::from_millis(timeout_ms as u64)));
    match result {
        Ok(Some(ev)) => CString::new(ev)
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut()),
        _ => std::ptr::null_mut(),
    }
}

/// Releases a string returned by this library.
///
/// # Safety
/// `ptr` must come from this library and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omnidrop_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        // SAFETY: the pointer was produced by CString::into_raw in this library.
        drop(unsafe { CString::from_raw(ptr) });
    }
}

/// Stops discovery and the runtime.
#[unsafe(no_mangle)]
pub extern "C" fn omnidrop_stop() {
    let _ = catch_unwind(engine::stop);
}
