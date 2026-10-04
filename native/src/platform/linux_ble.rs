//! BLE beacons on Linux through BlueZ (D-Bus): an `org.bluez.LEAdvertisement1` object is
//! registered with `LEAdvertisingManager1`, and LE discovery reports `ManufacturerData`
//! changes through `PropertiesChanged` / `InterfacesAdded` signals.

use super::BleHandle;
use crate::engine::Core;
use crate::events;
use crate::networking::{self, BLE_COMPANY_ID};
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream, Proxy};

const BLUEZ: &str = "org.bluez";
const ADV_PATH: &str = "/org/omnidrop/advertisement0";

struct Advertisement {
    payload: Vec<u8>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    fn release(&self) {}

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "broadcast".to_string()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, Value<'static>> {
        HashMap::from([(BLE_COMPANY_ID, Value::from(self.payload.clone()))])
    }
}

fn zerr(e: zbus::Error) -> io::Error {
    io::Error::other(format!("BlueZ: {e}"))
}

pub fn start(core: Arc<Core>) -> io::Result<BleHandle> {
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        if let Err(e) = run(core, stop_rx).await {
            events::emit(
                "ble_state",
                json!({ "available": false, "active": false, "error": e.to_string() }),
            );
        }
    });
    Ok(BleHandle::new(move || {
        let _ = stop_tx.send(());
        // Give the task a moment to unregister cleanly, then make sure it is gone.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            task.abort();
        });
    }))
}

