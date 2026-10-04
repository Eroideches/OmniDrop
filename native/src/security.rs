//! Security layer.
//!
//! * Authenticated key exchange: `Noise_XX_25519_ChaChaPoly_SHA256` (mutual authentication of the
//!   long-term Curve25519 static keys, forward secrecy through ephemeral ECDHE).
//! * After the handshake both peers exchange a 32-byte random contribution over the Noise
//!   transport; `master = HKDF-SHA256(salt = handshake hash, ikm = c_initiator || c_responder)`.
//! * Bulk/control records use AES-256-GCM when *both* peers have hardware AES (AES-NI + PCLMULQDQ
//!   on x86, ARMv8 Crypto Extensions AES + PMULL on aarch64), otherwise ChaCha20-Poly1305.
//! * The Short Authentication String (6-digit PIN + 4 emoji, ~44 bits) is derived from the Noise
//!   handshake hash, which differs on the two legs of any man-in-the-middle attempt.

use crate::config::{Identity, device_id_for};
use crate::util::{self, Os};
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use snow::Builder;
use snow::params::NoiseParams;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
pub const PROLOGUE: &[u8] = b"OmniDrop/1";
pub const PROTOCOL_VERSION: u8 = 1;
pub const TAG_LEN: usize = 16;
/// Upper bound for one encrypted record (control or data), tag included.
pub const MAX_RECORD: usize = 16 * 1024 * 1024 + 64 * 1024;
const NOISE_MAX: usize = 65535;

/// 64 visually distinct, single-code-point emoji; index = 6 bits of SAS material.
pub const SAS_EMOJI: [&str; 64] = [
    "🐶", "🐱", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🐦", "🦉",
    "🦄", "🐝", "🦋", "🐢", "🐙", "🦀", "🐬", "🐳", "🦈", "🐘", "🦒", "🦓", "🐪", "🦔", "🌵", "🌲",
    "🌻", "🌹", "🍄", "🍁", "🌙", "⭐", "🔥", "🌈", "🎈", "⚡", "🍎", "🍋", "🍉", "🍇", "🍓", "🍒",
    "🍍", "🥕", "🌽", "🍕", "🍔", "🍩", "🎂", "☕", "🎸", "🎲", "⚽", "🚀", "🚲", "⚓", "🔑", "💡",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CipherSuite {
    #[serde(rename = "AES-256-GCM")]
    Aes256Gcm,
    #[serde(rename = "ChaCha20-Poly1305")]
    ChaCha20Poly1305,
}

impl CipherSuite {
    pub fn label(self) -> &'static str {
        match self {
            CipherSuite::Aes256Gcm => "AES-256-GCM",
            CipherSuite::ChaCha20Poly1305 => "ChaCha20-Poly1305",
        }
    }
}

/// True when the AES-GCM backend compiled into this binary runs on hardware instructions.
pub fn hardware_aes_available() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("aes")
            && std::arch::is_x86_feature_detected!("pclmulqdq")
    }
    #[cfg(target_arch = "aarch64")]
    {
        // The `aes`/`polyval` crates only use the ARMv8 Crypto Extensions when built with
        // `--cfg aes_armv8 --cfg polyval_armv8` (see native/.cargo/config.toml).
        cfg!(all(aes_armv8, polyval_armv8))
            && std::arch::is_aarch64_feature_detected!("aes")
            && std::arch::is_aarch64_feature_detected!("pmull")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        false
    }
}

pub fn generate_static_keypair() -> Result<([u8; 32], [u8; 32]), snow::Error> {
    let params: NoiseParams = NOISE_PARAMS.parse()?;
    let kp = Builder::new(params).generate_keypair()?;
    let mut private = [0u8; 32];
    let mut public = [0u8; 32];
    private.copy_from_slice(&kp.private);
    public.copy_from_slice(&kp.public);
    Ok((private, public))
}

/// AEAD with in-place, detached-tag operation (no allocation, no copy of the payload).
pub enum Aead {
    Aes(Box<Aes256Gcm>),
    ChaCha(Box<ChaCha20Poly1305>),
}

