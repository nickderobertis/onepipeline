//! A minimal RFC 6455 WebSocket client over a local stream socket — what
//! `codex app-server --listen unix://PATH` speaks.
//!
//! The socket is an `AF_UNIX` stream on every platform: `std`'s
//! `UnixStream` on Unix, and the same address family through Winsock on
//! Windows (10 1803 and later). Never TCP: a loopback port is reachable by any
//! local user, while the socket lives in a directory only its owner can enter.
//!
//! Only what a JSON-RPC client needs is implemented: the opening handshake,
//! masked text frames out, unmasked text frames in (fragmented or not), pings
//! answered, and a close or EOF reported as the end of the connection.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The largest message accepted from the server. A JSON-RPC frame from the
/// app-server is at most a few megabytes (a large command's aggregated
/// output); a peer announcing more is broken, and reading it would grow this
/// process without bound.
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;

/// The longest the handshake's HTTP response may be before it is refused.
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;

/// The path to connect to for a server told to listen on `path`.
///
/// Codex binds a `--listen` path longer than the platform's `sun_path` as a
/// short target under `/tmp/codex-daemon-<uid>/` and leaves a symlink at
/// `path` — a symlink a socket `connect` cannot follow past the length limit
/// either, so the client connects to the link's target itself.
pub fn dial_path(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(target) if target.is_absolute() => target,
            Ok(target) => path.parent().unwrap_or(Path::new("")).join(target),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// One connected local stream socket.
pub struct LocalStream {
    inner: platform::Stream,
}

impl LocalStream {
    /// Connect to the socket a server listens on at `path` (through its
    /// symlink, per [`dial_path`]).
    pub fn connect(path: &Path) -> io::Result<Self> {
        Ok(LocalStream {
            inner: platform::Stream::connect(&dial_path(path))?,
        })
    }

    /// A second handle on the same connection, for a writer on another thread.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(LocalStream {
            inner: self.inner.try_clone()?,
        })
    }

    /// Bound every read: a read that waits longer returns
    /// [`io::ErrorKind::WouldBlock`] or [`io::ErrorKind::TimedOut`].
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    /// Close both directions, waking any reader blocked on this connection.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl Read for LocalStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for LocalStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What one read of the connection produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    /// One whole text message.
    Text(String),
    /// Nothing arrived within the read timeout; the connection is still open.
    Idle,
    /// The server closed the connection (a close frame, or EOF).
    Closed,
}

/// The sending half: every message is one masked text frame.
pub struct WsWriter {
    stream: LocalStream,
}

impl WsWriter {
    /// Send `text` as one text message.
    pub fn send_text(&mut self, text: &str) -> io::Result<()> {
        self.send_frame(0x1, text.as_bytes())
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | opcode);
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8);
        } else if len <= usize::from(u16::MAX) {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask = mask_key();
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.stream.write_all(&frame)
    }

    /// Say goodbye and close the connection.
    pub fn close(&mut self) {
        let _ = self.send_frame(0x8, &[]);
        self.stream.shutdown();
    }
}

/// The receiving half. Partial frames survive a read timeout, so a reader that
/// polls in short slices never loses a message cut across two of them.
pub struct WsReader {
    stream: LocalStream,
    buffer: Vec<u8>,
    /// The fragmented message being assembled, while one is.
    message: Option<Vec<u8>>,
    writer: Arc<Mutex<WsWriter>>,
}

