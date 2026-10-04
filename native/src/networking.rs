//! Networking: device discovery and socket management.
//!
//! Discovery sources feeding one [`Registry`]:
//! * mDNS / DNS-SD (`_omnidrop._tcp.local.`)
//! * UDP broadcast beacons on every IPv4 interface (including Wi-Fi Direct groups)
//! * BLE manufacturer-data beacons (desktop: BlueZ / WinRT in `platform`, Android: Kotlin host)
//! * Wi-Fi Direct links (desktop: NetworkManager / WinRT, Android: WifiP2pManager) followed by
//!   a TCP probe of the peer address
//! * manual probes and incoming probes
//!
//! Sockets: Tokio drives the TCP sockets with epoll on Linux/Android and with IOCP (AFD) on
//! Windows. Every transfer socket gets `TCP_NODELAY`, 4 MiB `SO_SNDBUF`/`SO_RCVBUF` (set before
//! `connect`/`listen` so the window scale is negotiated accordingly) and TCP keep-alive.

use crate::engine::Core;
use crate::events;
use crate::security::PROTOCOL_VERSION;
use crate::transfer;
use crate::util::Os;
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;
use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub const TCP_PORT: u16 = 44850;
pub const UDP_PORT: u16 = 44851;
pub const MDNS_SERVICE: &str = "_omnidrop._tcp.local.";
pub const SOCKET_BUFFER: usize = 4 * 1024 * 1024;
pub const MAGIC_LEN: usize = 8;
pub const MAGIC_CTRL: &[u8; MAGIC_LEN] = b"ODCTRL01";
pub const MAGIC_DATA: &[u8; MAGIC_LEN] = b"ODDATA01";
pub const MAGIC_PROBE: &[u8; MAGIC_LEN] = b"ODPROBE1";
/// Bluetooth SIG "reserved for internal use / testing" company identifier.
pub const BLE_COMPANY_ID: u16 = 0xFFFF;
const BEACON_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub os: Os,
    pub port: u16,
    pub version: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Mdns,
    Broadcast,
    Ble,
    WifiDirect,
    Manual,
    Incoming,
}

impl Transport {
    fn ttl(self) -> Duration {
        match self {
            Transport::Broadcast => Duration::from_secs(10),
            Transport::Mdns => Duration::from_secs(75),
            Transport::Ble => Duration::from_secs(30),
            Transport::WifiDirect | Transport::Manual | Transport::Incoming => {
                Duration::from_secs(60)
            }
        }
    }
}

struct AddrEntry {
    addr: SocketAddrV4,
    transport: Transport,
    last_seen: Instant,
}

struct Entry {
    info: DeviceInfo,
    addrs: Vec<AddrEntry>,
    ble: Option<(i16, Instant)>,
    /// True once the full name is known (BLE beacons only carry a 7-byte prefix).
    name_complete: bool,
    last_seen: Instant,
}

/// What the UI sees for one nearby device.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub os: Os,
    pub addresses: Vec<String>,
    pub transports: Vec<Transport>,
    pub rssi: Option<i16>,
    pub reachable: bool,
    pub trusted: bool,
}

pub struct Registry {
    self_id: String,
    entries: Mutex<HashMap<String, Entry>>,
    trusted: Mutex<HashSet<String>>,
    pub changed: Notify,
}

impl Registry {
    pub fn new(self_id: String) -> Registry {
        Registry {
            self_id,
            entries: Mutex::new(HashMap::new()),
            trusted: Mutex::new(HashSet::new()),
            changed: Notify::new(),
        }
    }

    pub fn set_trusted(&self, ids: impl IntoIterator<Item = String>) {
        *self.trusted.lock().unwrap() = ids.into_iter().collect();
        self.changed.notify_one();
    }

