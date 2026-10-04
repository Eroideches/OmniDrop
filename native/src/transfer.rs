//! Transfer engine.
//!
//! * Files are split into fixed 512 KiB *blocks* (the unit of integrity and resume). A data
//!   frame carries a *chunk* of 1..=8 consecutive blocks (512 KiB .. 4 MiB); every stream adapts
//!   its chunk size to keep each frame around 150 ms on the wire.
//! * N parallel TCP data streams (default 4) pull chunks from a shared scheduler, read them with
//!   positional I/O into a pooled buffer, hash every block with SHA-256, encrypt in place and
//!   send header + ciphertext + tag with a single write.
//! * The receiver decrypts in place, verifies the SHA-256 of every block, writes it at its offset
//!   and records the block in a persistent bitmap (`.omnidrop-partial/<key>.state`).
//! * Resume: after a network failure the sender reconnects automatically and re-offers the same
//!   transfer; the receiver answers with the blocks it already holds (bitmap + hashes), the
//!   sender re-hashes those blocks from its own file and skips the ones that match. Partial
//!   files also survive restarts and are matched by (peer key, path, size, mtime).
//! * End-to-end check: `root = SHA-256(h0 || h1 || ...)` over all block hashes, compared before
//!   the partial file is atomically renamed into place.

use crate::config::TrustedPeer;
use crate::engine::Core;
use crate::events;
use crate::networking::{self, MAGIC_CTRL, MAGIC_DATA, Transport};
use crate::security::{
    self, Aead, CipherSuite, RecordReader, RecordWriter, Sas, SessionKeys, TAG_LEN, nonce_for,
};
use crate::util::{self, Os};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Notify, oneshot, watch};

pub const BLOCK_SIZE: u64 = 512 * 1024;
pub const MAX_CHUNK_BLOCKS: u32 = 8;
pub const HASH_LEN: usize = 32;
const DATA_HDR_LEN: usize = 32;
const DATA_NONCE_PREFIX: [u8; 4] = *b"DATA";
const FRAME_CHUNK: &[u8; 4] = b"ODCH";
const FRAME_END: &[u8; 4] = b"ODEN";
const PARTIAL_DIR: &str = ".omnidrop-partial";
const STATE_MAGIC: &[u8; 8] = b"ODST1\0\0\0";
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const RECONNECT_WINDOW: Duration = Duration::from_secs(180);
const MAX_RECONNECTS: u32 = 20;
const UI_FILE_LIMIT: usize = 200;

type Hash = [u8; HASH_LEN];

pub fn blocks_for(size: u64) -> usize {
    size.div_ceil(BLOCK_SIZE) as usize
}

fn block_range(size: u64, block: usize) -> (u64, u64) {
    let start = block as u64 * BLOCK_SIZE;
    (start, (start + BLOCK_SIZE).min(size))
}

fn root_of(hashes: &[Hash]) -> Hash {
    let mut h = Sha256::new();
    for x in hashes {
        h.update(x);
    }
    h.finalize().into()
}

fn sha256(data: &[u8]) -> Hash {
    Sha256::digest(data).into()
}

fn io_err(kind: io::ErrorKind, msg: impl Into<String>) -> io::Error {
    io::Error::new(kind, msg.into())
}

// ---------------------------------------------------------------------------------------------
// Wire protocol (JSON records inside the encrypted control channel)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferItem {
    pub index: u32,
    /// Relative path, '/'-separated.
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeInfo {
    index: u32,
    bitmap: String,
    hashes: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Ctrl {
    Offer {
        transfer_id: String,
        items: Vec<OfferItem>,
        text: Option<String>,
        thumbs: HashMap<u32, String>,
        total_size: u64,
        block_size: u64,
        streams: u16,
        resume: bool,
    },
    Decision {
        accepted: bool,
        reason: Option<String>,
        session_id: String,
        resume: Vec<ResumeInfo>,
    },
    Progress {
        bytes: u64,
    },
    FileDone {
        index: u32,
        root: String,
    },
    FileResult {
        index: u32,
        ok: bool,
        error: Option<String>,
    },
    Pause,
    Resume,
    Cancel {
        reason: String,
    },
    Complete,
    Ping,
}

// ---------------------------------------------------------------------------------------------
// Block bookkeeping
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default, Debug)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    pub fn new(len: usize) -> BitSet {
        BitSet {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }
    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.words[i / 64] & (1 << (i % 64)) != 0
    }
    pub fn set(&mut self, i: usize) {
        if i < self.len {
            self.words[i / 64] |= 1 << (i % 64);
        }
    }
    pub fn clear(&mut self, i: usize) {
        if i < self.len {
            self.words[i / 64] &= !(1 << (i % 64));
        }
    }
    pub fn clear_all(&mut self) {
        self.words.iter_mut().for_each(|w| *w = 0);
    }
    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }
    pub fn is_full(&self) -> bool {
        self.count() == self.len
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.len.div_ceil(8)];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = (self.words[i / 8] >> ((i % 8) * 8)) as u8;
        }
        out
    }
    pub fn from_bytes(bytes: &[u8], len: usize) -> BitSet {
        let mut s = BitSet::new(len);
        for i in 0..len {
            if bytes.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0) {
                s.set(i);
            }
        }
        s
    }
}

#[derive(Debug)]
struct FileProgress {
    size: u64,
    dir: bool,
    nblocks: usize,
    done: BitSet,
    inflight: BitSet,
    hashes: Vec<Hash>,
    hint: usize,
    /// Sender: FileDone sent. Receiver: file finalized (renamed into place or failed).
    closed: bool,
    failed: bool,
}

impl FileProgress {
    fn new(size: u64, dir: bool) -> FileProgress {
        let nblocks = if dir { 0 } else { blocks_for(size) };
        FileProgress {
            size,
            dir,
            nblocks,
            done: BitSet::new(nblocks),
            inflight: BitSet::new(nblocks),
            hashes: vec![[0u8; HASH_LEN]; nblocks],
            hint: 0,
            closed: false,
            failed: false,
        }
    }

    fn complete(&self) -> bool {
        self.done.count() == self.nblocks
    }

    fn done_bytes(&self) -> u64 {
        if self.complete() {
            return self.size;
        }
        let mut total = 0;
        for i in 0..self.nblocks {
            if self.done.get(i) {
                let (s, e) = block_range(self.size, i);
                total += e - s;
            }
        }
        total
    }
}

#[derive(Debug, Default)]
struct Progress {
    files: Vec<FileProgress>,
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    file: usize,
    first_block: usize,
    nblocks: usize,
    offset: u64,
    len: u64,
}

// ---------------------------------------------------------------------------------------------
// Transfer (shared state for both directions, published to the UI)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TState {
    Preparing,
    Connecting,
    WaitingAccept,
    Verifying,
    Transferring,
    Finalizing,
    Reconnecting,
    Interrupted,
    Completed,
    Rejected,
    Cancelled,
    Failed,
}

impl TState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TState::Completed | TState::Rejected | TState::Cancelled | TState::Failed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerRef {
    pub id: String,
    pub name: String,
    pub os: Os,
}

struct SpeedMeter {
    last_bytes: u64,
    last_at: Instant,
    speed: f64,
}

pub struct Transfer {
    pub id: String,
    pub direction: Direction,
    peer: Mutex<PeerRef>,
    state: Mutex<TState>,
    error: Mutex<Option<String>>,
    sas: Mutex<Option<Sas>>,
    cipher: Mutex<Option<CipherSuite>>,
    items: Mutex<Vec<OfferItem>>,
    progress: Mutex<Progress>,
    done_bytes: AtomicU64,
    total_bytes: AtomicU64,
    streams: AtomicU16,
    local_paused: AtomicBool,
    remote_paused: AtomicBool,
    paused_tx: watch::Sender<bool>,
    cancel_tx: watch::Sender<bool>,
    accepted: AtomicBool,
    speed: Mutex<SpeedMeter>,
    started_ms: i64,
    saved_paths: Mutex<Vec<String>>,
    dest_dir: Mutex<String>,
    text: Mutex<Option<String>>,
    dirty: AtomicBool,
    pub changed: Notify,
}

impl Transfer {
    fn new(id: String, direction: Direction, peer: PeerRef) -> Arc<Transfer> {
        Arc::new(Transfer {
            id,
            direction,
            peer: Mutex::new(peer),
            state: Mutex::new(TState::Preparing),
            error: Mutex::new(None),
            sas: Mutex::new(None),
            cipher: Mutex::new(None),
            items: Mutex::new(Vec::new()),
            progress: Mutex::new(Progress::default()),
            done_bytes: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            streams: AtomicU16::new(0),
            local_paused: AtomicBool::new(false),
            remote_paused: AtomicBool::new(false),
            paused_tx: watch::channel(false).0,
            cancel_tx: watch::channel(false).0,
            accepted: AtomicBool::new(false),
            speed: Mutex::new(SpeedMeter {
                last_bytes: 0,
                last_at: Instant::now(),
                speed: 0.0,
            }),
            started_ms: util::now_ms(),
            saved_paths: Mutex::new(Vec::new()),
            dest_dir: Mutex::new(String::new()),
            text: Mutex::new(None),
            dirty: AtomicBool::new(true),
            changed: Notify::new(),
        })
    }

