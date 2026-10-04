//! Positional disk I/O for the transfer pipeline.
//!
//! The pipeline is zero-copy in user space: a pooled buffer is filled by the kernel
//! (`read_at`), encrypted in place, sent with a single `write`, and on the other side
//! decrypted in place and handed back to the kernel (`write_at`) without any intermediate copy.
//!
//! * Linux: an `io_uring` submission thread (buffers are owned by the in-flight request until
//!   the completion is reaped, short reads/writes are resubmitted).
//! * Windows / Android / io_uring unavailable (old kernel, seccomp, `io_uring_disabled`):
//!   positional `pread`/`pwrite` (`seek_read`/`seek_write` with OVERLAPPED offsets on Windows)
//!   on Tokio's blocking pool.

use std::fs::File;
use std::io;
use std::ops::Range;
use std::sync::Arc;

pub struct DiskIo {
    backend: Backend,
}

enum Backend {
    #[cfg(target_os = "linux")]
    Uring(uring::UringHandle),
    Blocking,
}

impl DiskIo {
    pub fn new() -> DiskIo {
        #[cfg(target_os = "linux")]
        {
            match uring::UringHandle::start() {
                Ok(handle) => {
                    return DiskIo {
                        backend: Backend::Uring(handle),
                    };
                }
                Err(e) => crate::events::log(
                    "info",
                    format!("io_uring unavailable ({e}), using positional pread/pwrite"),
                ),
            }
        }
        DiskIo {
            backend: Backend::Blocking,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            #[cfg(target_os = "linux")]
            Backend::Uring(_) => "io_uring",
            Backend::Blocking => {
                if cfg!(windows) {
                    "overlapped positional I/O"
                } else {
                    "pread/pwrite"
                }
            }
        }
    }

    /// Fills `buf[range]` with file bytes starting at `offset`; returns the buffer.
    pub async fn read_at(
        &self,
        file: Arc<File>,
        offset: u64,
        buf: Vec<u8>,
        range: Range<usize>,
    ) -> io::Result<Vec<u8>> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Uring(h) => h.submit(uring::Op::Read, file, offset, buf, range).await,
            Backend::Blocking => tokio::task::spawn_blocking(move || {
                let mut buf = buf;
                pread_exact(&file, &mut buf[range], offset)?;
                Ok(buf)
            })
            .await
            .map_err(io::Error::other)?,
        }
    }

    /// Writes `buf[range]` to the file at `offset`; returns the buffer for reuse.
    pub async fn write_at(
        &self,
        file: Arc<File>,
        offset: u64,
        buf: Vec<u8>,
        range: Range<usize>,
    ) -> io::Result<Vec<u8>> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Uring(h) => h.submit(uring::Op::Write, file, offset, buf, range).await,
            Backend::Blocking => tokio::task::spawn_blocking(move || {
                pwrite_all(&file, &buf[range.clone()], offset)?;
                Ok(buf)
            })
            .await
            .map_err(io::Error::other)?,
        }
    }
}

impl Default for DiskIo {
    fn default() -> Self {
        DiskIo::new()
    }
}

pub fn pread_exact(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file shorter than expected",
                ));
            }
            done += n;
        }
        Ok(())
    }
}

