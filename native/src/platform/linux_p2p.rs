//! Wi-Fi Direct on Linux through NetworkManager's `Device.WifiP2P` D-Bus API (NM >= 1.16 with a
//! P2P-capable wpa_supplicant). The polkit rule shipped in `linux/packaging` lets local active
//! users call these methods without a password.

use super::P2pPeer;
use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEVICE_TYPE_WIFI_P2P: u32 = 30;
const ACTIVE_STATE_ACTIVATED: u32 = 2;
const ACTIVE_STATE_DEACTIVATING: u32 = 3;
const ACTIVE_STATE_DEACTIVATED: u32 = 4;

static ACTIVE: Mutex<Option<OwnedObjectPath>> = Mutex::new(None);

fn current_user() -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return user;
        }
    }
    // SAFETY: getpwuid returns a pointer into static storage or NULL; we copy the name at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned();
        }
    }
    "root".to_string()
}

fn zerr(e: zbus::Error) -> io::Error {
    io::Error::other(format!("NetworkManager: {e}"))
}

async fn get_prop(
    conn: &Connection,
    path: &str,
    iface: &str,
    name: &str,
) -> io::Result<OwnedValue> {
    let props = Proxy::new(conn, NM, path, "org.freedesktop.DBus.Properties")
        .await
        .map_err(zerr)?;
    props.call("Get", &(iface, name)).await.map_err(zerr)
}

fn conv<T>(v: OwnedValue) -> io::Result<T>
where
    T: TryFrom<OwnedValue>,
    <T as TryFrom<OwnedValue>>::Error: std::fmt::Display,
{
    T::try_from(v).map_err(|e| io::Error::other(format!("unexpected D-Bus value: {e}")))
}

async fn p2p_device(conn: &Connection) -> io::Result<OwnedObjectPath> {
    let nm = Proxy::new(conn, NM, NM_PATH, NM).await.map_err(zerr)?;
    let devices: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await.map_err(zerr)?;
    for d in devices {
        let ty: u32 = conv(
            get_prop(
                conn,
                d.as_str(),
                "org.freedesktop.NetworkManager.Device",
                "DeviceType",
            )
            .await?,
        )?;
        if ty == DEVICE_TYPE_WIFI_P2P {
            return Ok(d);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no Wi-Fi Direct capable adapter (NetworkManager >= 1.16 with wpa_supplicant P2P support is required)",
    ))
}

async fn peers(conn: &Connection, device: &str) -> io::Result<Vec<(OwnedObjectPath, P2pPeer)>> {
    let list: Vec<OwnedObjectPath> = conv(
        get_prop(
            conn,
            device,
            "org.freedesktop.NetworkManager.Device.WifiP2P",
            "Peers",
        )
        .await?,
    )?;
    let iface = "org.freedesktop.NetworkManager.WifiP2PPeer";
    let mut out = Vec::new();
    for p in list {
        let name: String =
            conv(get_prop(conn, p.as_str(), iface, "Name").await?).unwrap_or_default();
        let hw: String = conv(get_prop(conn, p.as_str(), iface, "HwAddress").await?)?;
        let strength: u8 = conv(get_prop(conn, p.as_str(), iface, "Strength").await?).unwrap_or(0);
        out.push((
            p,
            P2pPeer {
                id: hw.clone(),
                name: if name.is_empty() { hw.clone() } else { name },
                address: hw,
                status: format!("{strength}%"),
            },
        ));
    }
    Ok(out)
}

pub async fn scan() -> io::Result<Vec<P2pPeer>> {
    let conn = Connection::system().await.map_err(zerr)?;
    let device = p2p_device(&conn).await?;
    let p2p = Proxy::new(
        &conn,
        NM,
        device.as_str(),
        "org.freedesktop.NetworkManager.Device.WifiP2P",
    )
    .await
    .map_err(zerr)?;
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("timeout", Value::from(20i32));
    p2p.call_method("StartFind", &(opts,)).await.map_err(zerr)?;
    tokio::time::sleep(Duration::from_secs(8)).await;
    Ok(peers(&conn, device.as_str())
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect())
}

pub async fn connect(peer_mac: &str) -> io::Result<Option<Ipv4Addr>> {
    let conn = Connection::system().await.map_err(zerr)?;
    let device = p2p_device(&conn).await?;
    let found = peers(&conn, device.as_str())
        .await?
        .into_iter()
        .find(|(_, p)| p.address.eq_ignore_ascii_case(peer_mac));
    let Some((peer_path, peer)) = found else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "peer is no longer visible, scan again",
        ));
    };

    let mut connection: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    connection.insert(
        "connection",
        HashMap::from([
            ("id", Value::from("OmniDrop Wi-Fi Direct")),
            ("type", Value::from("wifi-p2p")),
            ("autoconnect", Value::from(false)),
            // A per-user connection only needs `settings.modify.own`, which NetworkManager grants
            // to active local sessions (see the polkit rule in linux/packaging/polkit).
            ("permissions", Value::from(vec![format!("user:{}", current_user())])),
        ]),
    );
    connection.insert(
        "wifi-p2p",
        HashMap::from([("peer", Value::from(peer.address.clone()))]),
    );
    connection.insert("ipv4", HashMap::from([("method", Value::from("auto"))]));
    connection.insert("ipv6", HashMap::from([("method", Value::from("ignore"))]));

    let nm = Proxy::new(&conn, NM, NM_PATH, NM).await.map_err(zerr)?;
    let (_settings, active): (OwnedObjectPath, OwnedObjectPath) = nm
        .call(
            "AddAndActivateConnection",
            &(connection, device.as_ref(), peer_path.as_ref()),
        )
        .await
        .map_err(zerr)?;
    *ACTIVE.lock().unwrap() = Some(active.clone());

    let active_iface = "org.freedesktop.NetworkManager.Connection.Active";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let state: u32 = conv(get_prop(&conn, active.as_str(), active_iface, "State").await?)?;
        match state {
            ACTIVE_STATE_ACTIVATED => break,
            ACTIVE_STATE_DEACTIVATING | ACTIVE_STATE_DEACTIVATED => {
                return Err(io::Error::other(
                    "the peer refused or the group negotiation failed",
                ));
            }
            _ => {}
        }
        if tokio::time::Instant::now() > deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Wi-Fi Direct group not formed in time",
            ));
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    let ip4: OwnedObjectPath =
        conv(get_prop(&conn, active.as_str(), active_iface, "Ip4Config").await?)?;
    let ip_iface = "org.freedesktop.NetworkManager.IP4Config";
    let gateway: String =
        conv(get_prop(&conn, ip4.as_str(), ip_iface, "Gateway").await?).unwrap_or_default();
    if let Ok(gw) = gateway.parse::<Ipv4Addr>() {
        return Ok(Some(gw));
    }
    // We are the group owner (no gateway): the peer will find us through the broadcast beacons on
    // the new interface, so nothing to probe.
    Ok(None)
}

pub async fn disconnect() -> io::Result<()> {
    let Some(active) = ACTIVE.lock().unwrap().take() else {
        return Ok(());
    };
    let conn = Connection::system().await.map_err(zerr)?;
    let nm = Proxy::new(&conn, NM, NM_PATH, NM).await.map_err(zerr)?;
    nm.call_method("DeactivateConnection", &(active.as_ref(),))
        .await
        .map_err(zerr)?;
    Ok(())
}