impl WsReader {
    /// The next whole text message, [`Incoming::Idle`] if none arrived within
    /// the stream's read timeout, or [`Incoming::Closed`] once the server is
    /// gone.
    pub fn receive(&mut self) -> io::Result<Incoming> {
        loop {
            if let Some(incoming) = self.take_frame()? {
                return Ok(incoming);
            }
            let mut chunk = [0u8; 16 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(Incoming::Closed),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(Incoming::Idle)
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    return Ok(Incoming::Closed)
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Bound every read of [`Self::receive`]. A connection whose peer has
    /// closed it takes no bound and needs none (see `bound_reads`): the next
    /// receive reports what the peer sent, then [`Incoming::Closed`].
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        bound_reads(&self.stream, timeout)
    }

    fn take_frame(&mut self) -> io::Result<Option<Incoming>> {
        let Some(frame) = parse_frame(&self.buffer)? else {
            return Ok(None);
        };
        let payload = self.buffer[frame.payload_start..frame.payload_start + frame.len].to_vec();
        self.buffer.drain(..frame.payload_start + frame.len);
        match frame.opcode {
            // A JSON-RPC server sends text; a binary message is not one.
            0x2 => Err(protocol_error(
                "a binary message where JSON-RPC text was expected",
            )),
            0x0 | 0x1 => {
                let mut message = match (frame.opcode, self.message.take()) {
                    (0x1, Some(_)) => {
                        return Err(protocol_error(
                            "a new message before the fragmented one ended",
                        ))
                    }
                    (0x1, None) => Vec::new(),
                    (_, Some(started)) => started,
                    (_, None) => {
                        return Err(protocol_error("a continuation frame with no message"))
                    }
                };
                if (message.len() + payload.len()) as u64 > MAX_MESSAGE_BYTES {
                    return Err(protocol_error("a message larger than 64 MiB"));
                }
                message.extend_from_slice(&payload);
                if !frame.fin {
                    self.message = Some(message);
                    return Ok(None);
                }
                let text = String::from_utf8(message)
                    .map_err(|_| protocol_error("a text message that is not UTF-8"))?;
                Ok(Some(Incoming::Text(text)))
            }
            0x8 => {
                close_payload_valid(&payload)?;
                Ok(Some(Incoming::Closed))
            }
            0x9 => {
                if let Ok(mut writer) = self.writer.lock() {
                    let _ = writer.send_frame(0xA, &payload);
                }
                Ok(None)
            }
            // A pong: nothing to do. `parse_frame` refused every other opcode.
            _ => Ok(None),
        }
    }
}

/// A close frame's payload as RFC 6455 §5.5.1 and §7.4 allow it: empty, or a
/// status code a peer may send followed by a UTF-8 reason.
fn close_payload_valid(payload: &[u8]) -> io::Result<()> {
    match payload {
        [] => Ok(()),
        [_] => Err(protocol_error("a close frame with a one-byte payload")),
        [high, low, reason @ ..] => {
            let code = u16::from_be_bytes([*high, *low]);
            let sendable = matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999);
            if !sendable {
                return Err(protocol_error(
                    "a close frame with a status no peer may send",
                ));
            }
            std::str::from_utf8(reason)
                .map(|_| ())
                .map_err(|_| protocol_error("a close frame whose reason is not UTF-8"))
        }
    }
}

/// One frame's header, read off the front of a buffer.
struct FrameHeader {
    fin: bool,
    opcode: u8,
    payload_start: usize,
    len: usize,
}

/// The header of the frame at the front of `buffer` once all of it — header
/// and payload — is there. A masked frame from the server is a protocol
/// violation (RFC 6455 §5.1) and refused.
fn parse_frame(buffer: &[u8]) -> io::Result<Option<FrameHeader>> {
    if buffer.len() < 2 {
        return Ok(None);
    }
    let fin = buffer[0] & 0x80 != 0;
    let opcode = buffer[0] & 0x0f;
    if buffer[0] & 0x70 != 0 {
        return Err(protocol_error(
            "reserved bits set with no extension negotiated",
        ));
    }
    if !matches!(opcode, 0x0..=0x2 | 0x8..=0xA) {
        return Err(protocol_error("a reserved opcode"));
    }
    if buffer[1] & 0x80 != 0 {
        return Err(protocol_error("a masked frame from the server"));
    }
    if opcode >= 0x8 && (!fin || buffer[1] & 0x7f > 125) {
        return Err(protocol_error("a fragmented or oversized control frame"));
    }
    let (len, header) = match buffer[1] & 0x7f {
        126 => {
            if buffer.len() < 4 {
                return Ok(None);
            }
            let len = u64::from(u16::from_be_bytes([buffer[2], buffer[3]]));
            if len < 126 {
                return Err(protocol_error("a length not in its shortest encoding"));
            }
            (len, 4)
        }
        127 => {
            if buffer.len() < 10 {
                return Ok(None);
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[2..10]);
            let len = u64::from_be_bytes(bytes);
            if len <= u64::from(u16::MAX) {
                return Err(protocol_error("a length not in its shortest encoding"));
            }
            (len, 10)
        }
        short => (u64::from(short), 2),
    };
    if len > MAX_MESSAGE_BYTES {
        return Err(protocol_error("a frame larger than 64 MiB"));
    }
    let len = len as usize;
    if buffer.len() < header + len {
        return Ok(None);
    }
    Ok(Some(FrameHeader {
        fin,
        opcode,
        payload_start: header,
        len,
    }))
}

/// The longest a server may take to answer the WebSocket upgrade.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

/// Open a WebSocket on a connected stream: send the upgrade request and read
/// the `101 Switching Protocols` answer. Anything else is refused, naming the
/// status line the server sent.
///
/// The answer must arrive within [`HANDSHAKE_DEADLINE`]: a peer that accepts
/// the connection and never answers fails the handshake rather than holding
/// the caller past its own bounds.
pub fn handshake(stream: LocalStream) -> io::Result<(WsReader, Arc<Mutex<WsWriter>>)> {
    upgrade(stream)
}

/// [`handshake`] over any stream that ends up a [`LocalStream`].
fn upgrade(mut stream: impl UpgradeStream) -> io::Result<(WsReader, Arc<Mutex<WsWriter>>)> {
    let key = base64(&random_bytes::<16>());
    let response = read_upgrade_response(&mut stream, &key)?;
    let text = String::from_utf8_lossy(&response);
    let status = text.lines().next().unwrap_or_default().to_string();
    let header = |wanted: &str| {
        text.lines().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_string())
        })
    };
    let upgraded = status.split_whitespace().nth(1) == Some("101")
        && header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        && header("connection").is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    if !upgraded {
        return Err(protocol_error(&format!(
            "the server refused the WebSocket upgrade: `{status}`"
        )));
    }
    if header("sec-websocket-accept").as_deref() != Some(accept_key(&key).as_str()) {
        return Err(protocol_error(
            "the server's Sec-WebSocket-Accept does not answer this handshake's key",
        ));
    }
    // The handshake's bound is its own: the reader sets whatever its caller
    // asks for from here on.
    bound_reads(&stream, None)?;
    let stream = stream.into_local();
    let writer = Arc::new(Mutex::new(WsWriter {
        stream: stream.try_clone()?,
    }));
    Ok((
        WsReader {
            stream,
            buffer: Vec::new(),
            message: None,
            writer: Arc::clone(&writer),
        },
        writer,
    ))
}