pub fn pwrite_all(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = file.seek_write(&buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "disk write returned 0",
                ));
            }
            done += n;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod uring {
    use io_uring::{IoUring, Probe, opcode, types};
    use std::collections::HashMap;
    use std::fs::File;
    use std::io;
    use std::ops::Range;
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
    use tokio::sync::oneshot;

    #[derive(Clone, Copy)]
    pub enum Op {
        Read,
        Write,
    }

    struct Request {
        op: Op,
        file: Arc<File>,
        offset: u64,
        buf: Vec<u8>,
        range: Range<usize>,
        reply: oneshot::Sender<io::Result<Vec<u8>>>,
    }

    struct InFlight {
        req: Request,
        done: usize,
    }

    pub struct UringHandle {
        tx: Sender<Request>,
    }

    impl UringHandle {
        pub fn start() -> io::Result<UringHandle> {
            let ring = IoUring::new(128)?;
            let mut probe = Probe::new();
            ring.submitter().register_probe(&mut probe)?;
            if !probe.is_supported(opcode::Read::CODE) || !probe.is_supported(opcode::Write::CODE) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "kernel lacks IORING_OP_READ/WRITE",
                ));
            }
            let (tx, rx) = channel();
            std::thread::Builder::new()
                .name("omnidrop-io-uring".into())
                .spawn(move || run(ring, rx))?;
            Ok(UringHandle { tx })
        }

        pub async fn submit(
            &self,
            op: Op,
            file: Arc<File>,
            offset: u64,
            buf: Vec<u8>,
            range: Range<usize>,
        ) -> io::Result<Vec<u8>> {
            if range.end > buf.len() || range.start > range.end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "bad buffer range",
                ));
            }
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(Request {
                    op,
                    file,
                    offset,
                    buf,
                    range,
                    reply,
                })
                .map_err(|_| io::Error::other("io_uring worker stopped"))?;
            rx.await
                .map_err(|_| io::Error::other("io_uring worker dropped the request"))?
        }
    }

    fn push(ring: &mut IoUring, id: u64, inf: &mut InFlight) -> io::Result<()> {
        let start = inf.req.range.start + inf.done;
        let len = (inf.req.range.end - start) as u32;
        let fd = types::Fd(inf.req.file.as_raw_fd());
        let off = inf.req.offset + inf.done as u64;
        let entry = match inf.req.op {
            // SAFETY: the buffer is owned by `inf`, which stays in the in-flight table (and is not
            // reallocated) until the matching completion has been reaped.
            Op::Read => unsafe {
                opcode::Read::new(fd, inf.req.buf.as_mut_ptr().add(start), len)
                    .offset(off)
                    .build()
            },
            Op::Write => unsafe {
                opcode::Write::new(fd, inf.req.buf.as_ptr().add(start), len)
                    .offset(off)
                    .build()
            },
        }
        .user_data(id);
        loop {
            // SAFETY: see above; the entry references memory kept alive by the in-flight table.
            let pushed = unsafe { ring.submission().push(&entry).is_ok() };
            if pushed {
                return Ok(());
            }
            ring.submit()?;
        }
    }

    fn run(mut ring: IoUring, rx: Receiver<Request>) {
        let mut inflight: HashMap<u64, InFlight> = HashMap::new();
        let mut next_id: u64 = 1;
        let mut enqueue =
            |ring: &mut IoUring, inflight: &mut HashMap<u64, InFlight>, req: Request| {
                let id = next_id;
                next_id = next_id.wrapping_add(1).max(1);
                let mut inf = InFlight { req, done: 0 };
                if inf.req.range.is_empty() {
                    let buf = std::mem::take(&mut inf.req.buf);
                    let _ = inf.req.reply.send(Ok(buf));
                    return;
                }
                match push(ring, id, &mut inf) {
                    Ok(()) => {
                        inflight.insert(id, inf);
                    }
                    Err(e) => {
                        let _ = inf.req.reply.send(Err(e));
                    }
                }
            };
        loop {
            if inflight.is_empty() {
                match rx.recv() {
                    Ok(req) => enqueue(&mut ring, &mut inflight, req),
                    Err(_) => return,
                }
            }
            loop {
                match rx.try_recv() {
                    Ok(req) => enqueue(&mut ring, &mut inflight, req),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) if inflight.is_empty() => return,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            if inflight.is_empty() {
                continue;
            }
            match ring.submit_and_wait(1) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {}
                Err(e) => {
                    for (_, inf) in inflight.drain() {
                        let _ = inf
                            .req
                            .reply
                            .send(Err(io::Error::new(e.kind(), e.to_string())));
                    }
                    continue;
                }
            }
            let completions: Vec<(u64, i32)> = ring
                .completion()
                .map(|c| (c.user_data(), c.result()))
                .collect();
            for (id, res) in completions {
                let Some(mut inf) = inflight.remove(&id) else {
                    continue;
                };
                if res < 0 {
                    let _ = inf.req.reply.send(Err(io::Error::from_raw_os_error(-res)));
                    continue;
                }
                if res == 0 {
                    let kind = match inf.req.op {
                        Op::Read => io::ErrorKind::UnexpectedEof,
                        Op::Write => io::ErrorKind::WriteZero,
                    };
                    let _ = inf.req.reply.send(Err(io::Error::new(kind, "short I/O")));
                    continue;
                }
                inf.done += res as usize;
                if inf.req.range.start + inf.done >= inf.req.range.end {
                    let buf = std::mem::take(&mut inf.req.buf);
                    let _ = inf.req.reply.send(Ok(buf));
                } else {
                    match push(&mut ring, id, &mut inf) {
                        Ok(()) => {
                            inflight.insert(id, inf);
                        }
                        Err(e) => {
                            let _ = inf.req.reply.send(Err(e));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn roundtrip_positional_io() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(&vec![0u8; 8192]).unwrap();
        drop(f);
        let file = Arc::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap(),
        );
        let io = DiskIo::new();
        let mut data = vec![0u8; 100];
        data[10..20].copy_from_slice(b"0123456789");
        io.write_at(file.clone(), 4000, data, 10..20).await.unwrap();
        let out = io.read_at(file, 3998, vec![0u8; 16], 2..16).await.unwrap();
        assert_eq!(&out[4..14], b"0123456789");
        assert_eq!(&out[2..4], &[0, 0]);
    }
}