impl Aead {
    pub fn new(suite: CipherSuite, key: &[u8; 32]) -> Aead {
        let key = GenericArray::from_slice(key);
        match suite {
            CipherSuite::Aes256Gcm => Aead::Aes(Box::new(Aes256Gcm::new(key))),
            CipherSuite::ChaCha20Poly1305 => Aead::ChaCha(Box::new(ChaCha20Poly1305::new(key))),
        }
    }

    pub fn seal(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> io::Result<[u8; TAG_LEN]> {
        let nonce = GenericArray::from_slice(nonce);
        let tag = match self {
            Aead::Aes(c) => c.encrypt_in_place_detached(nonce, aad, buf),
            Aead::ChaCha(c) => c.encrypt_in_place_detached(nonce, aad, buf),
        }
        .map_err(|_| io::Error::other("encryption failed"))?;
        let mut out = [0u8; TAG_LEN];
        out.copy_from_slice(tag.as_slice());
        Ok(out)
    }

    pub fn open(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> io::Result<()> {
        if tag.len() != TAG_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad tag length"));
        }
        let nonce = GenericArray::from_slice(nonce);
        let tag = GenericArray::from_slice(tag);
        match self {
            Aead::Aes(c) => c.decrypt_in_place_detached(nonce, aad, buf, tag),
            Aead::ChaCha(c) => c.decrypt_in_place_detached(nonce, aad, buf, tag),
        }
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "authentication failed"))
    }
}

pub fn nonce_for(prefix: [u8; 4], counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&prefix);
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

/// Key material of one authenticated session.
#[derive(Clone)]
pub struct SessionKeys {
    pub suite: CipherSuite,
    master: [u8; 32],
    initiator: bool,
}

