//! Event bus from the engine to the host UI. Events are JSON strings pulled by the
//! Dart side through `omnidrop_poll_event` from a background isolate.

use serde_json::{Map, Value};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const QUEUE_CAPACITY: usize = 8192;

struct Bus {
    tx: SyncSender<String>,
    rx: Mutex<Receiver<String>>,
}

fn bus() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, rx) = sync_channel(QUEUE_CAPACITY);
        Bus {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

/// Emits `{"event": name, ...fields}`. `fields` must be a JSON object.
pub fn emit(name: &str, fields: Value) {
    let mut obj = match fields {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut m = Map::new();
            m.insert("data".into(), other);
            m
        }
    };
    obj.insert("event".into(), Value::String(name.to_string()));
    let text = Value::Object(obj).to_string();
    match bus().tx.try_send(text) {
        Ok(()) => {}
        // The UI is not draining events (e.g. app suspended): drop instead of growing forever.
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

pub fn log(level: &str, message: impl Into<String>) {
    emit(
        "log",
        serde_json::json!({ "level": level, "message": message.into() }),
    );
}

/// Blocks up to `timeout` for the next event.
pub fn poll(timeout: Duration) -> Option<String> {
    let rx = bus().rx.lock().ok()?;
    match rx.recv_timeout(timeout) {
        Ok(ev) => Some(ev),
        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
    }
}