    /// Records that `info` was seen at `addr` through `transport`.
    pub fn observe(&self, info: DeviceInfo, addr: Option<SocketAddrV4>, transport: Transport) {
        if info.id == self.self_id || info.id.len() != 16 {
            return;
        }
        // A loopback/unspecified address announced by another device would point back to us.
        let addr = addr.filter(|a| {
            let ip = a.ip();
            !ip.is_unspecified()
                && !(ip.is_loopback() && matches!(transport, Transport::Mdns | Transport::Broadcast | Transport::Ble))
        });
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let mut is_new = false;
        let entry = entries.entry(info.id.clone()).or_insert_with(|| {
            is_new = true;
            Entry {
                info: info.clone(),
                addrs: Vec::new(),
                ble: None,
                name_complete: false,
                last_seen: now,
            }
        });
        let mut changed = is_new;
        if transport != Transport::Ble || !entry.name_complete {
            if entry.info.name != info.name || entry.info.os != info.os {
                changed = true;
            }
            entry.info.name = info.name.clone();
            entry.info.os = info.os;
        }
        if transport != Transport::Ble {
            entry.name_complete = true;
            entry.info.port = info.port;
            entry.info.version = info.version;
        }
        entry.last_seen = now;
        if let Some(addr) = addr {
            match entry
                .addrs
                .iter_mut()
                .find(|a| a.addr == addr && a.transport == transport)
            {
                Some(a) => a.last_seen = now,
                None => {
                    entry.addrs.push(AddrEntry {
                        addr,
                        transport,
                        last_seen: now,
                    });
                    changed = true;
                }
            }
        }
        drop(entries);
        if changed {
            self.changed.notify_one();
        }
    }

    pub fn observe_ble(&self, beacon: &BleBeacon, rssi: i16) {
        if beacon.id == self.self_id {
            return;
        }
        let addr = beacon
            .ip
            .filter(|ip| !ip.is_unspecified())
            .map(|ip| SocketAddrV4::new(ip, beacon.port));
        self.observe(
            DeviceInfo {
                id: beacon.id.clone(),
                name: beacon.name_prefix.clone(),
                os: beacon.os,
                port: beacon.port,
                version: PROTOCOL_VERSION,
            },
            addr,
            Transport::Ble,
        );
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get_mut(&beacon.id) {
            let first = e.ble.is_none();
            e.ble = Some((rssi, Instant::now()));
            if first {
                self.changed.notify_one();
            }
        }
    }