/// The stream calls the upgrade makes, so a test can stand in for a platform's
/// socket where this one behaves differently.
trait UpgradeStream: Read + Write {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn into_local(self) -> LocalStream;
}

impl UpgradeStream for LocalStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        LocalStream::set_read_timeout(self, timeout)
    }

    fn into_local(self) -> LocalStream {
        self
    }
}

/// Bound the stream's reads, unless the peer has already closed it.
///
/// A zero timeout is refused here, before any socket call, on every platform:
/// `std` refuses it on Unix, and this module's Winsock path would otherwise
/// round it up to a millisecond.
///
/// Darwin refuses `setsockopt` with `EINVAL` on a socket whose peer has closed
/// both directions, where Linux sets the option. Such a socket cannot block a
/// read — it yields what the peer sent, then end-of-file — so that refusal is
/// not the connection's failure, and the read after it reports what the peer
/// actually did: a server that writes its refusal and closes is named by its
/// status line, not by `Invalid argument (os error 22)`. Only the socket call's
/// own `EINVAL` is passed over; every other failure is the caller's.
fn bound_reads(stream: &impl UpgradeStream, timeout: Option<Duration>) -> io::Result<()> {
    if timeout == Some(Duration::ZERO) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot bound reads by a zero timeout",
        ));
    }
    match stream.set_read_timeout(timeout) {
        Err(err) if err.raw_os_error() == Some(SOCKET_OPTION_REFUSED) => Ok(()),
        other => other,
    }
}

/// The OS error a socket-option call returns on a connection its peer closed.
#[cfg(unix)]
const SOCKET_OPTION_REFUSED: i32 = libc::EINVAL;
#[cfg(windows)]
const SOCKET_OPTION_REFUSED: i32 = windows_sys::Win32::Networking::WinSock::WSAEINVAL;

/// Send the upgrade request for `key` and read the server's whole HTTP answer,
/// within [`HANDSHAKE_DEADLINE`].
fn read_upgrade_response(stream: &mut impl UpgradeStream, key: &str) -> io::Result<Vec<u8>> {
    let request = format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    let deadline = std::time::Instant::now() + HANDSHAKE_DEADLINE;
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() > MAX_HANDSHAKE_BYTES {
            return Err(protocol_error("a handshake response larger than 16 KiB"));
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the server did not answer the WebSocket upgrade within {}s",
                    HANDSHAKE_DEADLINE.as_secs()
                ),
            ));
        }
        bound_reads(stream, Some(left))?;
        let read = match stream.read(&mut byte) {
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            other => other?,
        };
        match read {
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the server closed the connection during the WebSocket handshake",
                ))
            }
            _ => response.push(byte[0]),
        }
    }
    Ok(response)
}

fn protocol_error(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("websocket protocol error: {what}"),
    )
}

/// Bytes that need only be unpredictable to the peer, not secret: a masking key
/// and a handshake nonce. UUIDv7's random tail is enough.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let id = uuid::Uuid::now_v7();
        let tail = &id.as_bytes()[8..];
        let take = (N - filled).min(tail.len());
        out[filled..filled + take].copy_from_slice(&tail[..take]);
        filled += take;
    }
    out
}

fn mask_key() -> [u8; 4] {
    random_bytes::<4>()
}

/// The `Sec-WebSocket-Accept` a server answers the handshake key `key` with
/// (RFC 6455 §4.2.2): base64 of the SHA-1 of the key and the protocol's GUID.
#[must_use]
pub fn accept_key(key: &str) -> String {
    base64(&sha1(
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    ))
}

