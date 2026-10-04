//! Wi-Fi Direct on Windows 10/11 through WinRT `Windows.Devices.WiFiDirect`.
//!
//! * The PC advertises itself (`WiFiDirectAdvertisementPublisher`, normal discoverability) and
//!   accepts incoming connection requests (`WiFiDirectConnectionListener`).
//! * `scan` enumerates association endpoints, `connect` opens a `WiFiDirectDevice` and returns the
//!   remote IPv4 address of the new link so the engine can probe it.
//!
//! WinRT calls are blocking-joined on Tokio's blocking pool.

use super::P2pPeer;
use crate::engine::Core;
use crate::events;
use crate::networking::{self, Transport};
use serde_json::json;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::WiFiDirect::{
    WiFiDirectAdvertisementListenStateDiscoverability, WiFiDirectAdvertisementPublisher,
    WiFiDirectConnectionListener, WiFiDirectConnectionRequestedEventArgs, WiFiDirectDevice,
    WiFiDirectDeviceSelectorType,
};
use windows::Foundation::TypedEventHandler;
use windows::core::{HSTRING, Ref};

struct State {
    publisher: Option<WiFiDirectAdvertisementPublisher>,
    listener: Option<WiFiDirectConnectionListener>,
    devices: Vec<WiFiDirectDevice>,
}

static STATE: Mutex<State> = Mutex::new(State {
    publisher: None,
    listener: None,
    devices: Vec::new(),
});

fn werr(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("Wi-Fi Direct: {}", e.message()))
}

fn remote_ipv4(device: &WiFiDirectDevice) -> windows::core::Result<Option<Ipv4Addr>> {
    let pairs = device.GetConnectionEndpointPairs()?;
    for i in 0..pairs.Size()? {
        let host = pairs.GetAt(i)?.RemoteHostName()?.DisplayName()?.to_string();
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Ok(Some(ip));
        }
    }
    Ok(None)
}

fn spawn_probe(handle: &tokio::runtime::Handle, core: Arc<Core>, ip: Ipv4Addr) {
    handle.spawn(async move {
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
    });
}

/// Makes this PC discoverable and accepts incoming Wi-Fi Direct connections.
fn ensure_advertising(
    core: Arc<Core>,
    handle: tokio::runtime::Handle,
) -> windows::core::Result<()> {
    let mut st = STATE.lock().unwrap();
    if st.publisher.is_some() {
        return Ok(());
    }
    let publisher = WiFiDirectAdvertisementPublisher::new()?;
    let adv = publisher.Advertisement()?;
    adv.SetListenStateDiscoverability(WiFiDirectAdvertisementListenStateDiscoverability::Normal)?;
    publisher.Start()?;

    let listener = WiFiDirectConnectionListener::new()?;
    listener.ConnectionRequested(&TypedEventHandler::new(
        move |_sender: Ref<WiFiDirectConnectionListener>,
              args: Ref<WiFiDirectConnectionRequestedEventArgs>| {
            let args = args.ok()?;
            let request = args.GetConnectionRequest()?;
            let id = request.DeviceInformation()?.Id()?;
            let device = WiFiDirectDevice::FromIdAsync(&id)?.join()?;
            let remote = remote_ipv4(&device)?;
            STATE.lock().unwrap().devices.push(device);
            events::emit(
                "wifi_direct_link",
                json!({ "connected": true, "incoming": true }),
            );
            if let Some(ip) = remote {
                spawn_probe(&handle, core.clone(), ip);
            }
            Ok(())
        },
    ))?;
    st.publisher = Some(publisher);
    st.listener = Some(listener);
    Ok(())
}

pub async fn scan(core: Arc<Core>) -> io::Result<Vec<P2pPeer>> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || -> io::Result<Vec<P2pPeer>> {
        ensure_advertising(core, handle).map_err(werr)?;
        let selector =
            WiFiDirectDevice::GetDeviceSelector2(WiFiDirectDeviceSelectorType::AssociationEndpoint)
                .map_err(werr)?;
        let infos = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(werr)?
            .join()
            .map_err(werr)?;
        let mut peers = Vec::new();
        for i in 0..infos.Size().map_err(werr)? {
            let info = infos.GetAt(i).map_err(werr)?;
            let paired = info.Pairing().and_then(|p| p.IsPaired()).unwrap_or(false);
            peers.push(P2pPeer {
                id: info.Id().map_err(werr)?.to_string(),
                name: info.Name().map_err(werr)?.to_string(),
                address: String::new(),
                status: if paired {
                    "paired".into()
                } else {
                    "available".into()
                },
            });
        }
        Ok(peers)
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn connect(core: Arc<Core>, id: String) -> io::Result<Option<Ipv4Addr>> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || -> io::Result<Option<Ipv4Addr>> {
        ensure_advertising(core, handle).map_err(werr)?;
        let device = WiFiDirectDevice::FromIdAsync(&HSTRING::from(id.as_str()))
            .map_err(werr)?
            .join()
            .map_err(werr)?;
        let remote = remote_ipv4(&device).map_err(werr)?;
        STATE.lock().unwrap().devices.push(device);
        Ok(remote)
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn disconnect() -> io::Result<()> {
    let devices: Vec<WiFiDirectDevice> = std::mem::take(&mut STATE.lock().unwrap().devices);
    for d in devices {
        let _ = d.Close();
    }
    Ok(())
}