    pub fn forget_transport(&self, id: &str, transport: Transport) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get_mut(id) {
            let before = e.addrs.len();
            e.addrs.retain(|a| a.transport != transport);
            if e.addrs.len() != before {
                self.changed.notify_one();
            }
        }
    }

    /// Candidate addresses for `id`, most recently confirmed first.
    pub fn addresses(&self, id: &str) -> Vec<SocketAddrV4> {
        let entries = self.entries.lock().unwrap();
        let Some(e) = entries.get(id) else {
            return Vec::new();
        };
        let mut addrs: Vec<&AddrEntry> = e.addrs.iter().collect();
        addrs.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        let mut out: Vec<SocketAddrV4> = Vec::new();
        for a in addrs {
            if !out.contains(&a.addr) {
                out.push(a.addr);
            }
        }
        out
    }

    pub fn info(&self, id: &str) -> Option<DeviceInfo> {
        self.entries.lock().unwrap().get(id).map(|e| e.info.clone())
    }

    /// Drops stale addresses/devices; returns true when something changed.
    pub fn expire(&self) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let mut changed = false;
        entries.retain(|_, e| {
            let before = e.addrs.len();
            e.addrs
                .retain(|a| now.duration_since(a.last_seen) < a.transport.ttl());
            if e.addrs.len() != before {
                changed = true;
            }
            if let Some((_, seen)) = e.ble {
                if now.duration_since(seen) >= Transport::Ble.ttl() {
                    e.ble = None;
                    changed = true;
                }
            }
            let keep = !e.addrs.is_empty() || e.ble.is_some();
            if !keep {
                changed = true;
            }
            keep
        });
        changed
    }

    pub fn snapshot(&self) -> Vec<DeviceView> {
        let trusted = self.trusted.lock().unwrap().clone();
        let entries = self.entries.lock().unwrap();
        let mut out: Vec<DeviceView> = entries
            .values()
            .map(|e| {
                let mut transports: Vec<Transport> = Vec::new();
                for a in &e.addrs {
                    if !transports.contains(&a.transport) {
                        transports.push(a.transport);
                    }
                }
                if e.ble.is_some() && !transports.contains(&Transport::Ble) {
                    transports.push(Transport::Ble);
                }
                let mut addresses: Vec<String> = Vec::new();
                for a in &e.addrs {
                    let s = a.addr.to_string();
                    if !addresses.contains(&s) {
                        addresses.push(s);
                    }
                }
                DeviceView {
                    id: e.info.id.clone(),
                    name: e.info.name.clone(),
                    os: e.info.os,
                    reachable: !addresses.is_empty(),
                    addresses,
                    transports,
                    rssi: e.ble.map(|(r, _)| r),
                    trusted: trusted.contains(&e.info.id),
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }
}

// ---------------------------------------------------------------------------------------------
// BLE beacon format (manufacturer-specific data, company 0xFFFF, <= 24 bytes so it fits a legacy
// advertising PDU together with the flags AD structure):
//   [0..2]  'O' 'D'
//   [2]     protocol version << 4 | OS code
//   [3..11] device id (8 bytes)
//   [11..15] IPv4 address on the current LAN (0.0.0.0 when offline)
//   [15..17] TCP port, big endian
//   [17..]  first <= 7 bytes of the UTF-8 device name
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleBeacon {
    pub id: String,
    pub os: Os,
    pub ip: Option<Ipv4Addr>,
    pub port: u16,
    pub name_prefix: String,
}

pub const BLE_PAYLOAD_MAX: usize = 24;

pub fn encode_ble_beacon(id: &str, os: Os, ip: Option<Ipv4Addr>, port: u16, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLE_PAYLOAD_MAX);
    out.extend_from_slice(b"OD");
    out.push((PROTOCOL_VERSION << 4) | (os.code() & 0x0f));
    let id_bytes = hex::decode(id).unwrap_or_else(|_| vec![0; 8]);
    let mut id8 = [0u8; 8];
    for (dst, src) in id8.iter_mut().zip(id_bytes.iter()) {
        *dst = *src;
    }
    out.extend_from_slice(&id8);
    out.extend_from_slice(&ip.unwrap_or(Ipv4Addr::UNSPECIFIED).octets());
    out.extend_from_slice(&port.to_be_bytes());
    let mut cut = name.len().min(BLE_PAYLOAD_MAX - out.len());
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    out.extend_from_slice(&name.as_bytes()[..cut]);
    out
}

pub fn decode_ble_beacon(data: &[u8]) -> Option<BleBeacon> {
    if data.len() < 17 || &data[..2] != b"OD" || data[2] >> 4 != PROTOCOL_VERSION {
        return None;
    }
    let ip = Ipv4Addr::new(data[11], data[12], data[13], data[14]);
    let mut name_end = data.len();
    let name = loop {
        match std::str::from_utf8(&data[17..name_end]) {
            Ok(s) => break s.to_string(),
            Err(_) => name_end -= 1,
        }
    };
    Some(BleBeacon {
        id: hex::encode(&data[3..11]),
        os: Os::from_code(data[2] & 0x0f),
        ip: if ip.is_unspecified() { None } else { Some(ip) },
        port: u16::from_be_bytes([data[15], data[16]]),
        name_prefix: name,
    })
}

// ---------------------------------------------------------------------------------------------
// Interfaces and sockets
// ---------------------------------------------------------------------------------------------

pub struct LocalAddr {
    pub interface: String,
    pub ip: Ipv4Addr,
    pub broadcast: Option<Ipv4Addr>,
}

pub fn local_ipv4() -> Vec<LocalAddr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() || !iface.is_oper_up() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = &iface.addr {
                if v4.ip.is_link_local() {
                    continue;
                }
                let broadcast = v4.broadcast.or_else(|| {
                    let ip = u32::from(v4.ip);
                    let mask = u32::from(v4.netmask);
                    if mask == u32::MAX || mask == 0 {
                        None
                    } else {
                        Some(Ipv4Addr::from(ip | !mask))
                    }
                });
                out.push(LocalAddr {
                    interface: iface.name.clone(),
                    ip: v4.ip,
                    broadcast,
                });
            }
        }
    }
    out
}

