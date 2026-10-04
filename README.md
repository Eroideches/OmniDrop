<p align="center"><img src="assets/icon.png" width="112" alt="OmniDrop"></p>

<h1 align="center">OmniDrop</h1>

<p align="center">Fast, end-to-end encrypted peer-to-peer file transfer for <b>Android</b>, <b>Windows</b> and <b>Linux</b>.</p>

OmniDrop sends files, whole folders, clipboard text and links between nearby devices. It finds
them on the local network (mDNS + UDP broadcast), sees them over Bluetooth LE even without a
network, and can create a Wi-Fi Direct link when there is no router at all. Every transfer is
authenticated with a Noise_XX handshake and confirmed with a short code shown on both screens.

## Downloads

Grab the latest build from [Releases](https://github.com/Eroideches/OmniDrop/releases/latest):

| Platform | File |
|---|---|
| Android 8.0+ (64-bit ARM, most phones) | `omnidrop-android-arm64.apk` |
| Android 8.0+ (32-bit ARM) | `omnidrop-android-arm32.apk` |
| Android x86_64 (emulators, Chromebooks) | `omnidrop-android-x86_64.apk` |
| Windows 10/11 installer | `omnidrop-windows-x64-setup.exe` |
| Windows 10/11 portable | `omnidrop-windows-x64.zip` |
| Ubuntu 22.04+ / Debian 12+ | `omnidrop-linux-x64.deb` |
| Any Linux with glibc 2.35+ | `omnidrop-linux-x64.tar.gz` → `./install.sh` (or `sudo ./install.sh`) |
| Arch Linux | `PKGBUILD` → `makepkg -si` |

## Features

- **Radar dashboard** – animated radial map of nearby devices, grouped by link type, with
  Android / Windows / Linux icons, trust badges and link indicators.
- **Drag and drop** (Windows, Linux) – drop files or folders straight onto a device on the radar.
- **Send anything** – multiple files, folders (structure and empty directories preserved),
  clipboard text, typed text and links; "Share → OmniDrop" from any Android app.
- **Transfer monitor** – live throughput chart (MB/s), percentage, ETA, block-level progress map,
  parallel stream count, cipher in use, pause / resume / cancel on both sides.
- **Incoming request dialog** – sender, file list with sizes, image thumbnails, message preview
  and the verification code (6-digit PIN + 4 emoji); optional "trust this device" with
  auto-accept.
- **Resumable transfers** – interrupted transfers resume automatically; partial files survive
  app restarts and are reused when the same file is sent again.

## Architecture

```
lib/                 Flutter UI (Dart)  ──FFI (JSON)──►  native/  Rust core (cdylib)
android/ (Kotlin)    BLE advertise/scan, Wi-Fi Direct, foreground service, share intents
linux/, windows/     Flutter runners; CMake builds the Rust core with cargo and bundles it
```

### Rust core (`native/src/`)

| Module | What it does |
|---|---|
| `networking.rs` | Device registry fed by mDNS/DNS-SD (`_omnidrop._tcp`), UDP broadcast beacons on every IPv4 interface (incl. Wi-Fi Direct groups), BLE beacons and TCP probes; tuned sockets (`TCP_NODELAY`, 4 MiB `SO_SNDBUF`/`SO_RCVBUF` set before connect/listen, keep-alive); single TCP port with preamble dispatch (control / data / probe). |
| `security.rs` | `Noise_XX_25519_ChaChaPoly_SHA256` mutual authentication of the long-term Curve25519 keys (device id = SHA-256 of the static key), session key = HKDF over the handshake hash and both peers' random contributions, AES-256-GCM when both CPUs have AES instructions (AES-NI/PCLMULQDQ, ARMv8 AES/PMULL) otherwise ChaCha20-Poly1305, SAS = 6-digit PIN + 4 emoji (~44 bits) from the handshake hash. |
| `transfer.rs` | Files split into 512 KiB blocks; each data frame carries 1–8 blocks (512 KiB–4 MiB) sized adaptively per stream (~150 ms per frame); N parallel TCP streams (default 4); SHA-256 of every block verified by the receiver; persistent bitmaps for resume; end-to-end root hash check before the atomic rename into place; path sanitisation against traversal and reserved names. |
| `diskio.rs` | Positional disk I/O: io_uring on Linux (fallback to `pread`/`pwrite`), overlapped positional I/O on Windows. Buffers are filled by the kernel, encrypted/decrypted **in place** and written with a single syscall – no intermediate copies in user space. |
| `platform/` | Linux: BlueZ LE advertising + discovery and NetworkManager Wi-Fi P2P over D-Bus (zbus). Windows: WinRT Bluetooth LE advertisement publisher/watcher and `Windows.Devices.WiFiDirect`. |

Sockets are driven by Tokio, i.e. epoll on Linux/Android and IOCP on Windows.

**Measured throughput** (loopback, sender and receiver on the same 4-core laptop, AES-256-GCM,
io_uring): **118 MB/s** – `cargo test --release -- --ignored --nocapture loopback_throughput`.
Between two devices the network link is the limit (Gigabit Ethernet ≈ 110 MB/s, Wi-Fi 5/6
typically 30–100 MB/s).

### Wire protocol (v1)

1. TCP connect to port 44850 and send an 8-byte preamble (`ODCTRL01`, `ODDATA01`, `ODPROBE1`).
2. Control: Noise_XX handshake → encrypted hello with a 32-byte contribution → JSON control
   records (`offer`, `decision`, `progress`, `file_done`, `file_result`, `pause`, `resume`,
   `cancel`, `complete`) as AEAD records `[u32 len][ciphertext][tag]`.
3. Data: `ODDATA01` + session id + stream index, then frames
   `[32-byte header (AAD)][block hashes ‖ data (encrypted)][tag]`, per-stream key and nonce counter.
4. Discovery: UDP 44851 broadcast beacons, mDNS `_omnidrop._tcp.local.`, BLE manufacturer data
   (company 0xFFFF, `OD` + version/OS + id + IPv4 + port + name prefix).

## Security model

- The Noise_XX handshake authenticates both long-term keys and provides forward secrecy.
- The verification code is derived from the handshake transcript: a man-in-the-middle ends up
  with two different codes. Compare it on both screens the first time; trusted devices are
  remembered by their full public key.
- A device id is the hash of its public key, so a device cannot impersonate another one's id;
  the sender also checks that the key at an address matches the device it selected.
- Every block is authenticated by the AEAD and verified again with SHA-256, and the whole file is
  checked against the sender's root hash before it is moved into place.
- Nothing is accepted without an explicit "Accept" unless you enabled auto-accept for a trusted
  device.

## Platform notes

### Android (8.0+)
Permissions are requested on first launch: nearby Wi-Fi/Bluetooth devices, location (required by
Android ≤ 12 for scans), **All files access** (`MANAGE_EXTERNAL_STORAGE`: send any file without
copying it and save to `Download/OmniDrop`), notifications, and exclusion from battery
optimisation (`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`) so long transfers keep running with the
`dataSync` foreground service.

### Windows (10/11)
The installer can add firewall rules (`firewall-setup.ps1`, also included in the zip). BLE uses the
WinRT advertisement APIs and Wi-Fi Direct uses `Windows.Devices.WiFiDirect`; the installer and
the zip run as regular desktop apps, which need no capability declaration for these APIs. For an
optional MSIX package, `packaging/windows/Package.appxmanifest` declares the `wiFiControl`,
`bluetooth`, `radios` and `privateNetworkClientServer` capabilities.

### Linux (Ubuntu / Debian / Arch)
Wi-Fi Direct goes through NetworkManager's D-Bus API (NM ≥ 1.16 with a P2P-capable
wpa_supplicant). The packages install a polkit rule (`.rules`, plus a `.pkla` for polkit 0.105)
that lets users of an active local session in `sudo`/`wheel`/`netdev`/`network` create their own
P2P connections without a password. BLE needs BlueZ.

