//! Engine lifecycle and the JSON command interface used by the Flutter UI.

use crate::config::{Identity, Settings, TrustedPeer};
use crate::diskio::DiskIo;
use crate::events;
use crate::networking::{self, DeviceInfo, Discovery, Registry, Transport};
use crate::platform;
use crate::security::{self, LocalHello, PROTOCOL_VERSION};
use crate::transfer::{self, TransferManager, UserDecision};
use crate::util::Os;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::runtime::Runtime;

/// State shared by every task of the engine.
pub struct Core {
    pub identity: Identity,
    pub data_dir: PathBuf,
    pub settings: RwLock<Settings>,
    pub registry: Registry,
    pub transfers: TransferManager,
    pub disk: DiskIo,
    pub port: AtomicU16,
    /// Android: BLE and Wi-Fi Direct are driven by the Kotlin host through commands.
    pub host_handles_radio: bool,
}

impl Core {
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.identity.id.clone(),
            name: self.settings.read().unwrap().device_name.clone(),
            os: Os::current(),
            port: self.port.load(Ordering::Relaxed),
            version: PROTOCOL_VERSION,
        }
    }

    pub fn local_hello(&self) -> LocalHello {
        LocalHello {
            name: self.settings.read().unwrap().device_name.clone(),
            port: self.port.load(Ordering::Relaxed),
        }
    }

    pub fn save_settings(&self) {
        let s = self.settings.read().unwrap().clone();
        if let Err(e) = s.save(&self.data_dir) {
            events::log("error", format!("cannot save settings: {e}"));
        }
        self.registry
            .set_trusted(s.trusted.iter().map(|t| t.id.clone()));
        events::emit("settings", json!({ "settings": s }));
    }

    pub fn add_trusted(&self, peer: TrustedPeer) {
        {
            let mut s = self.settings.write().unwrap();
            s.trusted.retain(|t| t.id != peer.id);
            s.trusted.push(peer);
        }
        self.save_settings();
    }

    pub fn ble_payload(&self) -> Vec<u8> {
        let info = self.device_info();
        networking::encode_ble_beacon(
            &info.id,
            info.os,
            networking::primary_ipv4(),
            info.port,
            &info.name,
        )
    }
}

pub struct Engine {
    runtime: Runtime,
    core: Arc<Core>,
    discovery: Mutex<Option<Discovery>>,
}

static ENGINE: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

fn engine() -> Option<Arc<Engine>> {
    ENGINE.lock().unwrap().clone()
}

fn ok(v: Value) -> Value {
    let mut v = v;
    if let Value::Object(m) = &mut v {
        m.insert("ok".into(), Value::Bool(true));
        v
    } else {
        json!({ "ok": true, "data": v })
    }
}

fn err(msg: impl std::fmt::Display) -> Value {
    json!({ "ok": false, "error": msg.to_string() })
}

/// Starts the engine. Config: `{data_dir, download_dir, device_name?, host_radio?}`.
pub fn start(config: &Value) -> Value {
    if let Some(e) = engine() {
        return ok(status(&e.core));
    }
    let Some(data_dir) = config["data_dir"].as_str().map(PathBuf::from) else {
        return err("data_dir is required");
    };
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        return err(format!("cannot create {}: {e}", data_dir.display()));
    }
    let identity = match Identity::load_or_create(&data_dir) {
        Ok(i) => i,
        Err(e) => return err(format!("identity: {e}")),
    };
    let first_run = !data_dir.join("settings.json").exists();
    let mut settings = Settings::load(&data_dir);
    if first_run {
        if let Some(name) = config["device_name"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
        {
            settings.device_name = name.trim().to_string();
        }
    }
    if settings.download_dir.is_empty() {
        settings.download_dir = config["download_dir"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    }
    if settings.download_dir.is_empty() {
        settings.download_dir = data_dir.join("Received").to_string_lossy().to_string();
    }
    let settings = settings.normalized();
    let _ = std::fs::create_dir_all(settings.download_path());
    let _ = settings.save(&data_dir);

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .thread_name("omnidrop-rt")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return err(format!("runtime: {e}")),
    };
    let disk = {
        let _guard = runtime.enter();
        DiskIo::new()
    };
    let core = Arc::new(Core {
        registry: Registry::new(identity.id.clone()),
        identity,
        data_dir,
        settings: RwLock::new(settings.clone()),
        transfers: TransferManager::default(),
        disk,
        port: AtomicU16::new(networking::TCP_PORT),
        host_handles_radio: config["host_radio"].as_bool().unwrap_or(false),
    });
    core.registry
        .set_trusted(settings.trusted.iter().map(|t| t.id.clone()));

    let listener = runtime.block_on(async { networking::start_listener() });
    let (listener, port) = match listener {
        Ok(l) => l,
        Err(e) => return err(format!("cannot listen for transfers: {e}")),
    };
    core.port.store(port, Ordering::Relaxed);
    runtime.spawn(networking::accept_loop(core.clone(), listener));
    runtime.spawn(networking::registry_publisher(core.clone()));
    runtime.spawn(transfer::transfer_publisher(core.clone()));
    let discovery = {
        let _guard = runtime.enter();
        Discovery::start(&core)
    };
    let engine = Arc::new(Engine {
        runtime,
        core: core.clone(),
        discovery: Mutex::new(Some(discovery)),
    });
    *ENGINE.lock().unwrap() = Some(engine);
    ok(status(&core))
}