/// Address advertised in BLE beacons: the first private LAN address that is not a Wi-Fi Direct
/// group address.
pub fn primary_ipv4() -> Option<Ipv4Addr> {
    let addrs = local_ipv4();
    addrs
        .iter()
        .find(|a| {
            a.ip.is_private()
                && !(a.ip.octets()[0] == 192 && a.ip.octets()[1] == 168 && a.ip.octets()[2] == 49)
        })
        .or_else(|| addrs.iter().find(|a| a.ip.is_private()))
        .map(|a| a.ip)
}

pub fn tune_stream(stream: &TcpStream) {
    let sock = SockRef::from(stream);
    let _ = sock.set_tcp_nodelay(true);
    let _ = sock.set_send_buffer_size(SOCKET_BUFFER);
    let _ = sock.set_recv_buffer_size(SOCKET_BUFFER);
    let _ = sock.set_keepalive(true);
    let _ = sock.set_tcp_keepalive(&TcpKeepalive::new().with_time(Duration::from_secs(15)));
}

pub async fn connect_tuned(addr: SocketAddrV4, timeout: Duration) -> io::Result<TcpStream> {
    let socket = TcpSocket::new_v4()?;
    socket.set_send_buffer_size(SOCKET_BUFFER as u32)?;
    socket.set_recv_buffer_size(SOCKET_BUFFER as u32)?;
    let stream = tokio::time::timeout(timeout, socket.connect(SocketAddr::V4(addr)))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, format!("connect {addr} timed out"))
        })??;
    tune_stream(&stream);
    Ok(stream)
}

fn bind_listener() -> io::Result<TcpListener> {
    let try_bind = |port: u16| -> io::Result<TcpListener> {
        let socket = TcpSocket::new_v4()?;
        #[cfg(unix)]
        socket.set_reuseaddr(true)?;
        socket.set_recv_buffer_size(SOCKET_BUFFER as u32)?;
        socket.set_send_buffer_size(SOCKET_BUFFER as u32)?;
        socket.bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            port,
        )))?;
        socket.listen(1024)
    };
    match try_bind(TCP_PORT) {
        Ok(l) => Ok(l),
        Err(e) => {
            events::log(
                "warn",
                format!("TCP port {TCP_PORT} unavailable ({e}); using an ephemeral port"),
            );
            try_bind(0)
        }
    }
}

/// Binds the transfer listener and returns it with its actual port.
pub fn start_listener() -> io::Result<(TcpListener, u16)> {
    let listener = bind_listener()?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub async fn accept_loop(core: Arc<Core>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tune_stream(&stream);
                let core = core.clone();
                tokio::spawn(async move {
                    if let Err(e) = dispatch_connection(core, stream, peer).await {
                        if e.kind() != io::ErrorKind::UnexpectedEof {
                            events::log("debug", format!("connection from {peer}: {e}"));
                        }
                    }
                });
            }
            Err(e) => {
                events::log("warn", format!("accept failed: {e}"));
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn dispatch_connection(
    core: Arc<Core>,
    mut stream: TcpStream,
    peer: SocketAddr,
) -> io::Result<()> {
    let mut magic = [0u8; MAGIC_LEN];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut magic))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no preamble"))??;
    match &magic {
        m if m == MAGIC_CTRL => transfer::handle_incoming_control(core, stream, peer).await,
        m if m == MAGIC_DATA => transfer::handle_data_stream(core, stream).await,
        m if m == MAGIC_PROBE => handle_probe(core, stream, peer).await,
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown preamble",
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Probe: unauthenticated identity exchange used after Wi-Fi Direct links and for manual peers.
// Authentication still happens later through the Noise handshake (id = hash of static key).
// ---------------------------------------------------------------------------------------------

async fn write_json_frame(stream: &mut TcpStream, value: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    stream.write_all(&frame).await
}

async fn read_json_frame<T: for<'de> Deserialize<'de>>(stream: &mut TcpStream) -> io::Result<T> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > 16 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "probe frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn handle_probe(core: Arc<Core>, mut stream: TcpStream, peer: SocketAddr) -> io::Result<()> {
    let theirs: DeviceInfo =
        tokio::time::timeout(Duration::from_secs(5), read_json_frame(&mut stream))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "probe timeout"))??;
    write_json_frame(
        &mut stream,
        &serde_json::to_value(core.device_info()).unwrap(),
    )
    .await?;
    if let SocketAddr::V4(v4) = peer {
        core.registry.observe(
            theirs.clone(),
            Some(SocketAddrV4::new(*v4.ip(), theirs.port)),
            Transport::Incoming,
        );
    }
    Ok(())
}