async fn find_adapter(conn: &Connection) -> io::Result<OwnedObjectPath> {
    let om = Proxy::new(conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")
        .await
        .map_err(zerr)?;
    let objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> =
        om.call("GetManagedObjects", &()).await.map_err(zerr)?;
    let mut adapters: Vec<OwnedObjectPath> = objects
        .into_iter()
        .filter(|(_, ifaces)| ifaces.contains_key("org.bluez.Adapter1"))
        .map(|(p, _)| p)
        .collect();
    adapters.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Bluetooth adapter"))
}

async fn register_adv(conn: &Connection, adapter: &str, payload: Vec<u8>) -> io::Result<()> {
    let server = conn.object_server();
    let _ = server.remove::<Advertisement, _>(ADV_PATH).await;
    server
        .at(ADV_PATH, Advertisement { payload })
        .await
        .map_err(zerr)?;
    let mgr = Proxy::new(conn, BLUEZ, adapter, "org.bluez.LEAdvertisingManager1")
        .await
        .map_err(zerr)?;
    let opts: HashMap<&str, Value> = HashMap::new();
    let path = ObjectPath::try_from(ADV_PATH).map_err(|e| zerr(e.into()))?;
    mgr.call_method("RegisterAdvertisement", &(path, opts))
        .await
        .map_err(zerr)?;
    Ok(())
}

async fn unregister_adv(conn: &Connection, adapter: &str) {
    if let Ok(mgr) = Proxy::new(conn, BLUEZ, adapter, "org.bluez.LEAdvertisingManager1").await {
        if let Ok(path) = ObjectPath::try_from(ADV_PATH) {
            let _ = mgr.call_method("UnregisterAdvertisement", &(path,)).await;
        }
    }
    let _ = conn
        .object_server()
        .remove::<Advertisement, _>(ADV_PATH)
        .await;
}

async fn check_device(core: &Arc<Core>, conn: &Connection, path: &str) {
    let Ok(props) = Proxy::new(conn, BLUEZ, path, "org.freedesktop.DBus.Properties").await else {
        return;
    };
    let md: Result<OwnedValue, _> = props
        .call("Get", &("org.bluez.Device1", "ManufacturerData"))
        .await;
    let Ok(md) = md else { return };
    let Ok(map) = HashMap::<u16, OwnedValue>::try_from(md) else {
        return;
    };
    let Some(value) = map.get(&BLE_COMPANY_ID) else {
        return;
    };
    let Ok(bytes) =
        Vec::<u8>::try_from(value.try_clone().unwrap_or_else(|_| OwnedValue::from(0u8)))
    else {
        return;
    };
    let rssi: i16 = match props
        .call::<_, _, OwnedValue>("Get", &("org.bluez.Device1", "RSSI"))
        .await
    {
        Ok(v) => i16::try_from(v).unwrap_or(-100),
        Err(_) => -100,
    };
    if let Some(beacon) = networking::decode_ble_beacon(&bytes) {
        core.registry.observe_ble(&beacon, rssi);
    }
}

async fn run(core: Arc<Core>, mut stop: oneshot::Receiver<()>) -> io::Result<()> {
    let conn = Connection::system().await.map_err(zerr)?;
    let adapter_path = find_adapter(&conn).await?;
    let adapter = adapter_path.as_str().to_string();
    let adapter_proxy = Proxy::new(&conn, BLUEZ, adapter.as_str(), "org.bluez.Adapter1")
        .await
        .map_err(zerr)?;
    let powered: bool = adapter_proxy.get_property("Powered").await.map_err(zerr)?;
    if !powered {
        return Err(io::Error::other("Bluetooth is turned off"));
    }

    let mut payload = core.ble_payload();
    let advertising = match register_adv(&conn, &adapter, payload.clone()).await {
        Ok(()) => true,
        Err(e) => {
            events::log("warn", format!("BLE advertising unavailable: {e}"));
            false
        }
    };

    let mut filter: HashMap<&str, Value> = HashMap::new();
    filter.insert("Transport", Value::from("le"));
    filter.insert("DuplicateData", Value::from(true));
    let _ = adapter_proxy
        .call_method("SetDiscoveryFilter", &(filter,))
        .await;
    adapter_proxy
        .call_method("StartDiscovery", &())
        .await
        .map_err(zerr)?;
    events::emit(
        "ble_state",
        json!({ "available": true, "active": true, "advertising": advertising }),
    );

    let props_rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ)
        .map_err(zerr)?
        .interface("org.freedesktop.DBus.Properties")
        .map_err(zerr)?
        .member("PropertiesChanged")
        .map_err(zerr)?
        .build();
    let added_rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ)
        .map_err(zerr)?
        .interface("org.freedesktop.DBus.ObjectManager")
        .map_err(zerr)?
        .member("InterfacesAdded")
        .map_err(zerr)?
        .build();
    let mut props_stream = MessageStream::for_match_rule(props_rule, &conn, Some(256))
        .await
        .map_err(zerr)?;
    let mut added_stream = MessageStream::for_match_rule(added_rule, &conn, Some(64))
        .await
        .map_err(zerr)?;
    let mut refresh = tokio::time::interval(Duration::from_secs(20));

    loop {
        tokio::select! {
            _ = &mut stop => break,
            Some(Ok(msg)) = props_stream.next() => {
                let header = msg.header();
                let Some(path) = header.path() else { continue };
                if !path.as_str().starts_with(adapter.as_str()) || path.as_str() == adapter {
                    continue;
                }
                let iface: Result<(String, HashMap<String, OwnedValue>, Vec<String>), _> = msg.body().deserialize();
                if let Ok((iface, _, _)) = iface {
                    if iface == "org.bluez.Device1" {
                        check_device(&core, &conn, path.as_str()).await;
                    }
                }
            }
            Some(Ok(msg)) = added_stream.next() => {
                let body: Result<(OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>), _> = msg.body().deserialize();
                if let Ok((path, ifaces)) = body {
                    if ifaces.contains_key("org.bluez.Device1") {
                        check_device(&core, &conn, path.as_str()).await;
                    }
                }
            }
            _ = refresh.tick() => {
                // Discovery sessions may be stopped by other clients; restart quietly. Also
                // refresh the advertisement when the LAN address changed.
                let _ = adapter_proxy.call_method("StartDiscovery", &()).await;
                let fresh = core.ble_payload();
                if advertising && fresh != payload {
                    payload = fresh;
                    unregister_adv(&conn, &adapter).await;
                    let _ = register_adv(&conn, &adapter, payload.clone()).await;
                }
            }
        }
    }
    let _ = adapter_proxy.call_method("StopDiscovery", &()).await;
    if advertising {
        unregister_adv(&conn, &adapter).await;
    }
    Ok(())
}