### Off-grid
BLE beacons are used for discovery only; data always flows over IP. When two devices see each
other only over Bluetooth, tap the device and choose **Connect with Wi-Fi Direct**: on Android the
peer is matched automatically through Wi-Fi Direct service discovery, on desktop you pick it from
the Wi-Fi Direct list. Cross-vendor Wi-Fi Direct support depends on the Wi-Fi driver.

## Building

Requirements: Flutter 3.47+, Rust (stable), plus per platform:

```bash
# Linux (Ubuntu): sudo apt install clang cmake ninja-build pkg-config libgtk-3-dev liblzma-dev
flutter build linux --release          # CMake runs cargo and bundles libomnidrop_core.so

# Windows (Visual Studio 2022 with "Desktop development with C++")
flutter build windows --release        # CMake runs cargo and copies omnidrop_core.dll

# Android (JDK 17, Android SDK + NDK, cargo install cargo-ndk,
#          rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android)
scripts/build-native.sh android
flutter build apk --release --split-per-abi --target-platform android-arm,android-arm64,android-x64
```

Tests: `cd native && cargo test` (unit + end-to-end transfers over loopback) and `flutter test`.

### Release pipeline
Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`: Rust tests and clippy, then
Linux (`.tar.gz`, `.deb`, `PKGBUILD`), Windows (`.zip`, setup `.exe`) and Android (three split
APKs) builds, and a GitHub Release with all files and `SHA256SUMS.txt`.

Android release APKs leave CI unsigned on purpose – no signing key or password is stored on
GitHub. The maintainer signs them locally with `tools/firma-release.sh` (key password kept in
the desktop keyring) and attaches `omnidrop-android-*.apk` to the release.

## Licence

MIT – see [LICENSE](LICENSE).