fn status(core: &Arc<Core>) -> Value {
    let info = core.device_info();
    json!({
        "id": info.id,
        "name": info.name,
        "os": info.os,
        "port": info.port,
        "public_key": hex::encode(core.identity.public),
        "settings": *core.settings.read().unwrap(),
        "io_backend": core.disk.backend_name(),
        "aes_hw": security::hardware_aes_available(),
        "wifi_direct_supported": platform::p2p_supported() || core.host_handles_radio,
        "addresses": networking::local_ipv4().iter().map(|a| json!({"interface": a.interface, "ip": a.ip.to_string()})).collect::<Vec<_>>(),
    })
}

pub fn stop() {
    let engine = ENGINE.lock().unwrap().take();
    if let Some(engine) = engine {
        if let Some(d) = engine.discovery.lock().unwrap().take() {
            let _guard = engine.runtime.enter();
            d.stop();
        }
        if let Ok(engine) = Arc::try_unwrap(engine) {
            engine.runtime.shutdown_background();
        }
    }
}

fn restart_discovery(engine: &Engine) {
    let _guard = engine.runtime.enter();
    let mut slot = engine.discovery.lock().unwrap();
    if let Some(d) = slot.take() {
        d.stop();
    }
    *slot = Some(Discovery::start(&engine.core));
}

fn parse_addr(s: &str) -> Option<SocketAddrV4> {
    if let Ok(a) = s.parse::<SocketAddrV4>() {
        return Some(a);
    }
    s.parse::<Ipv4Addr>()
        .ok()
        .map(|ip| SocketAddrV4::new(ip, networking::TCP_PORT))
}