pub async fn probe(
    core: &Arc<Core>,
    addr: SocketAddrV4,
    transport: Transport,
) -> io::Result<DeviceInfo> {
    let mut stream = connect_tuned(addr, Duration::from_secs(4)).await?;
    stream.write_all(MAGIC_PROBE).await?;
    write_json_frame(
        &mut stream,
        &serde_json::to_value(core.device_info()).unwrap(),
    )
    .await?;
    let info: DeviceInfo =
        tokio::time::timeout(Duration::from_secs(5), read_json_frame(&mut stream))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "probe timeout"))??;
    core.registry.observe(
        info.clone(),
        Some(SocketAddrV4::new(*addr.ip(), info.port)),
        transport,
    );
    Ok(info)
}

// ---------------------------------------------------------------------------------------------
// UDP broadcast discovery
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Beacon {
    t: String,
    v: u8,
    id: String,
    n: String,
    o: Os,
    p: u16,
    #[serde(default)]
    q: bool,
}

fn udp_socket() -> io::Result<UdpSocket> {
    let make = |port: u16| -> io::Result<std::net::UdpSocket> {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
        s.set_reuse_port(true)?;
        s.set_broadcast(true)?;
        s.set_nonblocking(true)?;
        s.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).into())?;
        Ok(s.into())
    };
    let std_socket = match make(UDP_PORT) {
        Ok(s) => s,
        Err(e) => {
            events::log(
                "warn",
                format!("UDP {UDP_PORT} unavailable ({e}); broadcast replies only"),
            );
            make(0)?
        }
    };
    UdpSocket::from_std(std_socket)
}

fn beacon_bytes(core: &Core, query: bool) -> Vec<u8> {
    let info = core.device_info();
    serde_json::to_vec(&Beacon {
        t: "omnidrop".into(),
        v: info.version,
        id: info.id,
        n: info.name,
        o: info.os,
        p: info.port,
        q: query,
    })
    .unwrap_or_default()
}

async fn broadcast_once(socket: &UdpSocket, payload: &[u8]) {
    let mut targets: Vec<Ipv4Addr> = vec![Ipv4Addr::BROADCAST];
    for a in local_ipv4() {
        if let Some(b) = a.broadcast {
            if !targets.contains(&b) {
                targets.push(b);
            }
        }
    }
    for t in targets {
        let _ = socket
            .send_to(payload, SocketAddr::V4(SocketAddrV4::new(t, UDP_PORT)))
            .await;
    }
}