    fn touch(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn state(&self) -> TState {
        *self.state.lock().unwrap()
    }

    fn set_state(&self, s: TState) {
        let mut st = self.state.lock().unwrap();
        if st.is_terminal() {
            return;
        }
        *st = s;
        drop(st);
        self.touch();
        self.changed.notify_waiters();
    }

    fn fail(&self, msg: impl Into<String>) {
        *self.error.lock().unwrap() = Some(msg.into());
        self.set_state(TState::Failed);
    }

    fn set_items(&self, items: Vec<OfferItem>) {
        let mut p = self.progress.lock().unwrap();
        p.files = items
            .iter()
            .map(|i| FileProgress::new(i.size, i.dir))
            .collect();
        drop(p);
        let total: u64 = items.iter().filter(|i| !i.dir).map(|i| i.size).sum();
        self.total_bytes.store(total, Ordering::Relaxed);
        *self.items.lock().unwrap() = items;
        self.touch();
    }

    fn is_cancelled(&self) -> bool {
        *self.cancel_tx.borrow()
    }

    pub fn cancel_local(&self) {
        self.cancel_tx.send_replace(true);
        self.touch();
    }

    fn update_paused(&self) {
        let p =
            self.local_paused.load(Ordering::Relaxed) || self.remote_paused.load(Ordering::Relaxed);
        self.paused_tx.send_replace(p);
        self.touch();
    }

    pub fn set_local_paused(&self, paused: bool) {
        self.local_paused.store(paused, Ordering::Relaxed);
        self.update_paused();
    }

    fn set_remote_paused(&self, paused: bool) {
        self.remote_paused.store(paused, Ordering::Relaxed);
        self.update_paused();
    }

    async fn wait_unpaused(&self) {
        let mut rx = self.paused_tx.subscribe();
        let mut cancel = self.cancel_tx.subscribe();
        while *rx.borrow_and_update() && !*cancel.borrow_and_update() {
            tokio::select! {
                r = rx.changed() => if r.is_err() { return },
                r = cancel.changed() => if r.is_err() { return },
            }
        }
    }

    fn add_done(&self, bytes: u64) {
        self.done_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.touch();
    }

    fn recompute_done(&self) {
        let p = self.progress.lock().unwrap();
        let total: u64 = p.files.iter().map(FileProgress::done_bytes).sum();
        self.done_bytes.store(total, Ordering::Relaxed);
        drop(p);
        self.touch();
    }

    fn block_map(&self, cells: usize) -> (String, usize, usize) {
        let p = self.progress.lock().unwrap();
        let total: usize = p.files.iter().map(|f| f.nblocks).sum();
        let done: usize = p.files.iter().map(|f| f.done.count()).sum();
        if total == 0 {
            let all = !p.files.is_empty() && p.files.iter().all(|f| f.closed);
            return (if all { "2".into() } else { String::new() }, 0, 0);
        }
        let n = cells.min(total).max(1);
        // Single pass: (blocks in cell, done blocks, any in flight).
        let mut acc = vec![(0usize, 0usize, false); n];
        let mut global = 0usize;
        for f in &p.files {
            for b in 0..f.nblocks {
                let cell = global * n / total;
                let a = &mut acc[cell];
                a.0 += 1;
                if f.done.get(b) {
                    a.1 += 1;
                } else if f.inflight.get(b) {
                    a.2 = true;
                }
                global += 1;
            }
        }
        let map = acc
            .iter()
            .map(|&(count, d, inflight)| {
                if count > 0 && d == count {
                    '2'
                } else if d > 0 || inflight {
                    '1'
                } else {
                    '0'
                }
            })
            .collect();
        (map, total, done)
    }

    fn speed_tick(&self) -> f64 {
        let mut m = self.speed.lock().unwrap();
        let now = Instant::now();
        let dt = now.duration_since(m.last_at).as_secs_f64();
        if dt >= 0.4 {
            let bytes = self.done_bytes.load(Ordering::Relaxed);
            let inst = bytes.saturating_sub(m.last_bytes) as f64 / dt;
            m.speed = if m.speed == 0.0 {
                inst
            } else {
                m.speed * 0.6 + inst * 0.4
            };
            m.last_bytes = bytes;
            m.last_at = now;
        }
        m.speed
    }

    pub fn snapshot(&self) -> Value {
        let state = self.state();
        let paused = *self.paused_tx.borrow();
        let speed = if state == TState::Transferring && !paused {
            self.speed_tick()
        } else {
            let mut m = self.speed.lock().unwrap();
            m.last_at = Instant::now();
            m.last_bytes = self.done_bytes.load(Ordering::Relaxed);
            m.speed = 0.0;
            0.0
        };
        let total = self.total_bytes.load(Ordering::Relaxed);
        let done = self.done_bytes.load(Ordering::Relaxed).min(total);
        let eta = if speed > 1.0 && total > done {
            Some(((total - done) as f64 / speed).ceil() as u64)
        } else {
            None
        };
        let (map, blocks_total, blocks_done) = self.block_map(240);
        let items = self.items.lock().unwrap();
        let p = self.progress.lock().unwrap();
        let mut current = None;
        let files: Vec<Value> = items
            .iter()
            .filter(|i| !i.dir)
            .take(UI_FILE_LIMIT)
            .map(|i| {
                let fp = p.files.get(i.index as usize);
                let complete = fp.is_some_and(|f| f.complete());
                if current.is_none() && !complete {
                    current = Some(i.path.clone());
                }
                json!({
                    "path": i.path,
                    "size": i.size,
                    "done": complete,
                    "failed": fp.is_some_and(|f| f.failed),
                })
            })
            .collect();
        let file_count = items.iter().filter(|i| !i.dir).count();
        drop(p);
        drop(items);
        json!({
            "id": self.id,
            "direction": self.direction,
            "peer": *self.peer.lock().unwrap(),
            "state": state,
            "error": *self.error.lock().unwrap(),
            "sas": *self.sas.lock().unwrap(),
            "cipher": self.cipher.lock().unwrap().map(CipherSuite::label),
            "total_bytes": total,
            "done_bytes": done,
            "speed": speed,
            "eta": eta,
            "files": files,
            "file_count": file_count,
            "current_file": current,
            "block_map": map,
            "blocks_total": blocks_total,
            "blocks_done": blocks_done,
            "streams": self.streams.load(Ordering::Relaxed),
            "paused": paused,
            "local_paused": self.local_paused.load(Ordering::Relaxed),
            "saved_paths": *self.saved_paths.lock().unwrap(),
            "dest_dir": *self.dest_dir.lock().unwrap(),
            "text": *self.text.lock().unwrap(),
            "started_ms": self.started_ms,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------------------------

pub struct UserDecision {
    pub accept: bool,
    pub trust: bool,
}

struct DataSession {
    keys: SessionKeys,
    incoming: Arc<Incoming>,
}

#[derive(Default)]
pub struct TransferManager {
    transfers: Mutex<HashMap<String, Arc<Transfer>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<UserDecision>>>,
    data_sessions: Mutex<HashMap<[u8; 16], Arc<DataSession>>>,
    incoming: Mutex<HashMap<String, Arc<Incoming>>>,
}

impl TransferManager {
    fn insert(&self, t: Arc<Transfer>) {
        self.transfers.lock().unwrap().insert(t.id.clone(), t);
    }

    pub fn get(&self, id: &str) -> Option<Arc<Transfer>> {
        self.transfers.lock().unwrap().get(id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Transfer>> {
        self.transfers.lock().unwrap().values().cloned().collect()
    }

    pub fn remove_finished(&self) {
        self.transfers
            .lock()
            .unwrap()
            .retain(|_, t| !t.state().is_terminal());
    }

    pub fn respond(&self, request_id: &str, decision: UserDecision) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    pub fn pause(&self, id: &str, paused: bool) -> bool {
        match self.get(id) {
            Some(t) => {
                t.set_local_paused(paused);
                true
            }
            None => false,
        }
    }

    pub fn cancel(&self, id: &str) -> bool {
        match self.get(id) {
            Some(t) => {
                t.cancel_local();
                if matches!(t.state(), TState::Interrupted) {
                    if let Some(inc) = self.incoming.lock().unwrap().remove(&t.id) {
                        inc.discard_partials();
                    }
                    t.set_state(TState::Cancelled);
                }
                true
            }
            None => false,
        }
    }
}

/// Emits transfer snapshots to the UI at ~5 Hz while something changes.
pub async fn transfer_publisher(core: Arc<Core>) {
    let mut last_emitted: HashMap<String, TState> = HashMap::new();
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        for t in core.transfers.all() {
            let state = t.state();
            let active = !state.is_terminal();
            let dirty = t.dirty.swap(false, Ordering::Relaxed);
            let state_changed = last_emitted.get(&t.id) != Some(&state);
            if dirty || state_changed || (active && state == TState::Transferring) {
                events::emit("transfer", t.snapshot());
                last_emitted.insert(t.id.clone(), state);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sender
// ---------------------------------------------------------------------------------------------

struct SendPlan {
    items: Vec<OfferItem>,
    sources: Vec<Option<PathBuf>>,
    thumbs: HashMap<u32, String>,
    text: Option<String>,
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn build_plan(
    paths: Vec<PathBuf>,
    thumbs_by_path: HashMap<String, String>,
    text: Option<String>,
) -> io::Result<SendPlan> {
    let mut items = Vec::new();
    let mut sources = Vec::new();
    let mut thumbs = HashMap::new();
    let mut seen_names: HashMap<String, u32> = HashMap::new();
    for path in paths {
        let meta = fs::metadata(&path)
            .map_err(|e| io_err(e.kind(), format!("{}: {e}", path.display())))?;
        let base_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        // Two selected entries with the same name would collide on the receiver.
        let count = seen_names.entry(base_name.clone()).or_insert(0);
        *count += 1;
        let top = if *count == 1 {
            base_name
        } else {
            format!("{base_name} ({})", *count - 1)
        };
        if meta.is_dir() {
            for entry in walkdir::WalkDir::new(&path)
                .follow_links(false)
                .sort_by_file_name()
            {
                let entry = entry.map_err(|e| io::Error::other(e.to_string()))?;
                let rel = entry
                    .path()
                    .strip_prefix(&path)
                    .unwrap_or(entry.path())
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                let rel = if rel.is_empty() {
                    top.clone()
                } else {
                    format!("{top}/{rel}")
                };
                let ft = entry.file_type();
                if ft.is_dir() {
                    items.push(OfferItem {
                        index: items.len() as u32,
                        path: rel,
                        size: 0,
                        mtime: 0,
                        dir: true,
                    });
                    sources.push(None);
                } else if ft.is_file() {
                    let m = entry
                        .metadata()
                        .map_err(|e| io::Error::other(e.to_string()))?;
                    items.push(OfferItem {
                        index: items.len() as u32,
                        path: rel,
                        size: m.len(),
                        mtime: mtime_of(&m),
                        dir: false,
                    });
                    sources.push(Some(entry.path().to_path_buf()));
                }
            }
        } else {
            let index = items.len() as u32;
            if let Some(t) = thumbs_by_path.get(&path.to_string_lossy().to_string()) {
                thumbs.insert(index, t.clone());
            }
            items.push(OfferItem {
                index,
                path: top,
                size: meta.len(),
                mtime: mtime_of(&meta),
                dir: false,
            });
            sources.push(Some(path));
        }
    }
    Ok(SendPlan {
        items,
        sources,
        thumbs,
        text,
    })
}

pub fn start_send(
    core: &Arc<Core>,
    peer_id: String,
    paths: Vec<PathBuf>,
    text: Option<String>,
    thumbs: HashMap<String, String>,
) -> io::Result<String> {
    let info = core
        .registry
        .info(&peer_id)
        .ok_or_else(|| io_err(io::ErrorKind::NotFound, "unknown device"))?;
    if paths.is_empty() && text.as_deref().is_none_or(str::is_empty) {
        return Err(io_err(io::ErrorKind::InvalidInput, "nothing to send"));
    }
    let id = util::random_hex(8);
    let t = Transfer::new(
        id.clone(),
        Direction::Send,
        PeerRef {
            id: info.id.clone(),
            name: info.name.clone(),
            os: info.os,
        },
    );
    *t.text.lock().unwrap() = text.clone();
    core.transfers.insert(t.clone());
    let core = core.clone();
    tokio::spawn(async move {
        let plan = match tokio::task::spawn_blocking(move || build_plan(paths, thumbs, text)).await
        {
            Ok(Ok(plan)) => plan,
            Ok(Err(e)) => return t.fail(e.to_string()),
            Err(e) => return t.fail(e.to_string()),
        };
        t.set_items(plan.items.clone());
        run_sender(core, t, Arc::new(plan)).await;
    });
    Ok(id)
}

enum Outcome {
    Completed,
    Rejected(Option<String>),
    Cancelled(Option<String>),
}

async fn run_sender(core: Arc<Core>, t: Arc<Transfer>, plan: Arc<SendPlan>) {
    let mut attempts = 0u32;
    let mut last_good = Instant::now();
    loop {
        if t.is_cancelled() {
            t.set_state(TState::Cancelled);
            return;
        }
        t.set_state(if attempts == 0 {
            TState::Connecting
        } else {
            TState::Reconnecting
        });
        let resume = t.accepted.load(Ordering::Relaxed);
        let result = sender_session(&core, &t, &plan, resume).await;
        t.streams.store(0, Ordering::Relaxed);
        match result {
            Ok(Outcome::Completed) => {
                let failed = t
                    .progress
                    .lock()
                    .unwrap()
                    .files
                    .iter()
                    .filter(|f| f.failed)
                    .count();
                if failed > 0 {
                    t.fail(format!("{failed} file(s) failed the integrity check"));
                } else {
                    t.set_state(TState::Completed);
                }
                return;
            }
            Ok(Outcome::Rejected(reason)) => {
                *t.error.lock().unwrap() = reason;
                t.set_state(TState::Rejected);
                return;
            }
            Ok(Outcome::Cancelled(reason)) => {
                *t.error.lock().unwrap() = reason;
                t.set_state(TState::Cancelled);
                return;
            }
            Err(e) => {
                if !t.accepted.load(Ordering::Relaxed) {
                    t.fail(e.to_string());
                    return;
                }
                if t.done_bytes.load(Ordering::Relaxed) > 0 {
                    last_good = Instant::now();
                }
                attempts += 1;
                if attempts > MAX_RECONNECTS || last_good.elapsed() > RECONNECT_WINDOW {
                    t.fail(format!("connection lost: {e}"));
                    return;
                }
                *t.error.lock().unwrap() = Some(format!("connection lost: {e}"));
                t.set_state(TState::Reconnecting);
                let backoff = Duration::from_secs((2u64 << attempts.min(3)).min(10));
                let mut cancel = t.cancel_tx.subscribe();
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = cancel.changed() => {}
                }
            }
        }
    }
}

struct Scheduler {
    t: Arc<Transfer>,
    sources: Vec<Option<PathBuf>>,
    open: Mutex<HashMap<usize, Arc<File>>>,
    progressed: Notify,
}

impl Scheduler {
    fn next(&self, max_blocks: usize) -> Option<Chunk> {
        let mut p = self.t.progress.lock().unwrap();
        for (fi, f) in p.files.iter_mut().enumerate() {
            if f.dir || f.nblocks == 0 || f.complete() {
                continue;
            }
            let mut b = f.hint;
            while b < f.nblocks && (f.done.get(b) || f.inflight.get(b)) {
                b += 1;
            }
            if b >= f.nblocks {
                continue;
            }
            f.hint = b;
            let mut n = 0;
            while n < max_blocks
                && b + n < f.nblocks
                && !f.done.get(b + n)
                && !f.inflight.get(b + n)
            {
                f.inflight.set(b + n);
                n += 1;
            }
            let (start, _) = block_range(f.size, b);
            let (_, end) = block_range(f.size, b + n - 1);
            return Some(Chunk {
                file: fi,
                first_block: b,
                nblocks: n,
                offset: start,
                len: end - start,
            });
        }
        None
    }

    fn requeue(&self, c: &Chunk) {
        let mut p = self.t.progress.lock().unwrap();
        let f = &mut p.files[c.file];
        for b in c.first_block..c.first_block + c.nblocks {
            f.inflight.clear(b);
        }
        f.hint = f.hint.min(c.first_block);
    }

    fn complete(&self, c: &Chunk, hashes: &[Hash]) {
        let mut p = self.t.progress.lock().unwrap();
        let f = &mut p.files[c.file];
        for (i, b) in (c.first_block..c.first_block + c.nblocks).enumerate() {
            f.inflight.clear(b);
            f.done.set(b);
            f.hashes[b] = hashes[i];
        }
        let file_complete = f.complete();
        drop(p);
        if file_complete {
            self.open.lock().unwrap().remove(&c.file);
        }
        self.t.add_done(c.len);
        self.progressed.notify_one();
    }

    fn file(&self, index: usize) -> io::Result<Arc<File>> {
        let mut open = self.open.lock().unwrap();
        if let Some(f) = open.get(&index) {
            return Ok(f.clone());
        }
        let path = self.sources[index]
            .as_ref()
            .ok_or_else(|| io_err(io::ErrorKind::InvalidInput, "not a file"))?;
        let f = Arc::new(File::open(path)?);
        open.insert(index, f.clone());
        Ok(f)
    }

    fn pending_chunks(&self) -> bool {
        let p = self.t.progress.lock().unwrap();
        p.files.iter().any(|f| !f.dir && !f.complete())
    }
}

/// Re-hashes the blocks the receiver claims to have and keeps the matching ones.
fn apply_resume(sched: &Scheduler, resume: &[ResumeInfo]) -> io::Result<()> {
    {
        let mut p = sched.t.progress.lock().unwrap();
        for f in p.files.iter_mut() {
            f.done.clear_all();
            f.inflight.clear_all();
            f.hint = 0;
            f.closed = false;
        }
    }
    for info in resume {
        let index = info.index as usize;
        let (size, nblocks) = {
            let p = sched.t.progress.lock().unwrap();
            match p.files.get(index) {
                Some(f) if !f.dir => (f.size, f.nblocks),
                _ => continue,
            }
        };
        let bitmap = BitSet::from_bytes(&B64.decode(&info.bitmap).unwrap_or_default(), nblocks);
        let hashes = B64.decode(&info.hashes).unwrap_or_default();
        let file = sched.file(index)?;
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        let mut k = 0usize;
        let mut verified: Vec<(usize, Hash)> = Vec::new();
        for b in 0..nblocks {
            if !bitmap.get(b) {
                continue;
            }
            let claimed = hashes.get(k * HASH_LEN..(k + 1) * HASH_LEN);
            k += 1;
            let Some(claimed) = claimed else { break };
            let (s, e) = block_range(size, b);
            let slice = &mut buf[..(e - s) as usize];
            crate::diskio::pread_exact(&file, slice, s)?;
            let h = sha256(slice);
            if h.as_slice() == claimed {
                verified.push((b, h));
            }
        }
        let mut p = sched.t.progress.lock().unwrap();
        let f = &mut p.files[index];
        for (b, h) in verified {
            f.done.set(b);
            f.hashes[b] = h;
        }
    }
    sched.t.recompute_done();
    Ok(())
}

/// Tries every known address of `peer_id` until one completes the Noise handshake with the
/// expected identity (an address may now belong to another device, e.g. after a DHCP change).
async fn connect_authenticated(
    core: &Arc<Core>,
    peer_id: &str,
) -> io::Result<(TcpStream, SocketAddrV4, security::Established)> {
    let addrs = core.registry.addresses(peer_id);
    if addrs.is_empty() {
        return Err(io_err(
            io::ErrorKind::NotFound,
            "device has no reachable IP address (connect via Wi-Fi Direct or the same network)",
        ));
    }
    let mut last_err = None;
    for addr in addrs {
        let attempt = async {
            let mut stream = networking::connect_tuned(addr, Duration::from_secs(4)).await?;
            stream.write_all(MAGIC_CTRL).await?;
            let est = tokio::time::timeout(
                Duration::from_secs(15),
                security::handshake(&mut stream, &core.identity, &core.local_hello(), true),
            )
            .await
            .map_err(|_| io_err(io::ErrorKind::TimedOut, "handshake timed out"))??;
            if est.peer_id != peer_id {
                return Err(io_err(
                    io::ErrorKind::PermissionDenied,
                    "the device at this address has a different identity key",
                ));
            }
            Ok((stream, est))
        };
        match attempt.await {
            Ok((stream, est)) => return Ok((stream, addr, est)),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io_err(io::ErrorKind::NotFound, "no address")))
}

async fn sender_session(
    core: &Arc<Core>,
    t: &Arc<Transfer>,
    plan: &Arc<SendPlan>,
    resume: bool,
) -> io::Result<Outcome> {
    let peer_id = t.peer.lock().unwrap().id.clone();
    let (stream, addr, est) = connect_authenticated(core, &peer_id).await?;
    {
        let mut p = t.peer.lock().unwrap();
        p.name = est.peer.name.clone();
        p.os = est.peer.os;
    }
    *t.sas.lock().unwrap() = Some(est.sas.clone());
    *t.cipher.lock().unwrap() = Some(est.keys.suite);
    t.touch();

    let (rd, wr) = stream.into_split();
    let mut writer = RecordWriter::new(wr, est.keys.ctrl_send());
    let reader = RecordReader::new(rd, est.keys.ctrl_recv());
    let total_size: u64 = plan.items.iter().filter(|i| !i.dir).map(|i| i.size).sum();
    let streams = core.settings.read().unwrap().streams.clamp(1, 8);
    writer
        .send_json(&Ctrl::Offer {
            transfer_id: t.id.clone(),
            items: plan.items.clone(),
            text: plan.text.clone(),
            thumbs: if resume {
                HashMap::new()
            } else {
                plan.thumbs.clone()
            },
            total_size,
            block_size: BLOCK_SIZE,
            streams,
            resume,
        })
        .await?;
    if !resume {
        t.set_state(TState::WaitingAccept);
    }
    let (mut ctrl, _reader) = spawn_ctrl_reader(reader);

    let mut cancel = t.cancel_tx.subscribe();
    let wait = if resume {
        Duration::from_secs(30)
    } else {
        DECISION_TIMEOUT + Duration::from_secs(10)
    };
    let decision = tokio::select! {
        r = tokio::time::timeout(wait, next_ctrl(&mut ctrl)) => {
            r.map_err(|_| io_err(io::ErrorKind::TimedOut, "no answer from the receiver"))??
        }
        _ = cancel.changed() => {
            let _ = writer.send_json(&Ctrl::Cancel { reason: "cancelled by sender".into() }).await;
            return Ok(Outcome::Cancelled(None));
        }
    };
    let (session_id, resume_info) = match decision {
        Ctrl::Decision {
            accepted: true,
            session_id,
            resume,
            ..
        } => (session_id, resume),
        Ctrl::Decision {
            accepted: false,
            reason,
            ..
        } => return Ok(Outcome::Rejected(reason)),
        Ctrl::Cancel { reason } => return Ok(Outcome::Cancelled(Some(reason))),
        _ => return Err(io_err(io::ErrorKind::InvalidData, "unexpected message")),
    };
    t.accepted.store(true, Ordering::Relaxed);
    *t.error.lock().unwrap() = None;

    let file_items = plan.items.iter().any(|i| !i.dir);
    if !file_items {
        // Text / empty folders only: wait for the receiver to confirm.
        loop {
            match tokio::time::timeout(Duration::from_secs(30), next_ctrl(&mut ctrl)).await {
                Ok(Ok(Ctrl::Complete)) => return Ok(Outcome::Completed),
                Ok(Ok(Ctrl::Cancel { reason })) => return Ok(Outcome::Cancelled(Some(reason))),
                Ok(Ok(_)) => continue,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io_err(io::ErrorKind::TimedOut, "receiver did not confirm")),
            }
        }
    }

    let sid: [u8; 16] = hex::decode(&session_id)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| io_err(io::ErrorKind::InvalidData, "bad session id"))?;

    t.set_state(TState::Verifying);
    let sched = Arc::new(Scheduler {
        t: t.clone(),
        sources: plan.sources.clone(),
        open: Mutex::new(HashMap::new()),
        progressed: Notify::new(),
    });
    {
        let sched = sched.clone();
        tokio::task::spawn_blocking(move || apply_resume(&sched, &resume_info))
            .await
            .map_err(io::Error::other)??;
    }
    t.set_state(TState::Transferring);

    // Parallel data streams.
    let (fail_tx, mut fail_rx) = tokio::sync::mpsc::unbounded_channel::<io::Error>();
    let mut handles = Vec::new();
    for s in 0..streams {
        let (core, t, sched, fail_tx) = (core.clone(), t.clone(), sched.clone(), fail_tx.clone());
        let aead = est.keys.data(s);
        handles.push(tokio::spawn(async move {
            t.streams.fetch_add(1, Ordering::Relaxed);
            let r = data_stream_sender(&core, &t, &sched, addr, sid, s, aead).await;
            t.streams.fetch_sub(1, Ordering::Relaxed);
            if let Err(e) = r {
                let _ = fail_tx.send(e);
            }
        }));
    }
    drop(fail_tx);
    let _abort = AbortAll(handles);

    let mut paused_rx = t.paused_tx.subscribe();
    let mut local_paused_sent = false;
    loop {
        // Announce files whose blocks are all sent.
        let to_announce: Vec<(u32, Hash)> = {
            let mut p = t.progress.lock().unwrap();
            p.files
                .iter_mut()
                .enumerate()
                .filter(|(_, f)| !f.dir && !f.closed && f.complete())
                .map(|(i, f)| {
                    f.closed = true;
                    (i as u32, root_of(&f.hashes))
                })
                .collect()
        };
        for (index, root) in to_announce {
            writer
                .send_json(&Ctrl::FileDone {
                    index,
                    root: hex::encode(root),
                })
                .await?;
        }
        if !sched.pending_chunks() && t.state() == TState::Transferring {
            t.set_state(TState::Finalizing);
        }
        tokio::select! {
            msg = next_ctrl(&mut ctrl) => match msg? {
                Ctrl::Progress { .. } | Ctrl::Ping => {}
                Ctrl::FileResult { index, ok, error } => {
                    if !ok {
                        if let Some(f) = t.progress.lock().unwrap().files.get_mut(index as usize) {
                            f.failed = true;
                        }
                        events::log("warn", format!("file {index} rejected by receiver: {}", error.unwrap_or_default()));
                        t.touch();
                    }
                }
                Ctrl::Pause => t.set_remote_paused(true),
                Ctrl::Resume => t.set_remote_paused(false),
                Ctrl::Cancel { reason } => return Ok(Outcome::Cancelled(Some(reason))),
                Ctrl::Complete => return Ok(Outcome::Completed),
                _ => {}
            },
            _ = sched.progressed.notified() => {}
            r = paused_rx.changed() => {
                if r.is_ok() {
                    let local = t.local_paused.load(Ordering::Relaxed);
                    if local != local_paused_sent {
                        local_paused_sent = local;
                        writer.send_json(&if local { Ctrl::Pause } else { Ctrl::Resume }).await?;
                    }
                }
            }
            _ = cancel.changed() => {
                let _ = writer.send_json(&Ctrl::Cancel { reason: "cancelled by sender".into() }).await;
                return Ok(Outcome::Cancelled(None));
            }
            Some(e) = fail_rx.recv() => return Err(e),
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                writer.send_json(&Ctrl::Ping).await?;
            }
        }
    }
}

struct AbortAll(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortAll {
    fn drop(&mut self) {
        for h in &self.0 {
            h.abort();
        }
    }
}

type CtrlRx = tokio::sync::mpsc::Receiver<io::Result<Ctrl>>;

/// Reads control records on a dedicated task so that the session loops can `select!` on a
/// cancellation-safe channel instead of a partially-read socket.
fn spawn_ctrl_reader(mut reader: RecordReader<OwnedReadHalf>) -> (CtrlRx, AbortAll) {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let handle = tokio::spawn(async move {
        loop {
            let msg = reader.recv_json::<Ctrl>().await;
            let stop = msg.is_err();
            if tx.send(msg).await.is_err() || stop {
                break;
            }
        }
    });
    (rx, AbortAll(vec![handle]))
}

async fn next_ctrl(rx: &mut CtrlRx) -> io::Result<Ctrl> {
    match rx.recv().await {
        Some(msg) => msg,
        None => Err(io_err(
            io::ErrorKind::UnexpectedEof,
            "control channel closed",
        )),
    }
}

fn data_header(
    kind: &[u8; 4],
    file: u32,
    offset: u64,
    len: u32,
    nblocks: u16,
) -> [u8; DATA_HDR_LEN] {
    let mut h = [0u8; DATA_HDR_LEN];
    h[0..4].copy_from_slice(kind);
    h[4..8].copy_from_slice(&file.to_be_bytes());
    h[8..16].copy_from_slice(&offset.to_be_bytes());
    h[16..20].copy_from_slice(&len.to_be_bytes());
    h[20..22].copy_from_slice(&nblocks.to_be_bytes());
    h
}

async fn data_stream_sender(
    core: &Arc<Core>,
    t: &Arc<Transfer>,
    sched: &Arc<Scheduler>,
    addr: SocketAddrV4,
    session_id: [u8; 16],
    index: u16,
    aead: Aead,
) -> io::Result<()> {
    let mut stream = networking::connect_tuned(addr, Duration::from_secs(5)).await?;
    let mut hello = Vec::with_capacity(26);
    hello.extend_from_slice(MAGIC_DATA);
    hello.extend_from_slice(&session_id);
    hello.extend_from_slice(&index.to_be_bytes());
    stream.write_all(&hello).await?;

    let mut counter: u64 = 0;
    let mut chunk_blocks: usize = 2;
    let mut buf: Vec<u8> = Vec::with_capacity(
        DATA_HDR_LEN + MAX_CHUNK_BLOCKS as usize * (HASH_LEN + BLOCK_SIZE as usize) + TAG_LEN,
    );
    loop {
        t.wait_unpaused().await;
        if t.is_cancelled() {
            return Ok(());
        }
        let Some(chunk) = sched.next(chunk_blocks) else {
            break;
        };
        let started = Instant::now();
        let hashes_len = chunk.nblocks * HASH_LEN;
        let body_start = DATA_HDR_LEN + hashes_len;
        let body_end = body_start + chunk.len as usize;
        buf.resize(body_end + TAG_LEN, 0);
        let file = match sched.file(chunk.file) {
            Ok(f) => f,
            Err(e) => {
                sched.requeue(&chunk);
                return Err(e);
            }
        };
        buf = match core
            .disk
            .read_at(
                file,
                chunk.offset,
                std::mem::take(&mut buf),
                body_start..body_end,
            )
            .await
        {
            Ok(b) => b,
            Err(e) => {
                sched.requeue(&chunk);
                return Err(io_err(e.kind(), format!("read failed: {e}")));
            }
        };
        let header = data_header(
            FRAME_CHUNK,
            chunk.file as u32,
            chunk.offset,
            chunk.len as u32,
            chunk.nblocks as u16,
        );
        let nonce = nonce_for(DATA_NONCE_PREFIX, counter);
        let sealed = tokio::task::block_in_place(|| -> io::Result<Vec<Hash>> {
            let mut hashes = Vec::with_capacity(chunk.nblocks);
            for b in 0..chunk.nblocks {
                let s = body_start + b * BLOCK_SIZE as usize;
                let e = (s + BLOCK_SIZE as usize).min(body_end);
                hashes.push(sha256(&buf[s..e]));
            }
            for (b, h) in hashes.iter().enumerate() {
                buf[DATA_HDR_LEN + b * HASH_LEN..DATA_HDR_LEN + (b + 1) * HASH_LEN]
                    .copy_from_slice(h);
            }
            buf[..DATA_HDR_LEN].copy_from_slice(&header);
            let (hdr, rest) = buf.split_at_mut(DATA_HDR_LEN);
            let (body, tag_out) = rest.split_at_mut(body_end - DATA_HDR_LEN);
            let tag = aead.seal(&nonce, hdr, body)?;
            tag_out.copy_from_slice(&tag);
            Ok(hashes)
        });
        let hashes = match sealed {
            Ok(h) => h,
            Err(e) => {
                sched.requeue(&chunk);
                return Err(e);
            }
        };
        counter += 1;
        if let Err(e) = stream.write_all(&buf).await {
            sched.requeue(&chunk);
            return Err(e);
        }
        sched.complete(&chunk, &hashes);
        // Adaptive chunk size: aim for ~150 ms per frame, between 512 KiB and 4 MiB.
        let elapsed = started.elapsed();
        if elapsed < Duration::from_millis(75) && chunk_blocks < MAX_CHUNK_BLOCKS as usize {
            chunk_blocks *= 2;
        } else if elapsed > Duration::from_millis(300) && chunk_blocks > 1 {
            chunk_blocks /= 2;
        }
    }
    let header = data_header(FRAME_END, 0, 0, 0, 0);
    let tag = aead.seal(&nonce_for(DATA_NONCE_PREFIX, counter), &header, &mut [])?;
    let mut end = header.to_vec();
    end.extend_from_slice(&tag);
    stream.write_all(&end).await?;
    stream.shutdown().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Receiver
// ---------------------------------------------------------------------------------------------

struct RecvFile {
    item: OfferItem,
    dest: PathBuf,
    part: PathBuf,
    state: PathBuf,
    handle: Mutex<Option<Arc<File>>>,
    expected_root: Mutex<Option<Hash>>,
}

/// One accepted incoming transfer; survives reconnects of the sender.
pub struct Incoming {
    t: Arc<Transfer>,
    peer_static: [u8; 32],
    files: Vec<RecvFile>,
    file_completed: Notify,
    generation: watch::Sender<u64>,
    state_dirty: AtomicBool,
}

impl Incoming {
    fn handle(&self, index: usize) -> io::Result<Arc<File>> {
        let rf = &self.files[index];
        let mut h = rf.handle.lock().unwrap();
        if let Some(f) = h.as_ref() {
            return Ok(f.clone());
        }
        let f = Arc::new(OpenOptions::new().read(true).write(true).open(&rf.part)?);
        *h = Some(f.clone());
        Ok(f)
    }

    fn resume_info(&self) -> Vec<ResumeInfo> {
        let p = self.t.progress.lock().unwrap();
        p.files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.dir && f.done.count() > 0)
            .map(|(i, f)| {
                let mut hashes = Vec::with_capacity(f.done.count() * HASH_LEN);
                for b in 0..f.nblocks {
                    if f.done.get(b) {
                        hashes.extend_from_slice(&f.hashes[b]);
                    }
                }
                ResumeInfo {
                    index: i as u32,
                    bitmap: B64.encode(f.done.to_bytes()),
                    hashes: B64.encode(hashes),
                }
            })
            .collect()
    }

    fn persist_states(&self) {
        if !self.state_dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let p = self.t.progress.lock().unwrap();
        for (i, f) in p.files.iter().enumerate() {
            if f.dir || f.closed || f.nblocks == 0 {
                continue;
            }
            let rf = &self.files[i];
            let mut data = Vec::with_capacity(20 + f.nblocks.div_ceil(8) + f.nblocks * HASH_LEN);
            data.extend_from_slice(STATE_MAGIC);
            data.extend_from_slice(&f.size.to_le_bytes());
            data.extend_from_slice(&(f.nblocks as u32).to_le_bytes());
            data.extend_from_slice(&f.done.to_bytes());
            for h in &f.hashes {
                data.extend_from_slice(h);
            }
            let tmp = rf.state.with_extension("tmp");
            if fs::write(&tmp, &data).is_ok() {
                let _ = fs::rename(&tmp, &rf.state);
            }
        }
    }

    fn discard_partials(&self) {
        let p = self.t.progress.lock().unwrap();
        for (i, rf) in self.files.iter().enumerate() {
            if p.files.get(i).is_some_and(|f| f.closed) || rf.item.dir {
                continue;
            }
            *rf.handle.lock().unwrap() = None;
            let _ = fs::remove_file(&rf.part);
            let _ = fs::remove_file(&rf.state);
        }
    }

    /// Verifies and moves every complete file whose expected root is known into place.
    fn finalize_ready(&self) -> Vec<(u32, bool, Option<String>)> {
        let mut results = Vec::new();
        for (i, rf) in self.files.iter().enumerate() {
            if rf.item.dir {
                continue;
            }
            let (ready, computed) = {
                let p = self.t.progress.lock().unwrap();
                let f = &p.files[i];
                (
                    f.complete() && !f.closed,
                    if f.complete() {
                        Some(root_of(&f.hashes))
                    } else {
                        None
                    },
                )
            };
            if !ready {
                continue;
            }
            let Some(expected) = *rf.expected_root.lock().unwrap() else {
                continue;
            };
            let computed = computed.unwrap_or_default();
            let outcome = if computed != expected {
                Err("SHA-256 root mismatch".to_string())
            } else {
                self.move_into_place(rf).map_err(|e| e.to_string())
            };
            let mut p = self.t.progress.lock().unwrap();
            let f = &mut p.files[i];
            f.closed = true;
            match outcome {
                Ok(path) => {
                    drop(p);
                    self.t
                        .saved_paths
                        .lock()
                        .unwrap()
                        .push(path.to_string_lossy().to_string());
                    results.push((i as u32, true, None));
                }
                Err(e) => {
                    f.failed = true;
                    drop(p);
                    *rf.handle.lock().unwrap() = None;
                    let _ = fs::remove_file(&rf.part);
                    let _ = fs::remove_file(&rf.state);
                    results.push((i as u32, false, Some(e)));
                }
            }
            self.t.touch();
        }
        results
    }

    fn move_into_place(&self, rf: &RecvFile) -> io::Result<PathBuf> {
        if let Some(f) = rf.handle.lock().unwrap().take() {
            f.sync_all()?;
        }
        {
            let f = OpenOptions::new().write(true).open(&rf.part)?;
            if rf.item.mtime > 0 {
                let _ = f.set_modified(UNIX_EPOCH + Duration::from_millis(rf.item.mtime as u64));
            }
        }
        if let Some(parent) = rf.dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let dest = util::unique_path(&rf.dest);
        fs::rename(&rf.part, &dest)?;
        let _ = fs::remove_file(&rf.state);
        Ok(dest)
    }

    fn all_closed(&self) -> bool {
        let p = self.t.progress.lock().unwrap();
        p.files.iter().all(|f| f.dir || f.closed)
    }
}

fn file_key(peer_static: &[u8; 32], item: &OfferItem) -> String {
    let mut h = Sha256::new();
    h.update(peer_static);
    h.update([0]);
    h.update(item.path.as_bytes());
    h.update([0]);
    h.update(item.size.to_le_bytes());
    h.update(item.mtime.to_le_bytes());
    hex::encode(&h.finalize()[..16])
}

fn load_state(path: &Path, size: u64, nblocks: usize) -> Option<(BitSet, Vec<Hash>)> {
    let data = fs::read(path).ok()?;
    let bitmap_len = nblocks.div_ceil(8);
    if data.len() != 20 + bitmap_len + nblocks * HASH_LEN || &data[..8] != STATE_MAGIC {
        return None;
    }
    if u64::from_le_bytes(data[8..16].try_into().ok()?) != size
        || u32::from_le_bytes(data[16..20].try_into().ok()?) as usize != nblocks
    {
        return None;
    }
    let done = BitSet::from_bytes(&data[20..20 + bitmap_len], nblocks);
    let hashes = data[20 + bitmap_len..]
        .as_chunks::<HASH_LEN>()
        .0
        .to_vec();
    Some((done, hashes))
}

pub fn available_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is a valid out-pointer.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::unnecessary_cast)]
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::core::HSTRING;
        let mut free = 0u64;
        // SAFETY: valid wide string and out-pointer.
        unsafe {
            GetDiskFreeSpaceExW(
                &HSTRING::from(path.as_os_str()),
                Some(&mut free),
                None,
                None,
            )
        }
        .ok()?;
        Some(free)
    }
}

/// Opens/creates the partial files and recovers verified blocks from earlier attempts.
fn prepare_receive(
    t: &Arc<Transfer>,
    download_dir: &Path,
    peer_static: [u8; 32],
    items: &[OfferItem],
) -> io::Result<Incoming> {
    let partial_dir = download_dir.join(PARTIAL_DIR);
    fs::create_dir_all(&partial_dir)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        use windows::core::PCWSTR;
        let wide: Vec<u16> = partial_dir
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated wide string.
        let _ = unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN) };
    }
    let needed: u64 = items.iter().filter(|i| !i.dir).map(|i| i.size).sum();
    if let Some(free) = available_space(download_dir) {
        if free < needed {
            return Err(io_err(
                io::ErrorKind::StorageFull,
                format!("not enough free space ({} MB needed)", needed / 1_000_000),
            ));
        }
    }
    let mut files = Vec::with_capacity(items.len());
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    for item in items {
        let rel = util::sanitize_rel_path(&item.path).ok_or_else(|| {
            io_err(
                io::ErrorKind::InvalidData,
                format!("unsafe path {}", item.path),
            )
        })?;
        let dest = download_dir.join(&rel);
        if item.dir {
            fs::create_dir_all(&dest)?;
            files.push(RecvFile {
                item: item.clone(),
                dest,
                part: PathBuf::new(),
                state: PathBuf::new(),
                handle: Mutex::new(None),
                expected_root: Mutex::new(None),
            });
            continue;
        }
        let key = file_key(&peer_static, item);
        let part = partial_dir.join(format!("{key}.part"));
        let state = partial_dir.join(format!("{key}.state"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)?;
        let nblocks = blocks_for(item.size);
        if file.metadata()?.len() == item.size {
            if let Some((done, hashes)) = load_state(&state, item.size, nblocks) {
                let mut ok_blocks = Vec::new();
                for b in 0..nblocks {
                    if !done.get(b) {
                        continue;
                    }
                    let (s, e) = block_range(item.size, b);
                    let slice = &mut buf[..(e - s) as usize];
                    if crate::diskio::pread_exact(&file, slice, s).is_ok()
                        && sha256(slice) == hashes[b]
                    {
                        ok_blocks.push((b, hashes[b]));
                    }
                }
                let mut p = t.progress.lock().unwrap();
                let fp = &mut p.files[item.index as usize];
                for (b, h) in ok_blocks {
                    fp.done.set(b);
                    fp.hashes[b] = h;
                }
            }
        }
        file.set_len(item.size)?;
        drop(file);
        files.push(RecvFile {
            item: item.clone(),
            dest,
            part,
            state,
            handle: Mutex::new(None),
            expected_root: Mutex::new(None),
        });
    }
    t.recompute_done();
    Ok(Incoming {
        t: t.clone(),
        peer_static,
        files,
        file_completed: Notify::new(),
        generation: watch::channel(0).0,
        state_dirty: AtomicBool::new(true),
    })
}

pub async fn handle_incoming_control(
    core: Arc<Core>,
    mut stream: TcpStream,
    peer_addr: SocketAddr,
) -> io::Result<()> {
    let est = tokio::time::timeout(
        Duration::from_secs(15),
        security::handshake(&mut stream, &core.identity, &core.local_hello(), false),
    )
    .await
    .map_err(|_| io_err(io::ErrorKind::TimedOut, "handshake timed out"))??;
    let (rd, wr) = stream.into_split();
    let mut writer = RecordWriter::new(wr, est.keys.ctrl_send());
    let mut reader = RecordReader::new(rd, est.keys.ctrl_recv());
    let offer = tokio::time::timeout(Duration::from_secs(30), reader.recv_json::<Ctrl>())
        .await
        .map_err(|_| io_err(io::ErrorKind::TimedOut, "no offer"))??;
    let Ctrl::Offer {
        transfer_id,
        items,
        text,
        thumbs,
        total_size,
        block_size,
        resume,
        ..
    } = offer
    else {
        return Err(io_err(io::ErrorKind::InvalidData, "expected an offer"));
    };
    let (mut ctrl, reader_guard) = spawn_ctrl_reader(reader);
    if block_size != BLOCK_SIZE {
        writer
            .send_json(&Ctrl::Decision {
                accepted: false,
                reason: Some("incompatible block size".into()),
                session_id: String::new(),
                resume: vec![],
            })
            .await?;
        return Ok(());
    }
    if items
        .iter()
        .enumerate()
        .any(|(i, it)| it.index as usize != i)
        || transfer_id.len() > 64
    {
        return Err(io_err(io::ErrorKind::InvalidData, "malformed offer"));
    }
    if let SocketAddr::V4(v4) = peer_addr {
        core.registry.observe(
            networking::DeviceInfo {
                id: est.peer_id.clone(),
                name: est.peer.name.clone(),
                os: est.peer.os,
                port: est.peer.port,
                version: est.peer.version,
            },
            Some(SocketAddrV4::new(*v4.ip(), est.peer.port)),
            Transport::Incoming,
        );
    }

    // Automatic resume of an interrupted transfer from the same authenticated peer.
    if resume {
        let existing = core
            .transfers
            .incoming
            .lock()
            .unwrap()
            .get(&transfer_id)
            .cloned();
        match existing {
            Some(inc) if inc.peer_static == est.peer_static && !inc.t.state().is_terminal() => {
                return run_receiver(core, inc, est.keys, writer, ctrl, reader_guard, est.sas)
                    .await;
            }
            _ => {
                writer
                    .send_json(&Ctrl::Decision {
                        accepted: false,
                        reason: Some("transfer no longer available".into()),
                        session_id: String::new(),
                        resume: vec![],
                    })
                    .await?;
                return Ok(());
            }
        }
    }

    let peer = PeerRef {
        id: est.peer_id.clone(),
        name: est.peer.name.clone(),
        os: est.peer.os,
    };
    let t = Transfer::new(transfer_id.clone(), Direction::Receive, peer.clone());
    t.set_items(items.clone());
    *t.sas.lock().unwrap() = Some(est.sas.clone());
    *t.cipher.lock().unwrap() = Some(est.keys.suite);
    *t.text.lock().unwrap() = text.clone();
    let settings = core.settings.read().unwrap().clone();
    let download_dir = settings.download_path();
    *t.dest_dir.lock().unwrap() = download_dir.to_string_lossy().to_string();
    t.set_state(TState::WaitingAccept);

    let peer_key_hex = hex::encode(est.peer_static);
    let trusted = settings
        .trusted_key(&est.peer_id)
        .is_some_and(|tp| tp.public_key == peer_key_hex);
    let request_id = util::random_hex(8);
    let auto = trusted && settings.auto_accept_trusted;
    let ui_items: Vec<&OfferItem> = items.iter().take(UI_FILE_LIMIT).collect();
    let (tx, rx) = oneshot::channel();
    core.transfers
        .pending
        .lock()
        .unwrap()
        .insert(request_id.clone(), tx);
    if !auto {
        core.transfers.insert(t.clone());
    }
    events::emit(
        "incoming",
        json!({
            "request_id": request_id,
            "transfer_id": transfer_id,
            "peer": peer,
            "sas": est.sas,
            "cipher": est.keys.suite.label(),
            "items": ui_items,
            "item_count": items.len(),
            "file_count": items.iter().filter(|i| !i.dir).count(),
            "total_size": total_size,
            "text": text,
            "thumbs": thumbs,
            "trusted": trusted,
            "auto": auto,
        }),
    );
    let decision = if auto {
        core.transfers.pending.lock().unwrap().remove(&request_id);
        core.transfers.insert(t.clone());
        UserDecision {
            accept: true,
            trust: false,
        }
    } else {
        let waited = tokio::select! {
            r = tokio::time::timeout(DECISION_TIMEOUT, rx) => match r {
                Ok(Ok(d)) => Some(d),
                _ => None,
            },
            msg = next_ctrl(&mut ctrl) => {
                // Sender cancelled or disconnected while we were asking the user.
                let _ = msg;
                core.transfers.pending.lock().unwrap().remove(&request_id);
                events::emit("incoming_cancelled", json!({ "request_id": request_id }));
                t.set_state(TState::Cancelled);
                return Ok(());
            }
        };
        match waited {
            Some(d) => d,
            None => {
                core.transfers.pending.lock().unwrap().remove(&request_id);
                events::emit("incoming_cancelled", json!({ "request_id": request_id }));
                UserDecision {
                    accept: false,
                    trust: false,
                }
            }
        }
    };
    if !decision.accept {
        writer
            .send_json(&Ctrl::Decision {
                accepted: false,
                reason: Some("declined by the receiver".into()),
                session_id: String::new(),
                resume: vec![],
            })
            .await?;
        t.set_state(TState::Rejected);
        return Ok(());
    }
    if decision.trust {
        core.add_trusted(TrustedPeer {
            id: est.peer_id.clone(),
            name: est.peer.name.clone(),
            os: est.peer.os,
            public_key: peer_key_hex,
            added: util::now_ms(),
        });
    }
    t.accepted.store(true, Ordering::Relaxed);

    if items.iter().all(|i| i.dir) {
        let created: io::Result<()> = items.iter().try_for_each(|i| {
            let rel = util::sanitize_rel_path(&i.path)
                .ok_or_else(|| io_err(io::ErrorKind::InvalidData, "unsafe path"))?;
            fs::create_dir_all(download_dir.join(rel))
        });
        if let Some(text) = &text {
            events::emit(
                "text_received",
                json!({ "from": t.peer.lock().unwrap().clone(), "text": text, "transfer_id": t.id }),
            );
        }
        writer
            .send_json(&Ctrl::Decision {
                accepted: true,
                reason: None,
                session_id: util::random_hex(16),
                resume: vec![],
            })
            .await?;
        writer.send_json(&Ctrl::Complete).await?;
        match created {
            Ok(()) => t.set_state(TState::Completed),
            Err(e) => t.fail(e.to_string()),
        }
        return Ok(());
    }

    t.set_state(TState::Verifying);
    let prepared = {
        let t = t.clone();
        let items = items.clone();
        let dir = download_dir.clone();
        let peer_static = est.peer_static;
        tokio::task::spawn_blocking(move || prepare_receive(&t, &dir, peer_static, &items))
            .await
            .map_err(io::Error::other)?
    };
    let inc = match prepared {
        Ok(inc) => Arc::new(inc),
        Err(e) => {
            writer
                .send_json(&Ctrl::Decision {
                    accepted: false,
                    reason: Some(e.to_string()),
                    session_id: String::new(),
                    resume: vec![],
                })
                .await?;
            t.fail(e.to_string());
            return Ok(());
        }
    };
    if let Some(text) = &text {
        events::emit(
            "text_received",
            json!({ "from": t.peer.lock().unwrap().clone(), "text": text, "transfer_id": t.id }),
        );
    }
    core.transfers
        .incoming
        .lock()
        .unwrap()
        .insert(transfer_id.clone(), inc.clone());
    run_receiver(core, inc, est.keys, writer, ctrl, reader_guard, est.sas).await
}

async fn run_receiver(
    core: Arc<Core>,
    inc: Arc<Incoming>,
    keys: SessionKeys,
    mut writer: RecordWriter<OwnedWriteHalf>,
    mut ctrl: CtrlRx,
    _reader: AbortAll,
    sas: Sas,
) -> io::Result<()> {
    let t = inc.t.clone();
    let mut generation = 0;
    inc.generation.send_modify(|g| {
        *g += 1;
        generation = *g;
    });
    let mut gen_rx = inc.generation.subscribe();
    gen_rx.borrow_and_update();
    *t.sas.lock().unwrap() = Some(sas);
    *t.cipher.lock().unwrap() = Some(keys.suite);
    *t.error.lock().unwrap() = None;
    t.set_remote_paused(false);

    let sid: [u8; 16] = util::random_bytes();
    core.transfers.data_sessions.lock().unwrap().insert(
        sid,
        Arc::new(DataSession {
            keys,
            incoming: inc.clone(),
        }),
    );
    struct SessionGuard(Arc<Core>, [u8; 16]);
    impl Drop for SessionGuard {
        fn drop(&mut self) {
            self.0
                .transfers
                .data_sessions
                .lock()
                .unwrap()
                .remove(&self.1);
        }
    }
    let _guard = SessionGuard(core.clone(), sid);

    writer
        .send_json(&Ctrl::Decision {
            accepted: true,
            reason: None,
            session_id: hex::encode(sid),
            resume: inc.resume_info(),
        })
        .await?;
    t.set_state(TState::Transferring);
    if t.local_paused.load(Ordering::Relaxed) {
        writer.send_json(&Ctrl::Pause).await?;
    }

    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let mut persist = tokio::time::interval(Duration::from_secs(2));
    let mut cancel = t.cancel_tx.subscribe();
    let mut paused_rx = t.paused_tx.subscribe();
    let mut local_paused_sent = t.local_paused.load(Ordering::Relaxed);
    let result: io::Result<bool> = loop {
        let finalized = tokio::task::block_in_place(|| inc.finalize_ready());
        for (index, ok, error) in finalized {
            writer
                .send_json(&Ctrl::FileResult { index, ok, error })
                .await?;
        }
        if inc.all_closed() {
            writer.send_json(&Ctrl::Complete).await?;
            break Ok(true);
        }
        tokio::select! {
            msg = next_ctrl(&mut ctrl) => match msg {
                Ok(Ctrl::FileDone { index, root }) => {
                    if let (Some(rf), Ok(bytes)) = (inc.files.get(index as usize), hex::decode(&root)) {
                        if let Ok(h) = <[u8; 32]>::try_from(bytes.as_slice()) {
                            *rf.expected_root.lock().unwrap() = Some(h);
                        }
                    }
                    if inc.files.iter().all(|f| f.item.dir || f.expected_root.lock().unwrap().is_some()) {
                        t.set_state(TState::Finalizing);
                    }
                }
                Ok(Ctrl::Pause) => t.set_remote_paused(true),
                Ok(Ctrl::Resume) => t.set_remote_paused(false),
                Ok(Ctrl::Cancel { reason }) => {
                    inc.discard_partials();
                    *t.error.lock().unwrap() = Some(reason);
                    t.set_state(TState::Cancelled);
                    break Ok(false);
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            _ = inc.file_completed.notified() => {}
            _ = ticker.tick() => {
                writer.send_json(&Ctrl::Progress { bytes: t.done_bytes.load(Ordering::Relaxed) }).await?;
            }
            _ = persist.tick() => {
                let inc2 = inc.clone();
                let _ = tokio::task::spawn_blocking(move || inc2.persist_states()).await;
            }
            r = paused_rx.changed() => {
                if r.is_ok() {
                    let local = t.local_paused.load(Ordering::Relaxed);
                    if local != local_paused_sent {
                        local_paused_sent = local;
                        writer.send_json(&if local { Ctrl::Pause } else { Ctrl::Resume }).await?;
                    }
                }
            }
            _ = cancel.changed() => {
                let _ = writer.send_json(&Ctrl::Cancel { reason: "cancelled by receiver".into() }).await;
                inc.discard_partials();
                t.set_state(TState::Cancelled);
                break Ok(false);
            }
            r = gen_rx.changed() => {
                if r.is_err() || *gen_rx.borrow() != generation {
                    // A newer control connection (sender reconnect) took over.
                    return Ok(());
                }
            }
        }
    };
    match result {
        Ok(true) => {
            let failed = t
                .progress
                .lock()
                .unwrap()
                .files
                .iter()
                .filter(|f| f.failed)
                .count();
            if failed > 0 {
                t.fail(format!("{failed} file(s) failed the integrity check"));
            } else {
                t.set_state(TState::Completed);
            }
            core.transfers.incoming.lock().unwrap().remove(&t.id);
            Ok(())
        }
        Ok(false) => {
            core.transfers.incoming.lock().unwrap().remove(&t.id);
            Ok(())
        }
        Err(e) => {
            if *inc.generation.borrow() == generation {
                inc.state_dirty.store(true, Ordering::Relaxed);
                let inc2 = inc.clone();
                let _ = tokio::task::spawn_blocking(move || inc2.persist_states()).await;
                *t.error.lock().unwrap() = Some(format!(
                    "connection lost: {e} (waiting for the sender to resume)"
                ));
                t.set_state(TState::Interrupted);
                let core2 = core.clone();
                let inc2 = inc.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RECONNECT_WINDOW + Duration::from_secs(60)).await;
                    if *inc2.generation.borrow() == generation
                        && inc2.t.state() == TState::Interrupted
                    {
                        core2.transfers.incoming.lock().unwrap().remove(&inc2.t.id);
                        inc2.t.fail("the sender did not resume the transfer (partial data kept for a later retry)");
                    }
                });
            }
            Ok(())
        }
    }
}

pub async fn handle_data_stream(core: Arc<Core>, mut stream: TcpStream) -> io::Result<()> {
    let mut hello = [0u8; 18];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut hello))
        .await
        .map_err(|_| io_err(io::ErrorKind::TimedOut, "data hello timeout"))??;
    let sid: [u8; 16] = hello[..16].try_into().unwrap();
    let index = u16::from_be_bytes([hello[16], hello[17]]);
    let ds = core
        .transfers
        .data_sessions
        .lock()
        .unwrap()
        .get(&sid)
        .cloned()
        .ok_or_else(|| io_err(io::ErrorKind::NotFound, "unknown data session"))?;
    let aead = ds.keys.data(index);
    let inc = ds.incoming.clone();
    let t = inc.t.clone();
    t.streams.fetch_add(1, Ordering::Relaxed);
    struct StreamCount(Arc<Transfer>);
    impl Drop for StreamCount {
        fn drop(&mut self) {
            self.0.streams.fetch_sub(1, Ordering::Relaxed);
            self.0.touch();
        }
    }
    let _count = StreamCount(t.clone());

    let mut counter: u64 = 0;
    let mut hdr = [0u8; DATA_HDR_LEN];
    let mut buf: Vec<u8> = Vec::new();
    loop {
        t.wait_unpaused().await;
        if t.is_cancelled() {
            return Ok(());
        }
        stream.read_exact(&mut hdr).await?;
        let nonce = nonce_for(DATA_NONCE_PREFIX, counter);
        if &hdr[..4] == FRAME_END {
            let mut tag = [0u8; TAG_LEN];
            stream.read_exact(&mut tag).await?;
            aead.open(&nonce, &hdr, &mut [], &tag)?;
            return Ok(());
        }
        if &hdr[..4] != FRAME_CHUNK {
            return Err(io_err(io::ErrorKind::InvalidData, "bad frame type"));
        }
        let file_index = u32::from_be_bytes(hdr[4..8].try_into().unwrap()) as usize;
        let offset = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
        let len = u32::from_be_bytes(hdr[16..20].try_into().unwrap()) as u64;
        let nblocks = u16::from_be_bytes(hdr[20..22].try_into().unwrap()) as usize;
        let (size, dir) = {
            let p = t.progress.lock().unwrap();
            let f = p
                .files
                .get(file_index)
                .ok_or_else(|| io_err(io::ErrorKind::InvalidData, "bad file index"))?;
            (f.size, f.dir)
        };
        let valid = !dir
            && len > 0
            && offset.is_multiple_of(BLOCK_SIZE)
            && nblocks >= 1
            && nblocks <= MAX_CHUNK_BLOCKS as usize
            && nblocks == len.div_ceil(BLOCK_SIZE) as usize
            && offset + len <= size
            && (offset + len == size || len.is_multiple_of(BLOCK_SIZE));
        if !valid {
            return Err(io_err(io::ErrorKind::InvalidData, "invalid chunk header"));
        }
        let hashes_len = nblocks * HASH_LEN;
        let body_len = hashes_len + len as usize;
        buf.resize(body_len + TAG_LEN, 0);
        stream.read_exact(&mut buf).await?;
        let hashes = tokio::task::block_in_place(|| -> io::Result<Vec<Hash>> {
            let (body, tag) = buf.split_at_mut(body_len);
            aead.open(&nonce, &hdr, body, tag)?;
            let (claimed, data) = body.split_at(hashes_len);
            let mut hashes = Vec::with_capacity(nblocks);
            for b in 0..nblocks {
                let s = b * BLOCK_SIZE as usize;
                let e = (s + BLOCK_SIZE as usize).min(data.len());
                let h = sha256(&data[s..e]);
                if h.as_slice() != &claimed[b * HASH_LEN..(b + 1) * HASH_LEN] {
                    return Err(io_err(io::ErrorKind::InvalidData, "block SHA-256 mismatch"));
                }
                hashes.push(h);
            }
            Ok(hashes)
        })?;
        counter += 1;
        let file = inc.handle(file_index)?;
        buf = core
            .disk
            .write_at(file, offset, std::mem::take(&mut buf), hashes_len..body_len)
            .await?;
        let first_block = (offset / BLOCK_SIZE) as usize;
        let complete = {
            let mut p = t.progress.lock().unwrap();
            let f = &mut p.files[file_index];
            let mut added = 0u64;
            for (i, h) in hashes.iter().enumerate() {
                let b = first_block + i;
                if !f.done.get(b) {
                    f.done.set(b);
                    let (s, e) = block_range(f.size, b);
                    added += e - s;
                }
                f.hashes[b] = *h;
            }
            drop(p);
            t.add_done(added);
            let p = t.progress.lock().unwrap();
            p.files[file_index].complete()
        };
        inc.state_dirty.store(true, Ordering::Relaxed);
        if complete {
            inc.file_completed.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset_roundtrip() {
        let mut b = BitSet::new(130);
        for i in [0, 5, 63, 64, 129] {
            b.set(i);
        }
        let c = BitSet::from_bytes(&b.to_bytes(), 130);
        for i in 0..130 {
            assert_eq!(b.get(i), c.get(i));
        }
        assert_eq!(c.count(), 5);
    }

    #[test]
    fn block_math() {
        assert_eq!(blocks_for(0), 0);
        assert_eq!(blocks_for(1), 1);
        assert_eq!(blocks_for(BLOCK_SIZE), 1);
        assert_eq!(blocks_for(BLOCK_SIZE + 1), 2);
        assert_eq!(
            block_range(BLOCK_SIZE + 10, 1),
            (BLOCK_SIZE, BLOCK_SIZE + 10)
        );
    }

    #[test]
    fn scheduler_hands_out_dynamic_chunks() {
        let t = Transfer::new(
            "x".into(),
            Direction::Send,
            PeerRef {
                id: "p".into(),
                name: "p".into(),
                os: Os::Linux,
            },
        );
        t.set_items(vec![OfferItem {
            index: 0,
            path: "a".into(),
            size: BLOCK_SIZE * 10 + 5,
            mtime: 0,
            dir: false,
        }]);
        let s = Scheduler {
            t: t.clone(),
            sources: vec![None],
            open: Mutex::new(HashMap::new()),
            progressed: Notify::new(),
        };
        let a = s.next(8).unwrap();
        assert_eq!((a.first_block, a.nblocks, a.len), (0, 8, BLOCK_SIZE * 8));
        let b = s.next(8).unwrap();
        assert_eq!(
            (b.first_block, b.nblocks, b.len),
            (8, 3, BLOCK_SIZE * 2 + 5)
        );
        assert!(s.next(8).is_none());
        s.requeue(&a);
        let c = s.next(2).unwrap();
        assert_eq!((c.first_block, c.nblocks), (0, 2));
        s.complete(&c, &[[1u8; 32], [2u8; 32]]);
        assert_eq!(t.done_bytes.load(Ordering::Relaxed), BLOCK_SIZE * 2);
    }
}

#[cfg(test)]
mod e2e {
    use super::*;
    use crate::config::{Identity, Settings};
    use crate::diskio::DiskIo;
    use crate::networking::{DeviceInfo, Registry};
    use std::io::Write;

    fn make_core(dir: &Path, name: &str) -> Arc<Core> {
        let data = dir.join(format!("{name}-data"));
        let dl = dir.join(format!("{name}-downloads"));
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&dl).unwrap();
        let identity = Identity::load_or_create(&data).unwrap();
        let settings = Settings {
            device_name: name.into(),
            download_dir: dl.to_string_lossy().to_string(),
            streams: 3,
            auto_accept_trusted: true,
            ..Settings::default()
        };
        Arc::new(Core {
            registry: Registry::new(identity.id.clone()),
            identity,
            data_dir: data,
            settings: std::sync::RwLock::new(settings),
            transfers: TransferManager::default(),
            disk: DiskIo::new(),
            port: AtomicU16::new(0),
            host_handles_radio: false,
        })
    }

    async fn listen(core: &Arc<Core>) -> u16 {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(64).unwrap();
        let port = listener.local_addr().unwrap().port();
        core.port.store(port, Ordering::Relaxed);
        tokio::spawn(networking::accept_loop(core.clone(), listener));
        port
    }

    fn pair(a: &Arc<Core>, b: &Arc<Core>, b_port: u16) {
        a.registry.observe(
            DeviceInfo {
                id: b.identity.id.clone(),
                name: "B".into(),
                os: Os::Linux,
                port: b_port,
                version: 1,
            },
            Some(SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, b_port)),
            Transport::Manual,
        );
        // B trusts A and auto-accepts, so no UI is needed.
        b.settings.write().unwrap().trusted.push(TrustedPeer {
            id: a.identity.id.clone(),
            name: "A".into(),
            os: Os::Linux,
            public_key: hex::encode(a.identity.public),
            added: 0,
        });
    }

    fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    async fn wait_terminal(core: &Arc<Core>, id: &str) -> TState {
        for _ in 0..600 {
            if let Some(t) = core.transfers.get(id) {
                let s = t.state();
                if s.is_terminal() {
                    return s;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("transfer did not finish");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn transfers_files_and_folders_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let a = make_core(tmp.path(), "a");
        let b = make_core(tmp.path(), "b");
        let b_port = listen(&b).await;
        listen(&a).await;
        pair(&a, &b, b_port);

        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("album/sub")).unwrap();
        fs::create_dir_all(src.join("album/empty")).unwrap();
        let big = pseudo_random(5 * 1024 * 1024 + 12345, 1);
        fs::write(src.join("big.bin"), &big).unwrap();
        fs::write(src.join("album/a.txt"), b"hello").unwrap();
        fs::write(src.join("album/sub/zero.dat"), b"").unwrap();
        let mid = pseudo_random(BLOCK_SIZE as usize * 3, 2);
        fs::write(src.join("album/sub/mid.bin"), &mid).unwrap();

        let id = start_send(
            &a,
            b.identity.id.clone(),
            vec![src.join("big.bin"), src.join("album")],
            Some("note".into()),
            HashMap::new(),
        )
        .unwrap();
        assert_eq!(wait_terminal(&a, &id).await, TState::Completed);
        assert_eq!(wait_terminal(&b, &id).await, TState::Completed);

        let dl = b.settings.read().unwrap().download_path();
        assert_eq!(fs::read(dl.join("big.bin")).unwrap(), big);
        assert_eq!(fs::read(dl.join("album/a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(dl.join("album/sub/zero.dat")).unwrap(), b"");
        assert_eq!(fs::read(dl.join("album/sub/mid.bin")).unwrap(), mid);
        assert!(dl.join("album/empty").is_dir());
        let partial = dl.join(PARTIAL_DIR);
        assert_eq!(fs::read_dir(partial).unwrap().count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resumes_from_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let a = make_core(tmp.path(), "a");
        let b = make_core(tmp.path(), "b");
        let b_port = listen(&b).await;
        pair(&a, &b, b_port);

        let src = tmp.path().join("video.mkv");
        let data = pseudo_random(BLOCK_SIZE as usize * 6 + 777, 3);
        let mut f = File::create(&src).unwrap();
        f.write_all(&data).unwrap();
        drop(f);
        let meta = fs::metadata(&src).unwrap();
        let item = OfferItem {
            index: 0,
            path: "video.mkv".into(),
            size: meta.len(),
            mtime: mtime_of(&meta),
            dir: false,
        };

        // Simulate an earlier interrupted session: blocks 0, 1 and 4 are on disk, block 4 is
        // corrupted and must be re-sent; the rest of the partial file is garbage.
        let dl = b.settings.read().unwrap().download_path();
        let partial = dl.join(PARTIAL_DIR);
        fs::create_dir_all(&partial).unwrap();
        let key = file_key(&a.identity.public, &item);
        let mut part = vec![0xAAu8; data.len()];
        let bs = BLOCK_SIZE as usize;
        part[..2 * bs].copy_from_slice(&data[..2 * bs]);
        fs::write(partial.join(format!("{key}.part")), &part).unwrap();
        let nblocks = blocks_for(item.size);
        let mut done = BitSet::new(nblocks);
        let mut state = Vec::new();
        state.extend_from_slice(STATE_MAGIC);
        state.extend_from_slice(&item.size.to_le_bytes());
        state.extend_from_slice(&(nblocks as u32).to_le_bytes());
        for b in [0, 1, 4] {
            done.set(b);
        }
        state.extend_from_slice(&done.to_bytes());
        for b in 0..nblocks {
            let (s, e) = block_range(item.size, b);
            state.extend_from_slice(&sha256(&data[s as usize..e as usize]));
        }
        fs::write(partial.join(format!("{key}.state")), &state).unwrap();

        let id = start_send(&a, b.identity.id.clone(), vec![src.clone()], None, HashMap::new()).unwrap();
        assert_eq!(wait_terminal(&a, &id).await, TState::Completed);
        assert_eq!(wait_terminal(&b, &id).await, TState::Completed);
        assert_eq!(fs::read(dl.join("video.mkv")).unwrap(), data);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rejects_wrong_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let a = make_core(tmp.path(), "a");
        let b = make_core(tmp.path(), "b");
        let c = make_core(tmp.path(), "c");
        let c_port = listen(&c).await;
        // A believes B lives at C's address: the Noise handshake must expose the impostor.
        pair(&a, &b, c_port);
        let src = tmp.path().join("x.txt");
        fs::write(&src, b"secret").unwrap();
        let id = start_send(&a, b.identity.id.clone(), vec![src], None, HashMap::new()).unwrap();
        assert_eq!(wait_terminal(&a, &id).await, TState::Failed);
    }

    /// Throughput benchmark over loopback (crypto + hashing + disk, no network limit):
    /// `cargo test --release -- --ignored --nocapture loopback_throughput`
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn loopback_throughput() {
        let tmp = tempfile::tempdir().unwrap();
        let a = make_core(tmp.path(), "a");
        let b = make_core(tmp.path(), "b");
        a.settings.write().unwrap().streams = 4;
        let b_port = listen(&b).await;
        pair(&a, &b, b_port);
        let size: usize = std::env::var("OMNIDROP_BENCH_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(512)
            * 1024
            * 1024;
        let src = tmp.path().join("bench.bin");
        let chunk = pseudo_random(4 * 1024 * 1024, 9);
        let mut f = File::create(&src).unwrap();
        for _ in 0..size / chunk.len() {
            f.write_all(&chunk).unwrap();
        }
        f.sync_all().unwrap();
        drop(f);
        let started = Instant::now();
        let id = start_send(&a, b.identity.id.clone(), vec![src], None, HashMap::new()).unwrap();
        assert_eq!(wait_terminal(&b, &id).await, TState::Completed);
        let secs = started.elapsed().as_secs_f64();
        let cipher = a.transfers.get(&id).unwrap().cipher.lock().unwrap().map(CipherSuite::label);
        println!(
            "{} MiB in {:.2}s = {:.0} MB/s ({:?}, disk I/O: {})",
            size / 1024 / 1024,
            secs,
            size as f64 / secs / 1e6,
            cipher,
            b.disk.backend_name()
        );
    }
}