impl SessionKeys {
    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::from_prk(&self.master).expect("32-byte PRK is valid for SHA-256");
        let mut out = [0u8; 32];
        hk.expand(label, &mut out)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        out
    }

    pub fn ctrl_send(&self) -> Aead {
        let label: &[u8] = if self.initiator {
            b"ctrl i2r"
        } else {
            b"ctrl r2i"
        };
        Aead::new(self.suite, &self.derive(label))
    }

    pub fn ctrl_recv(&self) -> Aead {
        let label: &[u8] = if self.initiator {
            b"ctrl r2i"
        } else {
            b"ctrl i2r"
        };
        Aead::new(self.suite, &self.derive(label))
    }

    /// Key for data stream `index` (always sender -> receiver).
    pub fn data(&self, index: u16) -> Aead {
        Aead::new(self.suite, &self.derive(format!("data {index}").as_bytes()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sas {
    pub pin: String,
    pub emoji: Vec<String>,
}

pub fn compute_sas(handshake_hash: &[u8]) -> Sas {
    let hk = Hkdf::<Sha256>::new(Some(b"omnidrop sas v1"), handshake_hash);
    let mut out = [0u8; 8];
    hk.expand(b"sas", &mut out)
        .expect("8 bytes is a valid HKDF-SHA256 output length");
    let n = u64::from_be_bytes([0, 0, 0, out[0], out[1], out[2], out[3], out[4]]);
    let bits = u32::from_be_bytes([0, out[5], out[6], out[7]]);
    let emoji = (0..4)
        .map(|i| SAS_EMOJI[((bits >> (18 - 6 * i)) & 63) as usize].to_string())
        .collect();
    Sas {
        pin: format!("{:06}", n % 1_000_000),
        emoji,
    }
}

/// Public device description exchanged inside the encrypted channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub name: String,
    pub os: Os,
    pub port: u16,
    pub version: u8,
    pub aes_hw: bool,
    pub contribution: String,
}

pub struct LocalHello {
    pub name: String,
    pub port: u16,
}

pub struct Established {
    pub keys: SessionKeys,
    pub sas: Sas,
    pub peer: Hello,
    pub peer_static: [u8; 32],
    pub peer_id: String,
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(data.len() + 2);
    frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
    frame.extend_from_slice(data);
    w.write_all(&frame).await?;
    w.flush().await
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

fn noise_err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("noise: {e}"))
}

/// Runs the Noise_XX handshake plus the contribution exchange and derives the session keys.
pub async fn handshake<S>(
    stream: &mut S,
    identity: &Identity,
    local: &LocalHello,
    initiator: bool,
) -> io::Result<Established>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let params: NoiseParams = NOISE_PARAMS.parse().map_err(noise_err)?;
    let builder = Builder::new(params)
        .local_private_key(&identity.private)
        .map_err(noise_err)?
        .prologue(PROLOGUE)
        .map_err(noise_err)?;
    let mut hs = if initiator {
        builder.build_initiator().map_err(noise_err)?
    } else {
        builder.build_responder().map_err(noise_err)?
    };
    let mut out = vec![0u8; NOISE_MAX];
    let mut payload = vec![0u8; NOISE_MAX];

    if initiator {
        // -> e
        let n = hs.write_message(&[], &mut out).map_err(noise_err)?;
        write_frame(stream, &out[..n]).await?;
        // <- e, ee, s, es
        let msg = read_frame(stream).await?;
        hs.read_message(&msg, &mut payload).map_err(noise_err)?;
        // -> s, se
        let n = hs.write_message(&[], &mut out).map_err(noise_err)?;
        write_frame(stream, &out[..n]).await?;
    } else {
        let msg = read_frame(stream).await?;
        hs.read_message(&msg, &mut payload).map_err(noise_err)?;
        let n = hs.write_message(&[], &mut out).map_err(noise_err)?;
        write_frame(stream, &out[..n]).await?;
        let msg = read_frame(stream).await?;
        hs.read_message(&msg, &mut payload).map_err(noise_err)?;
    }

    let remote = hs
        .get_remote_static()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "peer static key missing"))?;
    let mut peer_static = [0u8; 32];
    peer_static.copy_from_slice(remote);
    let hash = hs.get_handshake_hash().to_vec();
    let mut transport = hs.into_transport_mode().map_err(noise_err)?;

    let contribution: [u8; 32] = util::random_bytes();
    let hello = Hello {
        name: local.name.clone(),
        os: Os::current(),
        port: local.port,
        version: PROTOCOL_VERSION,
        aes_hw: hardware_aes_available(),
        contribution: hex::encode(contribution),
    };
    let hello_bytes = serde_json::to_vec(&hello).map_err(io::Error::other)?;
    let n = transport
        .write_message(&hello_bytes, &mut out)
        .map_err(noise_err)?;
    write_frame(stream, &out[..n]).await?;
    let msg = read_frame(stream).await?;
    let n = transport
        .read_message(&msg, &mut payload)
        .map_err(noise_err)?;
    let peer: Hello = serde_json::from_slice(&payload[..n])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if peer.version != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported protocol version {}", peer.version),
        ));
    }
    let peer_contribution = hex::decode(&peer.contribution)
        .ok()
        .filter(|c| c.len() == 32)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad key contribution"))?;

    let mut ikm = Vec::with_capacity(64);
    if initiator {
        ikm.extend_from_slice(&contribution);
        ikm.extend_from_slice(&peer_contribution);
    } else {
        ikm.extend_from_slice(&peer_contribution);
        ikm.extend_from_slice(&contribution);
    }
    let hk = Hkdf::<Sha256>::new(Some(&hash), &ikm);
    let mut master = [0u8; 32];
    hk.expand(b"omnidrop/1 master", &mut master)
        .expect("32 bytes is a valid HKDF-SHA256 output length");

    let suite = if hello.aes_hw && peer.aes_hw {
        CipherSuite::Aes256Gcm
    } else {
        CipherSuite::ChaCha20Poly1305
    };

    Ok(Established {
        keys: SessionKeys {
            suite,
            master,
            initiator,
        },
        sas: compute_sas(&hash),
        peer_id: device_id_for(&peer_static),
        peer,
        peer_static,
    })
}

const CTRL_NONCE_PREFIX: [u8; 4] = *b"CTRL";