/// SHA-1 (FIPS 180-4), for the handshake's accept key alone: the protocol
/// names this digest, and nothing here relies on it for security.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut data = message.to_vec();
    let bits = (message.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());
    for block in data.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard base64 with padding, for the handshake key.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * index) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(unix)]
mod platform {
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    pub struct Stream(pub(super) UnixStream);

    impl Stream {
        pub fn connect(path: &Path) -> io::Result<Self> {
            UnixStream::connect(path).map(Stream)
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Stream)
        }

        pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }

        pub fn shutdown(&self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }

        pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }

        pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
    }
}

#[cfg(windows)]
mod platform {
    //! `AF_UNIX` stream sockets through Winsock (Windows 10 1803 and later).
    //! `std` has no Unix-socket type on Windows, so the few calls a client
    //! needs are made directly.

    use std::io;
    use std::path::Path;
    use std::sync::{Arc, Once};
    use std::time::Duration;

    use windows_sys::Win32::Networking::WinSock::{
        closesocket, connect, recv, send, setsockopt, shutdown, socket, WSAGetLastError,
        WSAStartup, AF_UNIX, INVALID_SOCKET, SD_BOTH, SOCKADDR, SOCKADDR_UN, SOCKET, SOCKET_ERROR,
        SOCK_STREAM, SOL_SOCKET, SO_RCVTIMEO, WSADATA,
    };

    struct Owned(SOCKET);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle came from `socket` and is closed exactly once,
            // here, when the last clone of it goes.
            unsafe {
                closesocket(self.0);
            }
        }
    }

    /// One socket shared by the reader and the writer: Winsock's `recv` and
    /// `send` may run on one socket from two threads at once.
    pub struct Stream(Arc<Owned>);

    fn startup() {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            // SAFETY: `data` is a live, writable WSADATA for the call; a failed
            // startup surfaces as the `socket` call failing next.
            unsafe {
                let mut data: WSADATA = std::mem::zeroed();
                WSAStartup(0x0202, &mut data);
            }
        });
    }

    fn last_error() -> io::Error {
        // SAFETY: reads this thread's last Winsock error; no arguments.
        io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
    }

    /// `path` as a `sockaddr_un`, refusing one longer than `sun_path` holds.
    pub(super) fn address(path: &Path) -> io::Result<(SOCKADDR_UN, i32)> {
        let bytes = path.to_string_lossy().into_owned().into_bytes();
        let mut address = SOCKADDR_UN {
            sun_family: AF_UNIX,
            sun_path: [0; 108],
        };
        if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "`{}` cannot name an AF_UNIX socket: it is longer than 107 bytes or holds NUL",
                    path.display()
                ),
            ));
        }
        for (slot, byte) in address.sun_path.iter_mut().zip(&bytes) {
            *slot = *byte as i8;
        }
        let len = (std::mem::size_of::<u16>() + bytes.len() + 1) as i32;
        Ok((address, len))
    }

    pub(super) fn new_socket() -> io::Result<SOCKET> {
        startup();
        // SAFETY: plain integer arguments; the result is checked below.
        let handle = unsafe { socket(i32::from(AF_UNIX), SOCK_STREAM, 0) };
        if handle == INVALID_SOCKET {
            return Err(last_error());
        }
        Ok(handle)
    }

    impl Stream {
        pub fn connect(path: &Path) -> io::Result<Self> {
            let (address, len) = address(path)?;
            let owned = Owned(new_socket()?);
            // SAFETY: `address` is a live SOCKADDR_UN of at least `len` bytes,
            // which is the layout Winsock reads for AF_UNIX.
            let result = unsafe { connect(owned.0, (&raw const address).cast::<SOCKADDR>(), len) };
            if result == SOCKET_ERROR {
                return Err(last_error());
            }
            Ok(Stream(Arc::new(owned)))
        }

        #[cfg(test)]
        pub(super) fn from_raw(handle: SOCKET) -> Self {
            Stream(Arc::new(Owned(handle)))
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Stream(Arc::clone(&self.0)))
        }

        pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            let millis: u32 =
                timeout.map_or(0, |t| t.as_millis().clamp(1, u128::from(u32::MAX)) as u32);
            // SAFETY: the option value is a live u32 of the length passed.
            let result = unsafe {
                setsockopt(
                    (self.0).0,
                    SOL_SOCKET,
                    SO_RCVTIMEO,
                    (&raw const millis).cast::<u8>(),
                    std::mem::size_of::<u32>() as i32,
                )
            };
            if result == SOCKET_ERROR {
                return Err(last_error());
            }
            Ok(())
        }

        pub fn shutdown(&self) {
            // SAFETY: the handle is live for as long as `self` is.
            unsafe {
                shutdown((self.0).0, SD_BOTH);
            }
        }

        pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(i32::MAX as usize) as i32;
            // SAFETY: `buf` is live and writable for `len` bytes.
            let read = unsafe { recv((self.0).0, buf.as_mut_ptr(), len, 0) };
            if read == SOCKET_ERROR {
                let err = last_error();
                // WSAETIMEDOUT is the timeout `SO_RCVTIMEO` asked for.
                if err.raw_os_error() == Some(10060) {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, err));
                }
                return Err(err);
            }
            Ok(read as usize)
        }

        pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len().min(i32::MAX as usize) as i32;
            // SAFETY: `buf` is live and readable for `len` bytes.
            let written = unsafe { send((self.0).0, buf.as_ptr(), len, 0) };
            if written == SOCKET_ERROR {
                return Err(last_error());
            }
            Ok(written as usize)
        }
    }
}