pub fn command(cmd: &Value) -> Value {
    let Some(engine) = engine() else {
        return err("engine not started");
    };
    let core = &engine.core;
    let name = cmd["cmd"].as_str().unwrap_or_default();
    match name {
        "status" => ok(status(core)),
        "get_settings" => ok(json!({ "settings": *core.settings.read().unwrap() })),
        "update_settings" => {
            let current = serde_json::to_value(&*core.settings.read().unwrap()).unwrap_or_default();
            let mut merged = current.clone();
            if let (Value::Object(m), Value::Object(patch)) = (&mut merged, &cmd["settings"]) {
                for (k, v) in patch {
                    if k != "trusted" {
                        m.insert(k.clone(), v.clone());
                    }
                }
            }
            let new: Settings = match serde_json::from_value(merged.clone()) {
                Ok(s) => s,
                Err(e) => return err(format!("invalid settings: {e}")),
            };
            let new = new.normalized();
            if let Err(e) = std::fs::create_dir_all(new.download_path()) {
                return err(format!("download folder: {e}"));
            }
            let discovery_changed = ["device_name", "mdns", "broadcast", "ble", "wifi_direct"]
                .iter()
                .any(|k| current[k] != merged[k]);
            *core.settings.write().unwrap() = new.clone();
            core.save_settings();
            if discovery_changed {
                restart_discovery(&engine);
            }
            ok(json!({ "settings": new }))
        }
        "refresh" => {
            if let Some(d) = engine.discovery.lock().unwrap().as_ref() {
                d.refresh();
            }
            ok(json!({}))
        }
        "send" => {
            let Some(peer) = cmd["peer_id"].as_str() else {
                return err("peer_id required");
            };
            let paths: Vec<PathBuf> = cmd["paths"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default();
            let text = cmd["text"]
                .as_str()
                .map(str::to_string)
                .filter(|t| !t.is_empty());
            let thumbs: HashMap<String, String> = cmd["thumbs"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let _guard = engine.runtime.enter();
            match transfer::start_send(core, peer.to_string(), paths, text, thumbs) {
                Ok(id) => ok(json!({ "transfer_id": id })),
                Err(e) => err(e),
            }
        }
        "respond" => {
            let Some(rid) = cmd["request_id"].as_str() else {
                return err("request_id required");
            };
            let decision = UserDecision {
                accept: cmd["accept"].as_bool().unwrap_or(false),
                trust: cmd["trust"].as_bool().unwrap_or(false),
            };
            if core.transfers.respond(rid, decision) {
                ok(json!({}))
            } else {
                err("request expired")
            }
        }
        "pause" | "resume" => {
            let id = cmd["transfer_id"].as_str().unwrap_or_default();
            if core.transfers.pause(id, name == "pause") {
                ok(json!({}))
            } else {
                err("unknown transfer")
            }
        }
        "cancel" => {
            let id = cmd["transfer_id"].as_str().unwrap_or_default();
            if core.transfers.cancel(id) {
                ok(json!({}))
            } else {
                err("unknown transfer")
            }
        }
        "clear_finished" => {
            core.transfers.remove_finished();
            ok(json!({}))
        }
        "transfers" => ok(json!({
            "transfers": core.transfers.all().iter().map(|t| t.snapshot()).collect::<Vec<_>>()
        })),
        "devices" => ok(json!({ "devices": core.registry.snapshot() })),
        "forget_trusted" => {
            let id = cmd["id"].as_str().unwrap_or_default().to_string();
            core.settings
                .write()
                .unwrap()
                .trusted
                .retain(|t| t.id != id);
            core.save_settings();
            ok(json!({}))
        }
        "probe" => {
            let Some(addr) = cmd["address"].as_str().and_then(parse_addr) else {
                return err("invalid address");
            };
            let transport = if cmd["transport"].as_str() == Some("wifi_direct") {
                Transport::WifiDirect
            } else {
                Transport::Manual
            };
            let core = core.clone();
            engine.runtime.spawn(async move {
                match networking::probe(&core, addr, transport).await {
                    Ok(info) => events::emit(
                        "probe_result",
                        json!({ "ok": true, "address": addr.to_string(), "device": info }),
                    ),
                    Err(e) => events::emit(
                        "probe_result",
                        json!({ "ok": false, "address": addr.to_string(), "error": e.to_string() }),
                    ),
                }
            });
            ok(json!({}))
        }
        "ble_seen" => {
            let data = cmd["data"].as_str().and_then(|d| B64.decode(d).ok());
            let rssi = cmd["rssi"].as_i64().unwrap_or(-100) as i16;
            match data.as_deref().and_then(networking::decode_ble_beacon) {
                Some(b) => {
                    core.registry.observe_ble(&b, rssi);
                    ok(json!({ "id": b.id }))
                }
                None => err("not an OmniDrop beacon"),
            }
        }
        "ble_payload" => ok(json!({
            "payload": B64.encode(core.ble_payload()),
            "company_id": networking::BLE_COMPANY_ID,
        })),
        "wifi_direct_scan" => {
            let core = core.clone();
            engine.runtime.spawn(async move {
                match platform::p2p_scan(&core).await {
                    Ok(peers) => {
                        events::emit("wifi_direct", json!({ "available": true, "peers": peers }))
                    }
                    Err(e) => events::emit(
                        "wifi_direct",
                        json!({ "available": false, "peers": [], "error": e.to_string() }),
                    ),
                }
            });
            ok(json!({}))
        }
        "wifi_direct_connect" => {
            let Some(peer) = cmd["peer"].as_str().map(str::to_string) else {
                return err("peer required");
            };
            let core = core.clone();
            engine.runtime.spawn(async move {
                match platform::p2p_connect(&core, &peer).await {
                    Ok(remote) => {
                        events::emit(
                            "wifi_direct_link",
                            json!({ "connected": true, "peer": peer, "remote_ip": remote.map(|r| r.to_string()) }),
                        );
                        if let Some(ip) = remote {
                            let addr = SocketAddrV4::new(ip, networking::TCP_PORT);
                            match networking::probe(&core, addr, Transport::WifiDirect).await {
                                Ok(info) => events::emit(
                                    "probe_result",
                                    json!({ "ok": true, "address": addr.to_string(), "device": info }),
                                ),
                                Err(e) => events::emit(
                                    "probe_result",
                                    json!({ "ok": false, "address": addr.to_string(), "error": e.to_string() }),
                                ),
                            }
                        }
                    }
                    Err(e) => events::emit(
                        "wifi_direct_link",
                        json!({ "connected": false, "peer": peer, "error": e.to_string() }),
                    ),
                }
            });
            ok(json!({}))
        }
        "wifi_direct_disconnect" => {
            let core = core.clone();
            engine.runtime.spawn(async move {
                let r = platform::p2p_disconnect(&core).await;
                events::emit(
                    "wifi_direct_link",
                    json!({ "connected": false, "error": r.err().map(|e| e.to_string()) }),
                );
            });
            ok(json!({}))
        }
        "local_addresses" => ok(json!({
            "addresses": networking::local_ipv4().iter().map(|a| json!({"interface": a.interface, "ip": a.ip.to_string()})).collect::<Vec<_>>()
        })),
        other => err(format!("unknown command {other}")),
    }
}