/// Writing half of an encrypted record stream: `[u32 len][ciphertext][tag]`, AAD = length.
pub struct RecordWriter<W> {
    inner: W,
    aead: Aead,
    counter: u64,
    scratch: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> RecordWriter<W> {
    pub fn new(inner: W, aead: Aead) -> Self {
        RecordWriter {
            inner,
            aead,
            counter: 0,
            scratch: Vec::new(),
        }
    }

    pub async fn send(&mut self, plaintext: &[u8]) -> io::Result<()> {
        let len = plaintext.len() + TAG_LEN;
        if len > MAX_RECORD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "record too large",
            ));
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&(len as u32).to_be_bytes());
        self.scratch.extend_from_slice(plaintext);
        let nonce = nonce_for(CTRL_NONCE_PREFIX, self.counter);
        let (hdr, body) = self.scratch.split_at_mut(4);
        let tag = self.aead.seal(&nonce, hdr, body)?;
        self.counter += 1;
        self.scratch.extend_from_slice(&tag);
        self.inner.write_all(&self.scratch).await?;
        self.inner.flush().await
    }

    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        self.send(&bytes).await
    }
}

pub struct RecordReader<R> {
    inner: R,
    aead: Aead,
    counter: u64,
}

impl<R: AsyncRead + Unpin> RecordReader<R> {
    pub fn new(inner: R, aead: Aead) -> Self {
        RecordReader {
            inner,
            aead,
            counter: 0,
        }
    }

    pub async fn recv(&mut self) -> io::Result<Vec<u8>> {
        let mut hdr = [0u8; 4];
        self.inner.read_exact(&mut hdr).await?;
        let len = u32::from_be_bytes(hdr) as usize;
        if !(TAG_LEN..=MAX_RECORD).contains(&len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad record length",
            ));
        }
        let mut buf = vec![0u8; len];
        self.inner.read_exact(&mut buf).await?;
        let nonce = nonce_for(CTRL_NONCE_PREFIX, self.counter);
        let split = len - TAG_LEN;
        let (body, tag) = buf.split_at_mut(split);
        self.aead.open(&nonce, &hdr, body, tag)?;
        self.counter += 1;
        buf.truncate(split);
        Ok(buf)
    }

    pub async fn recv_json<T: for<'de> Deserialize<'de>>(&mut self) -> io::Result<T> {
        let bytes = self.recv().await?;
        serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn identity() -> Identity {
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap()
    }

    #[test]
    fn emoji_table_is_unique() {
        let set: HashSet<_> = SAS_EMOJI.iter().collect();
        assert_eq!(set.len(), 64);
    }

    #[tokio::test]
    async fn handshake_agrees_on_sas_and_keys() {
        let (a, b) = (identity(), identity());
        let (mut s1, mut s2) = tokio::io::duplex(1 << 20);
        let ha = LocalHello {
            name: "A".into(),
            port: 1,
        };
        let hb = LocalHello {
            name: "B".into(),
            port: 2,
        };
        let (ra, rb) = tokio::join!(
            handshake(&mut s1, &a, &ha, true),
            handshake(&mut s2, &b, &hb, false)
        );
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        assert_eq!(ra.sas, rb.sas);
        assert_eq!(ra.peer_id, b.id);
        assert_eq!(rb.peer_id, a.id);
        assert_eq!(ra.peer.name, "B");

        let (r1, w1) = tokio::io::split(s1);
        let (r2, w2) = tokio::io::split(s2);
        let mut wa = RecordWriter::new(w1, ra.keys.ctrl_send());
        let mut rbr = RecordReader::new(r2, rb.keys.ctrl_recv());
        wa.send(b"hello over aead").await.unwrap();
        assert_eq!(rbr.recv().await.unwrap(), b"hello over aead");
        let mut wb = RecordWriter::new(w2, rb.keys.ctrl_send());
        let mut rar = RecordReader::new(r1, ra.keys.ctrl_recv());
        wb.send(b"reply").await.unwrap();
        assert_eq!(rar.recv().await.unwrap(), b"reply");
    }

    #[test]
    fn aead_detects_tampering() {
        for suite in [CipherSuite::Aes256Gcm, CipherSuite::ChaCha20Poly1305] {
            let aead = Aead::new(suite, &[7u8; 32]);
            let nonce = nonce_for(*b"TEST", 1);
            let mut data = b"payload".to_vec();
            let tag = aead.seal(&nonce, b"aad", &mut data).unwrap();
            let mut tampered = data.clone();
            tampered[0] ^= 1;
            assert!(aead.open(&nonce, b"aad", &mut tampered, &tag).is_err());
            aead.open(&nonce, b"aad", &mut data, &tag).unwrap();
            assert_eq!(data, b"payload");
        }
    }
}