/// A listening `AF_UNIX` socket, for the tests that stand a server up on every
/// platform: `std`'s `UnixListener` on Unix, Winsock on Windows.
#[cfg(test)]
pub(crate) mod listener {
    use std::io;
    use std::path::Path;

    use super::LocalStream;

    pub struct LocalListener {
        #[cfg(unix)]
        inner: std::os::unix::net::UnixListener,
        #[cfg(windows)]
        inner: windows_sys::Win32::Networking::WinSock::SOCKET,
    }

    impl LocalListener {
        pub fn bind(path: &Path) -> io::Result<Self> {
            #[cfg(unix)]
            {
                Ok(LocalListener {
                    inner: std::os::unix::net::UnixListener::bind(path)?,
                })
            }
            #[cfg(windows)]
            {
                use windows_sys::Win32::Networking::WinSock::{
                    bind, listen, SOCKADDR, SOCKET_ERROR,
                };
                let (address, len) = super::platform::address(path)?;
                let handle = super::platform::new_socket()?;
                // SAFETY: `address` is a live SOCKADDR_UN of `len` bytes.
                unsafe {
                    if bind(handle, (&raw const address).cast::<SOCKADDR>(), len) == SOCKET_ERROR
                        || listen(handle, 8) == SOCKET_ERROR
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(LocalListener { inner: handle })
            }
        }

        pub fn accept(&self) -> io::Result<LocalStream> {
            #[cfg(unix)]
            {
                let (stream, _) = self.inner.accept()?;
                Ok(LocalStream {
                    inner: super::platform::Stream(stream),
                })
            }
            #[cfg(windows)]
            {
                use windows_sys::Win32::Networking::WinSock::{accept, INVALID_SOCKET};
                // SAFETY: null address out-pointers are allowed by `accept`.
                let handle =
                    unsafe { accept(self.inner, std::ptr::null_mut(), std::ptr::null_mut()) };
                if handle == INVALID_SOCKET {
                    return Err(io::Error::last_os_error());
                }
                Ok(LocalStream {
                    inner: super::platform::Stream::from_raw(handle),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::scratch::ScratchDir;
    use std::io::BufRead;

    /// Read a client's whole upgrade request, so a server that answers and
    /// drops the stream leaves nothing unread — on Windows, closing a socket
    /// with unread bytes resets it, and the client then reads `10054`
    /// instead of the answer.
    fn read_upgrade(stream: &mut impl Read) -> String {
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
            request.push(byte[0]);
        }
        String::from_utf8(request).unwrap()
    }

    /// A server's `101` answer to `request`, with the accept key it owes.
    fn switching(request: &str) -> String {
        let key = request
            .lines()
            .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
            .expect("the upgrade carries a key");
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(key.trim())
        )
    }

    #[test]
    fn the_accept_key_matches_rfc_6455_s_worked_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(
            sha1(b"abc")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (input, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), encoded);
        }
    }

    /// A server frame, unmasked as RFC 6455 requires of one.
    fn server_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![if fin { 0x80 } else { 0 } | opcode];
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        frame
    }

    /// Unmask one client frame off `reader`, returning its opcode and payload.
    fn read_client_frame(reader: &mut impl Read) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        reader.read_exact(&mut head).unwrap();
        assert!(head[1] & 0x80 != 0, "a client frame must be masked");
        let len = match head[1] & 0x7f {
            126 => {
                let mut ext = [0u8; 2];
                reader.read_exact(&mut ext).unwrap();
                usize::from(u16::from_be_bytes(ext))
            }
            127 => {
                let mut ext = [0u8; 8];
                reader.read_exact(&mut ext).unwrap();
                u64::from_be_bytes(ext) as usize
            }
            short => usize::from(short),
        };
        let mut mask = [0u8; 4];
        reader.read_exact(&mut mask).unwrap();
        let mut payload = vec![0u8; len];
        reader.read_exact(&mut payload).unwrap();
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
        (head[0] & 0x0f, payload)
    }