async fn broadcast_loop(core: Arc<Core>, wake: Arc<Notify>) {
    let socket = match udp_socket() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            events::log("error", format!("UDP discovery disabled: {e}"));
            return;
        }
    };
    let rx_socket = socket.clone();
    let rx_core = core.clone();
    let receiver = tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let Ok((n, from)) = rx_socket.recv_from(&mut buf).await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let Ok(b) = serde_json::from_slice::<Beacon>(&buf[..n]) else {
                continue;
            };
            if b.t != "omnidrop" || b.id == rx_core.identity.id {
                continue;
            }
            let SocketAddr::V4(from4) = from else {
                continue;
            };
            rx_core.registry.observe(
                DeviceInfo {
                    id: b.id,
                    name: b.n,
                    os: b.o,
                    port: b.p,
                    version: b.v,
                },
                Some(SocketAddrV4::new(*from4.ip(), b.p)),
                Transport::Broadcast,
            );
            if b.q {
                let reply = beacon_bytes(&rx_core, false);
                let _ = rx_socket.send_to(&reply, from).await;
            }
        }
    });
    let _guard = AbortOnDrop(receiver);
    // First beacons ask everybody to answer immediately for a fast initial radar.
    let mut query_rounds = 3;
    loop {
        let payload = beacon_bytes(&core, query_rounds > 0);
        query_rounds = (query_rounds - 1).max(0);
        broadcast_once(&socket, &payload).await;
        tokio::select! {
            _ = tokio::time::sleep(BEACON_INTERVAL) => {}
            _ = wake.notified() => { query_rounds = 2; }
        }
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// ---------------------------------------------------------------------------------------------
// mDNS / DNS-SD
// ---------------------------------------------------------------------------------------------

fn start_mdns(core: &Arc<Core>, stop: Arc<AtomicBool>) -> io::Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new().map_err(|e| io::Error::other(e.to_string()))?;
    // Transfers are IPv4 and loopback addresses must never be advertised to other devices.
    for kind in [IfKind::LoopbackV4, IfKind::LoopbackV6, IfKind::IPv6] {
        let _ = daemon.disable_interface(kind);
    }
    let info = core.device_info();
    let mut props = HashMap::new();
    props.insert("id".to_string(), info.id.clone());
    props.insert("n".to_string(), info.name.clone());
    props.insert("o".to_string(), info.os.as_str().to_string());
    props.insert("v".to_string(), info.version.to_string());
    let service = ServiceInfo::new(
        MDNS_SERVICE,
        &info.id,
        &format!("omnidrop-{}.local.", info.id),
        "",
        info.port,
        props,
    )
    .map_err(|e| io::Error::other(e.to_string()))?
    .enable_addr_auto();
    daemon
        .register(service)
        .map_err(|e| io::Error::other(e.to_string()))?;

    let browse_daemon = daemon.clone();
    let core = core.clone();
    std::thread::Builder::new()
        .name("omnidrop-mdns".into())
        .spawn(move || {
            let mut receiver = browse_daemon.browse(MDNS_SERVICE).ok();
            let mut last_browse = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                // Re-browse periodically: cached answers are re-emitted, which refreshes TTLs.
                if last_browse.elapsed() > Duration::from_secs(30) {
                    let _ = browse_daemon.stop_browse(MDNS_SERVICE);
                    receiver = browse_daemon.browse(MDNS_SERVICE).ok();
                    last_browse = Instant::now();
                }
                let Some(rx) = receiver.as_ref() else {
                    std::thread::sleep(Duration::from_secs(1));
                    receiver = browse_daemon.browse(MDNS_SERVICE).ok();
                    continue;
                };
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(ServiceEvent::ServiceResolved(svc)) => {
                        let Some(id) = svc.get_property_val_str("id").map(str::to_string) else {
                            continue;
                        };
                        let name = svc.get_property_val_str("n").unwrap_or(&id).to_string();
                        let os = Os::parse(svc.get_property_val_str("o").unwrap_or(""));
                        let version = svc
                            .get_property_val_str("v")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(PROTOCOL_VERSION);
                        let port = svc.get_port();
                        for ip in svc.get_addresses_v4() {
                            core.registry.observe(
                                DeviceInfo {
                                    id: id.clone(),
                                    name: name.clone(),
                                    os,
                                    port,
                                    version,
                                },
                                Some(SocketAddrV4::new(ip, port)),
                                Transport::Mdns,
                            );
                        }
                    }
                    Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                        if let Some(id) = fullname.split('.').next() {
                            core.registry.forget_transport(id, Transport::Mdns);
                        }
                    }
                    Ok(_) => {}
                    Err(mdns_sd::RecvTimeoutError::Timeout) => {}
                    Err(mdns_sd::RecvTimeoutError::Disconnected) => {
                        receiver = None;
                    }
                }
            }
        })?;
    Ok(daemon)
}

// ---------------------------------------------------------------------------------------------
// Discovery lifecycle
// ---------------------------------------------------------------------------------------------

pub struct Discovery {
    tasks: Vec<JoinHandle<()>>,
    mdns: Option<ServiceDaemon>,
    mdns_stop: Arc<AtomicBool>,
    ble: Option<crate::platform::BleHandle>,
    pub wake: Arc<Notify>,
}

impl Discovery {
    pub fn start(core: &Arc<Core>) -> Discovery {
        let settings = core.settings.read().unwrap().clone();
        let wake = Arc::new(Notify::new());
        let mut tasks = Vec::new();
        if settings.broadcast {
            tasks.push(tokio::spawn(broadcast_loop(core.clone(), wake.clone())));
        }
        let mdns_stop = Arc::new(AtomicBool::new(false));
        let mdns = if settings.mdns {
            match start_mdns(core, mdns_stop.clone()) {
                Ok(d) => Some(d),
                Err(e) => {
                    events::log("warn", format!("mDNS unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let ble = if settings.ble && !core.host_handles_radio {
            match crate::platform::start_ble(core.clone()) {
                Ok(h) => Some(h),
                Err(e) => {
                    events::emit(
                        "ble_state",
                        json!({"available": false, "active": false, "error": e.to_string()}),
                    );
                    None
                }
            }
        } else {
            None
        };
        Discovery {
            tasks,
            mdns,
            mdns_stop,
            ble,
            wake,
        }
    }

    pub fn refresh(&self) {
        self.wake.notify_one();
    }

    pub fn stop(mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
        self.mdns_stop.store(true, Ordering::Relaxed);
        if let Some(d) = self.mdns.take() {
            let _ = d.shutdown();
        }
        if let Some(b) = self.ble.take() {
            b.stop();
        }
    }
}

/// Publishes the device list to the UI (debounced) and expires stale entries.
pub async fn registry_publisher(core: Arc<Core>) {
    let mut last = String::new();
    loop {
        tokio::select! {
            _ = core.registry.changed.notified() => {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                core.registry.expire();
            }
        }
        let devices = core.registry.snapshot();
        let text = serde_json::to_string(&devices).unwrap_or_default();
        if text != last {
            last = text;
            events::emit("devices", json!({ "devices": devices }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ble_beacon_roundtrip() {
        let payload = encode_ble_beacon(
            "0123456789abcdef",
            Os::Android,
            Some(Ipv4Addr::new(192, 168, 1, 7)),
            44850,
            "Pixèl 8 Pro",
        );
        assert!(payload.len() <= BLE_PAYLOAD_MAX);
        let b = decode_ble_beacon(&payload).unwrap();
        assert_eq!(b.id, "0123456789abcdef");
        assert_eq!(b.os, Os::Android);
        assert_eq!(b.ip, Some(Ipv4Addr::new(192, 168, 1, 7)));
        assert_eq!(b.port, 44850);
        assert!("Pixèl 8 Pro".starts_with(&b.name_prefix));
        assert!(!b.name_prefix.is_empty());
    }

    #[test]
    fn registry_tracks_and_expires() {
        let r = Registry::new("ffffffffffffffff".into());
        let info = DeviceInfo {
            id: "0011223344556677".into(),
            name: "Laptop".into(),
            os: Os::Linux,
            port: 44850,
            version: 1,
        };
        let addr = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 2), 44850);
        r.observe(info, Some(addr), Transport::Broadcast);
        assert_eq!(r.addresses("0011223344556677"), vec![addr]);
        let snap = r.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].reachable);
    }

    #[test]
    fn ignores_loopback_from_discovery() {
        let r = Registry::new("ffffffffffffffff".into());
        let info = DeviceInfo {
            id: "0011223344556677".into(),
            name: "Remote".into(),
            os: Os::Windows,
            port: 44850,
            version: 1,
        };
        let lo = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 44850);
        r.observe(info.clone(), Some(lo), Transport::Mdns);
        assert!(r.addresses("0011223344556677").is_empty());
        r.observe(info, Some(lo), Transport::Manual);
        assert_eq!(r.addresses("0011223344556677"), vec![lo]);
    }
}