    /// A client talks to a server standing on a real `AF_UNIX` socket, on
    /// every platform: the handshake, a masked message out, a fragmented
    /// message and a ping in (answered with a pong), a message across a read
    /// timeout, and the close.
    #[test]
    fn a_client_exchanges_messages_with_a_server_on_a_local_socket() {
        let dir = ScratchDir::new("ws-local").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(request.starts_with("GET / HTTP/1.1\r\n"), "{request}");
            assert!(request.contains("Upgrade: websocket\r\n"), "{request}");
            writer.write_all(switching(&request).as_bytes()).unwrap();
            let (opcode, payload) = read_client_frame(&mut reader);
            assert_eq!(
                (opcode, payload.as_slice()),
                (0x1, b"{\"id\":1}".as_slice())
            );
            let long = "x".repeat(300);
            writer
                .write_all(&server_frame(0x1, false, b"{\"part\":"))
                .unwrap();
            writer.write_all(&server_frame(0x9, true, b"hi")).unwrap();
            writer.write_all(&server_frame(0x0, true, b"1}")).unwrap();
            let (opcode, payload) = read_client_frame(&mut reader);
            assert_eq!((opcode, payload.as_slice()), (0xA, b"hi".as_slice()));
            let frame = server_frame(0x1, true, long.as_bytes());
            writer.write_all(&frame[..3]).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            writer.write_all(&frame[3..]).unwrap();
            writer.write_all(&server_frame(0x8, true, b"")).unwrap();
        });

        let (mut reader, writer) = handshake(LocalStream::connect(&path).unwrap()).unwrap();
        writer.lock().unwrap().send_text("{\"id\":1}").unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut texts = Vec::new();
        let mut idles = 0;
        loop {
            match reader.receive().unwrap() {
                Incoming::Text(text) => texts.push(text),
                Incoming::Idle => idles += 1,
                Incoming::Closed => break,
            }
            assert!(idles < 200, "the server never closed");
        }
        server.join().unwrap();
        assert_eq!(texts, ["{\"part\":1}".to_string(), "x".repeat(300)]);
        assert!(idles > 0, "the split message never crossed a read timeout");
    }

    #[test]
    fn a_refused_upgrade_names_the_status_the_server_sent() {
        let dir = ScratchDir::new("ws-refused").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            read_upgrade(&mut stream);
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .unwrap();
        });
        let err = handshake(LocalStream::connect(&path).unwrap())
            .err()
            .unwrap();
        server.join().unwrap();
        assert!(
            err.to_string().contains("HTTP/1.1 400 Bad Request"),
            "{err}"
        );
    }

    /// A connection as Darwin's socket layer has it once the peer closed both
    /// directions: what the peer sent is still read, but `setsockopt` — so
    /// every read timeout — fails with `EINVAL`. The closing is real; only the
    /// refusal Linux does not make is played, from the moment the server has
    /// dropped its end.
    struct PeerClosedOnDarwin {
        stream: LocalStream,
        server_closed: std::sync::mpsc::Receiver<()>,
        closed: std::cell::Cell<bool>,
    }

    impl Read for PeerClosedOnDarwin {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.stream.read(buf)
        }
    }

    impl Write for PeerClosedOnDarwin {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let written = self.stream.write(buf)?;
            if !self.closed.get() {
                // The server answers the whole request and closes before the
                // client makes its next call: the race CI's macOS runner lost.
                self.server_closed.recv().unwrap();
                self.closed.set(true);
            }
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl UpgradeStream for PeerClosedOnDarwin {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            if self.closed.get() {
                return Err(io::Error::from_raw_os_error(SOCKET_OPTION_REFUSED));
            }
            self.stream.set_read_timeout(timeout)
        }

        fn into_local(self) -> LocalStream {
            self.stream
        }
    }

    /// A server that answers the upgrade with `answer` (given the request)
    /// and closes at once, and a client connection to it on which every
    /// socket option fails after that close, as on Darwin.
    fn answer_and_close(
        tag: &str,
        answer: impl FnOnce(&str) -> String + Send + 'static,
    ) -> (PeerClosedOnDarwin, std::thread::JoinHandle<()>, ScratchDir) {
        let dir = ScratchDir::new(tag).unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let (closed, server_closed) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let request = read_upgrade(&mut stream);
            stream.write_all(answer(&request).as_bytes()).unwrap();
            drop(stream);
            closed.send(()).unwrap();
        });
        let client = PeerClosedOnDarwin {
            stream: LocalStream::connect(&path).unwrap(),
            server_closed,
            closed: std::cell::Cell::new(false),
        };
        (client, server, dir)
    }

    #[test]
    fn a_refusal_from_a_peer_that_closed_at_once_names_its_status_where_socket_options_fail() {
        let (client, server, _dir) = answer_and_close("ws-refused-closed", |_| {
            "HTTP/1.1 400 Bad Request\r\n\r\n".to_string()
        });
        let err = upgrade(client).err().unwrap();
        server.join().unwrap();
        assert!(
            err.to_string()
                .contains("refused the WebSocket upgrade: `HTTP/1.1 400 Bad Request`"),
            "{err}"
        );
    }

    #[test]
    fn an_upgrade_from_a_peer_that_closed_at_once_opens_and_reads_closed_where_socket_options_fail()
    {
        let (client, server, _dir) = answer_and_close("ws-upgraded-closed", switching);
        let (mut reader, _writer) = upgrade(client).unwrap();
        server.join().unwrap();
        assert_eq!(reader.receive().unwrap(), Incoming::Closed);
    }

    /// A zero read timeout is refused through the reader, as `std` refuses it:
    /// it bounds nothing, so accepting it would leave the reader's polling
    /// unbounded while its caller believed otherwise. Any other failure of the
    /// socket call is the caller's too, and only Darwin's peer-closed `EINVAL`
    /// is passed over.
    #[test]
    fn a_zero_read_timeout_is_refused_on_a_live_connection() {
        let dir = ScratchDir::new("ws-zero-timeout").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let request = read_upgrade(&mut stream);
            stream.write_all(switching(&request).as_bytes()).unwrap();
            let _ = finished.recv();
        });
        let (reader, _writer) = handshake(LocalStream::connect(&path).unwrap()).unwrap();
        let err = reader
            .set_read_timeout(Some(Duration::ZERO))
            .expect_err("a zero timeout was accepted");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        reader
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let other = bound_reads(
            &Refusing(SOCKET_OPTION_REFUSED + 1),
            Some(Duration::from_secs(1)),
        );
        assert!(
            other.is_err(),
            "a socket failure besides the peer-closed one was passed over"
        );
        done.send(()).unwrap();
        server.join().unwrap();
    }

    /// A stream whose every socket-option call fails with the OS error it holds.
    struct Refusing(i32);

    impl Read for Refusing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for Refusing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl UpgradeStream for Refusing {
        fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(self.0))
        }

        fn into_local(self) -> LocalStream {
            unreachable!("never upgraded")
        }
    }

    #[test]
    fn an_upgrade_answering_another_key_is_refused() {
        let dir = ScratchDir::new("ws-wrong-accept").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            read_upgrade(&mut stream);
            stream
                .write_all(switching("Sec-WebSocket-Key: some-other-key\r\n").as_bytes())
                .unwrap();
        });
        let err = handshake(LocalStream::connect(&path).unwrap())
            .err()
            .unwrap();
        server.join().unwrap();
        assert!(err.to_string().contains("Sec-WebSocket-Accept"), "{err}");
    }

    #[test]
    fn a_masked_or_oversized_server_frame_is_refused() {
        assert!(parse_frame(&[0x81, 0x81, 0, 0, 0, 0, b'x']).is_err());
        assert!(parse_frame(&[0xC1, 0x01, b'x']).is_err(), "a reserved bit");
        assert!(
            parse_frame(&[0x83, 0x01, b'x']).is_err(),
            "a reserved opcode"
        );
        assert!(
            parse_frame(&[0x09, 0x00]).is_err(),
            "a fragmented control frame"
        );
        assert!(
            parse_frame(&[0x81, 126, 0, 5, 1, 2, 3, 4, 5]).is_err(),
            "a long encoding of 5"
        );
        let mut huge = vec![0x81, 127];
        huge.extend_from_slice(&(MAX_MESSAGE_BYTES + 1).to_be_bytes());
        assert!(parse_frame(&huge).is_err());
        assert!(parse_frame(&[0x81]).unwrap().is_none());
    }

    /// A server frame of any length, its 64-bit length encoding included.
    fn long_server_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![if fin { 0x80 } else { 0 } | opcode, 127];
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    /// Every violation a server can commit after a good upgrade, sent over a
    /// real `AF_UNIX` connection: the client's next receive fails, naming it.
    #[test]
    fn every_protocol_violation_on_a_live_connection_fails_the_receive_naming_it() {
        let half = vec![b'x'; 33 * 1024 * 1024];
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "a masked frame from the server",
                vec![0x81, 0x81, 0, 0, 0, 0, b'x'],
            ),
            (
                "reserved bits set with no extension negotiated",
                vec![0xC1, 0x01, b'x'],
            ),
            ("a reserved opcode", vec![0x83, 0x01, b'x']),
            ("a fragmented or oversized control frame", vec![0x09, 0x00]),
            (
                "a length not in its shortest encoding",
                vec![0x81, 126, 0, 5, 1, 2, 3, 4, 5],
            ),
            ("a binary message", server_frame(0x2, true, b"{}")),
            (
                "a continuation frame with no message",
                server_frame(0x0, true, b"{}"),
            ),
            ("a new message before the fragmented one ended", {
                let mut both = server_frame(0x1, false, b"{");
                both.extend(server_frame(0x1, true, b"}"));
                both
            }),
            (
                "a text message that is not UTF-8",
                server_frame(0x1, true, &[0xff, 0xfe]),
            ),
            (
                "a close frame with a one-byte payload",
                server_frame(0x8, true, &[3]),
            ),
            (
                "a close frame with a status no peer may send",
                server_frame(0x8, true, &[0x03, 0xED]),
            ),
            (
                "a close frame whose reason is not UTF-8",
                server_frame(0x8, true, &[0x03, 0xE8, 0xff]),
            ),
            ("a message larger than 64 MiB", {
                let mut both = long_server_frame(0x1, false, &half);
                both.extend(long_server_frame(0x0, true, &half));
                both
            }),
        ];
        for (index, (named, bytes)) in cases.into_iter().enumerate() {
            let dir = ScratchDir::new(&format!("ws-violation-{index}")).unwrap();
            let path = dir.join("s.sock");
            let listener = listener::LocalListener::bind(&path).unwrap();
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            let server = std::thread::spawn(move || {
                let stream = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                writer.write_all(switching(&request).as_bytes()).unwrap();
                // The client may refuse before reading everything.
                let _ = writer.write_all(&bytes);
                let _ = done_rx.recv_timeout(Duration::from_secs(30));
            });
            let (mut reader, _writer) = handshake(LocalStream::connect(&path).unwrap()).unwrap();
            reader
                .set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let refused = loop {
                match reader.receive() {
                    Err(err) => break err,
                    Ok(Incoming::Idle) => continue,
                    Ok(other) => panic!("{named}: received {other:?} instead of a refusal"),
                }
            };
            let _ = done_tx.send(());
            server.join().unwrap();
            assert_eq!(
                refused.kind(),
                io::ErrorKind::InvalidData,
                "{named}: {refused}"
            );
            assert!(refused.to_string().contains(named), "{named}: {refused}");
        }
    }

    /// A path no `AF_UNIX` address can hold — past `sun_path`, or holding NUL —
    /// is refused before anything is dialled, on every platform (on Windows by
    /// this module's own address builder).
    #[test]
    fn a_path_no_socket_address_can_hold_is_refused() {
        let dir = ScratchDir::new("ws-bad-address").unwrap();
        let long = dir.join("x".repeat(200));
        let nul = dir.join("with\0nul.sock");
        for path in [long, nul] {
            let err = LocalStream::connect(&path).err().unwrap();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidInput,
                "{}: {err}",
                path.display()
            );
            #[cfg(windows)]
            assert!(
                err.to_string().contains("cannot name an AF_UNIX socket"),
                "{err}"
            );
        }
    }

    /// A client message of every length encoding — 7-bit, 16-bit and 64-bit —
    /// reaches a server on a real `AF_UNIX` connection whole.
    #[test]
    fn a_client_message_of_every_length_encoding_arrives_whole() {
        let dir = ScratchDir::new("ws-long-send").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let sizes = [10usize, 300, 70_000];
        let server = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            writer.write_all(switching(&request).as_bytes()).unwrap();
            sizes
                .iter()
                .map(|_| read_client_frame(&mut reader))
                .collect::<Vec<_>>()
        });
        let (_reader, writer) = handshake(LocalStream::connect(&path).unwrap()).unwrap();
        for size in sizes {
            writer.lock().unwrap().send_text(&"y".repeat(size)).unwrap();
        }
        let received = server.join().unwrap();
        for ((opcode, payload), size) in received.into_iter().zip(sizes) {
            assert_eq!(opcode, 0x1);
            assert_eq!(payload, "y".repeat(size).into_bytes(), "{size} bytes");
        }
    }

    #[test]
    fn an_upgrade_answer_past_16_kib_is_refused_on_a_live_connection() {
        let dir = ScratchDir::new("ws-huge-upgrade").unwrap();
        let path = dir.join("s.sock");
        let listener = listener::LocalListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            read_upgrade(&mut stream);
            let padding = format!("X-Padding: {}\r\n", "p".repeat(20 * 1024));
            let _ = stream
                .write_all(format!("HTTP/1.1 101 Switching Protocols\r\n{padding}\r\n").as_bytes());
        });
        let err = handshake(LocalStream::connect(&path).unwrap())
            .err()
            .unwrap();
        server.join().unwrap();
        assert!(err.to_string().contains("larger than 16 KiB"), "{err}");
    }

    #[test]
    fn a_close_with_a_sendable_status_and_reason_is_a_close() {
        assert!(close_payload_valid(&[]).is_ok());
        assert!(close_payload_valid(&[0x03, 0xE8]).is_ok());
        assert!(close_payload_valid(&[0x0B, 0xB8, b'o', b'k']).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_long_listen_path_is_dialled_through_its_symlink() {
        let dir = ScratchDir::new("ws-link").unwrap();
        let target = dir.join("t");
        std::fs::write(&target, b"").unwrap();
        let link = dir.join("deeply-nested-listen-path.sock");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(dial_path(&link), target);
        assert_eq!(dial_path(&target), target);
    }
}
