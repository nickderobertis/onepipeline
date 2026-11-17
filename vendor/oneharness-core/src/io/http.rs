//! The socket half of an HTTP-submitted turn.
//!
//! A tiny HTTP/1.1 client rather than a dependency: `oneharness-core` depends
//! only on serde/toml/thiserror/which/wait-timeout by design (see AGENTS.md),
//! and what these two servers need is one request/response and one
//! server-sent-events stream over loopback TCP or a unix socket. All framing
//! and every route lives in [`crate::domain::http`]; this module only moves
//! bytes.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::domain::control::DialAddress;
#[cfg(test)]
use crate::domain::http::Method;
use crate::domain::http::{
    parse_head, BodyFraming, ChunkedDecoder, HeadScan, HeadUnreadable, HttpRequest, SseAccumulator,
    StatusCode, MAX_BODY_BYTES,
};

/// A server's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub body: String,
}

impl HttpResponse {
    /// Whether the server accepted the request — see [`StatusCode::is_success`],
    /// which is where that question is decided for every reader.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status.is_success()
    }
}

/// A connection to one control server, dialed fresh per request (both servers
/// answer `Connection: close`, and a pooled socket buys nothing at this rate).
///
/// Addressed by a [`DialAddress`], not the general
/// [`ServerAddress`](crate::domain::control::ServerAddress): a stdio
/// server is reached over pipes its parent already holds, so there is no
/// address for this client to connect to. Narrowing at construction turns
/// "an HTTP client for a stdio server" from a dial-time error into a value
/// nobody can build.
#[derive(Debug, Clone)]
pub struct HttpClient {
    address: DialAddress,
    timeout: Duration,
}

impl HttpClient {
    #[must_use]
    pub fn new(address: DialAddress, timeout: Duration) -> Self {
        HttpClient { address, timeout }
    }

    /// Send `request` and read the whole answer.
    ///
    /// The answer ends where its own framing says it does — the terminating
    /// chunk, or `Content-Length` bytes of body — and only falls back to reading
    /// until EOF when the server framed it neither way. Waiting for the close
    /// instead is what a `Connection: close` request seems to promise, and
    /// opencode does not keep that promise on every route: it answers in full
    /// and leaves the socket open, so a reader that waits for EOF times out and
    /// reports a complete answer as cut short. A read that fails — or a close
    /// that arrives — *before* the declared framing is satisfied really is a
    /// truncated answer, and a truncated body parsed as JSON is a wrong answer
    /// rather than a missing one.
    pub fn send(&self, request: &HttpRequest) -> io::Result<HttpResponse> {
        let mut stream = self.dial(self.timeout)?;
        write_request(&mut stream, request, &self.address)?;
        let mut pending = Vec::new();
        let mut head: Option<crate::domain::http::ResponseHead> = None;
        let mut chunks = ChunkedDecoder::default();
        let mut body: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            // Framing the peer broke is decided here, before the next blocking
            // read, because it is decided the same way wherever the bytes came
            // from — the head's own trailing bytes or a later arrival — and
            // nothing that arrives later makes it readable. Waiting instead
            // spends the whole timeout and reports a malformed answer as a slow
            // one.
            if chunks.malformed() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the control server's answer cannot be read: it declared \
                     Transfer-Encoding: chunked and then sent something that is not chunk framing",
                ));
            }
            // Completeness is decided BEFORE the next blocking read, so a whole
            // answer never costs a read timeout.
            if let Some(head) = head {
                match head.framing {
                    BodyFraming::Chunked if chunks.is_complete() => break,
                    BodyFraming::Length(length) if body.len() >= length => {
                        body.truncate(length);
                        break;
                    }
                    _ => {}
                }
            }
            let read = stream.read(&mut buffer).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("the control server's answer was cut short: {err}"),
                )
            })?;
            if read == 0 {
                // The close. A server that framed its answer and then hung up
                // before delivering it cut the answer short just as surely as a
                // failing read did, and half a JSON document is a wrong answer
                // rather than a missing one. Only an answer the server framed
                // neither way legitimately ends here.
                if head.is_some_and(|head| head.framing.ends_before_close()) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the control server's answer was cut short: it closed before the body \
                         it declared had arrived",
                    ));
                }
                break;
            }
            let bytes = &buffer[..read];
            match head {
                Some(head) => {
                    absorb(bytes, head.framing.is_chunked(), &mut chunks, &mut body);
                    // Both ways a framed-looking answer keeps costing memory:
                    // chunk framing that never terminates, and a body (chunked
                    // or unframed) that simply never stops. Neither ends on its
                    // own, so the read does.
                    if chunks.overflowed() || body.len() > MAX_BODY_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "the control server's answer passed the {} bytes a reader will \
                                 hold without ending",
                                MAX_BODY_BYTES
                            ),
                        ));
                    }
                }
                None => {
                    pending.extend_from_slice(bytes);
                    match parse_head(&pending) {
                        HeadScan::Incomplete => {}
                        HeadScan::Whole(parsed) => {
                            let rest = pending.split_off(parsed.body_at);
                            absorb(&rest, parsed.framing.is_chunked(), &mut chunks, &mut body);
                            head = Some(parsed);
                        }
                        // Refused where it arrives rather than read on: the
                        // head is what says how much of this connection is the
                        // answer, so anything read past an unreadable one is a
                        // guess dressed up as a body.
                        HeadScan::Unreadable(why) => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("the control server's answer cannot be read: {why}"),
                            ))
                        }
                    }
                }
            }
        }
        let Some(head) = head else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the control server answered something that is not an HTTP response",
            ));
        };
        Ok(HttpResponse {
            status: head.status,
            body: String::from_utf8_lossy(&body).to_string(),
        })
    }

    /// Open `request` as a server-sent-events stream, read one payload at a
    /// time. The stream's own timeout bounds a server that goes quiet.
    pub fn open_stream(&self, request: &HttpRequest, timeout: Duration) -> io::Result<EventStream> {
        let mut stream = self.dial(timeout)?;
        write_request(&mut stream, request, &self.address)?;
        Ok(EventStream {
            stream,
            head: StreamHead::Arriving(Vec::new()),
            chunks: ChunkedDecoder::default(),
            sse: SseAccumulator::default(),
            ready: Vec::new(),
        })
    }

    fn dial(&self, timeout: Duration) -> io::Result<Socket> {
        match &self.address {
            DialAddress::Tcp { port } => {
                let stream = TcpStream::connect(("127.0.0.1", port.get()))?;
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                Ok(Socket::Tcp(stream))
            }
            #[cfg(unix)]
            DialAddress::UnixSocket { path } => {
                let stream = std::os::unix::net::UnixStream::connect(path.as_path())?;
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                Ok(Socket::Unix(stream))
            }
            #[cfg(not(unix))]
            DialAddress::UnixSocket { .. } => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "unix-socket control servers are not available on this platform",
            )),
        }
    }
}

/// Add `bytes` to a response body, de-chunking on the way in when the server
/// framed it that way. Shared by the one-shot reader and nothing else; the
/// event stream keeps its own accumulator because it also splits SSE payloads.
fn absorb(bytes: &[u8], chunked: bool, chunks: &mut ChunkedDecoder, body: &mut Vec<u8>) {
    if chunked {
        let decoded = chunks.feed(bytes);
        body.extend_from_slice(&decoded);
    } else {
        body.extend_from_slice(bytes);
    }
}

/// Where a stream is in its own framing.
///
/// One value rather than a seen/status/chunked triple, so a reader can never be
/// mid-head and already framed at the same time: until the head is whole the
/// only thing that exists is the bytes it is being assembled from, and once it
/// is whole the only thing that outlives it is the body framing it declared.
/// A refused subscription never reaches [`StreamHead::Framed`] at all —
/// [`EventStream::poll`] answers [`StreamPoll::Refused`] while still arriving,
/// because a server that said `404` still sends a body and reading that body as
/// events would report a turn that never streamed.
enum StreamHead {
    /// The head is still arriving; these are its bytes so far.
    Arriving(Vec<u8>),
    /// The head arrived naming a success status, and framed the body this way.
    Framed(BodyFraming),
}

/// A live event stream, yielding one `data:` payload at a time.
pub struct EventStream {
    stream: Socket,
    head: StreamHead,
    chunks: ChunkedDecoder,
    sse: SseAccumulator,
    ready: Vec<String>,
}

/// What one poll of an event stream found.
///
/// Quiet and closed are deliberately different answers: a turn that is thinking
/// produces no events for many seconds, and treating that as the end of the
/// stream would end the run while the agent was still working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPoll {
    Event(String),
    /// Nothing arrived within the read timeout; the stream is still open.
    Idle,
    /// The server answered the subscription with a non-success status, so its
    /// body is an error document rather than a stream of events.
    Refused(StatusCode),
    /// The subscription cannot be read, so nothing after it can be framed as an
    /// event. Its own answer, not a close: the socket is still open and a
    /// caller that read this as quiet would wait out its timeout on a stream
    /// that will never yield a payload.
    Unreadable(StreamUnreadable),
    /// The server closed the stream.
    Closed,
}

/// Why a stream will yield no payload.
///
/// Three reasons rather than one, because a stream can fail anywhere in its
/// framing: the head it started with, the chunk framing that head declared, or
/// an event that never ends. All three are the peer's doing and none is a
/// close, so all three are said out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamUnreadable {
    /// The head cannot be framed, so nothing after it can be either.
    Head(HeadUnreadable),
    /// The head declared `Transfer-Encoding: chunked` and the body is not
    /// chunk-framed. Reading it as an unframed body instead would splice the
    /// peer's framing bytes into the events a caller acts on.
    Framing,
    /// One event grew past the bytes a reader will hold without ending. Not a
    /// pause in a long turn: nothing that arrives later makes it readable.
    Payload(usize),
}

impl std::fmt::Display for StreamUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamUnreadable::Head(why) => write!(f, "{why}"),
            StreamUnreadable::Framing => write!(
                f,
                "it declared Transfer-Encoding: chunked and then sent something that is not \
                 chunk framing"
            ),
            StreamUnreadable::Payload(bound) => {
                write!(
                    f,
                    "one event passed the {bound} bytes a reader will hold without ending"
                )
            }
        }
    }
}

impl EventStream {
    /// The next event payload, or why there is none yet.
    pub fn poll(&mut self) -> StreamPoll {
        loop {
            if !self.ready.is_empty() {
                return StreamPoll::Event(self.ready.remove(0));
            }
            let mut buffer = [0u8; 8192];
            let read = match self.stream.read(&mut buffer) {
                Ok(read) => read,
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return StreamPoll::Idle
                }
                Err(_) => return StreamPoll::Closed,
            };
            if read == 0 {
                return StreamPoll::Closed;
            }
            let bytes = &buffer[..read];
            if let StreamHead::Arriving(pending) = &mut self.head {
                pending.extend_from_slice(bytes);
                let head = match parse_head(pending) {
                    HeadScan::Incomplete => continue,
                    HeadScan::Whole(head) => head,
                    HeadScan::Unreadable(why) => {
                        return StreamPoll::Unreadable(StreamUnreadable::Head(why))
                    }
                };
                if !head.status.is_success() {
                    return StreamPoll::Refused(head.status);
                }
                let body: Vec<u8> = pending.split_off(head.body_at);
                self.head = StreamHead::Framed(head.framing);
                self.absorb(&body);
                if let Some(unreadable) = self.unreadable() {
                    return unreadable;
                }
                continue;
            }
            self.absorb(bytes);
            if let Some(unreadable) = self.unreadable() {
                return unreadable;
            }
        }
    }

    /// The answer for a stream the peer's own framing has made unreadable, or
    /// `None` while it is still readable. A long turn is quiet, never endless:
    /// the accumulators only overflow on framing that has no ending in it, and
    /// broken chunk framing is not an arrival to wait on at all.
    fn unreadable(&self) -> Option<StreamPoll> {
        if self.chunks.malformed() {
            return Some(StreamPoll::Unreadable(StreamUnreadable::Framing));
        }
        (self.chunks.overflowed() || self.sse.overflowed()).then_some(StreamPoll::Unreadable(
            StreamUnreadable::Payload(MAX_BODY_BYTES),
        ))
    }

    fn absorb(&mut self, bytes: &[u8]) {
        let decoded = match self.head {
            StreamHead::Framed(framing) if framing.is_chunked() => self.chunks.feed(bytes),
            // Nothing is absorbed before the head is framed — `poll` only ever
            // calls this with the body it split off a whole head, or later.
            _ => bytes.to_vec(),
        };
        self.ready.extend(self.sse.feed(&decoded));
    }
}

/// The two socket families a control server binds. An enum rather than a boxed
/// trait object so the read timeout each one needs stays on the concrete type.
enum Socket {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(stream) => stream.read(buf),
            #[cfg(unix)]
            Socket::Unix(stream) => stream.read(buf),
        }
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(stream) => stream.write(buf),
            #[cfg(unix)]
            Socket::Unix(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Socket::Tcp(stream) => stream.flush(),
            #[cfg(unix)]
            Socket::Unix(stream) => stream.flush(),
        }
    }
}

fn write_request(
    stream: &mut Socket,
    request: &HttpRequest,
    address: &DialAddress,
) -> io::Result<()> {
    let host = match address {
        DialAddress::Tcp { port } => format!("127.0.0.1:{}", port.get()),
        DialAddress::UnixSocket { .. } => "localhost".to_string(),
    };
    let body = request.body().unwrap_or_default().to_string();
    let head = format!(
        "{} {} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream, application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        request.method().as_str(),
        request.path(),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::control::Port;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// A one-connection HTTP server that answers `answer` verbatim, so the
    /// client is exercised against real socket reads rather than a fake.
    fn serve_once(answer: &'static [u8]) -> (Port, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = Port::new(listener.local_addr().unwrap().port()).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = io::BufReader::new(socket.try_clone().unwrap());
            let mut request = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    // A declared length this double cannot read is a framing
                    // error, not an empty body: reading none of a body that is
                    // there would hand the test a truncated request.
                    length = value
                        .trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("unreadable Content-Length: {value}"));
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            request.push_str(&String::from_utf8_lossy(&body));
            socket.write_all(answer).unwrap();
            socket.flush().unwrap();
            request
        });
        (port, handle)
    }

    fn client(port: Port) -> HttpClient {
        HttpClient::new(DialAddress::Tcp { port }, Duration::from_secs(5))
    }

    #[test]
    fn a_chunked_answer_reaches_the_caller_as_its_body() {
        // The framing both control servers use: without de-chunking this is a
        // JSON decode error on an answer the server got right.
        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n{\"id\":\"ses_01\"}\r\n\r\n0\r\n\r\n",
        );
        let response = client(port)
            .send(&HttpRequest::for_test(
                Method::Post,
                "/api/session",
                Some("{}"),
            ))
            .unwrap();
        assert_eq!(response.status.get(), 200);
        assert_eq!(response.body.trim(), r#"{"id":"ses_01"}"#);
        assert!(response.ok());

        // The request the server actually received: method, path and body.
        let request = server.join().unwrap();
        assert!(
            request.starts_with("POST /api/session HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Content-Length: 2\r\n"), "{request}");
        assert!(request.ends_with("{}"), "{request}");
    }

    /// A server that answers in full and then *keeps the connection open*,
    /// ignoring the `Connection: close` the request asked for — which is what
    /// `opencode serve` does on the readiness route. It holds the socket until
    /// the CLIENT closes it, so a reader waiting for EOF waits out its whole
    /// timeout and cannot pass by luck.
    fn serve_once_without_closing(answer: &'static [u8]) -> (Port, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = Port::new(listener.local_addr().unwrap().port()).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = io::BufReader::new(socket.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            socket.write_all(answer).unwrap();
            socket.flush().unwrap();
            // Never closes; only the client hanging up releases this.
            let mut drain = Vec::new();
            let _ = reader.read_to_end(&mut drain);
        });
        (port, handle)
    }

    #[test]
    fn a_complete_answer_is_returned_without_waiting_for_the_server_to_close() {
        // Content-Length framing: the whole body is here, so the answer is
        // whole — whatever the server then does with the socket.
        let (port, server) = serve_once_without_closing(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"id\":\"ses_01\"}",
        );
        let started = std::time::Instant::now();
        let response = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap();
        assert_eq!(response.status.get(), 200);
        assert_eq!(response.body, r#"{"id":"ses_01"}"#);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the answer was framed as complete, so reading it must not cost a read timeout"
        );
        server.join().unwrap();

        // The same for chunked framing, which ends at its terminating chunk.
        let (port, server) = serve_once_without_closing(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"a\":1}\r\n0\r\n\r\n",
        );
        let started = std::time::Instant::now();
        let response = client(port)
            .send(&HttpRequest::for_test(Method::Post, "/api/session", None))
            .unwrap();
        assert_eq!(response.body, r#"{"a":1}"#);
        assert!(started.elapsed() < Duration::from_secs(5));
        server.join().unwrap();
    }

    #[test]
    fn a_refusal_is_data_the_caller_can_act_on_rather_than_an_error() {
        let (port, server) = serve_once(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 21\r\n\r\n404 page not found\r\n\r\n",
        );
        let response = client(port)
            .send(&HttpRequest::for_test(
                Method::Post,
                "/v1/workspaces/x/agent/sessions",
                None,
            ))
            .unwrap();
        assert_eq!(response.status.get(), 404);
        assert!(!response.ok());
        assert!(response.body.contains("404 page not found"));
        let _ = server.join();
    }

    #[test]
    fn a_redirect_this_client_never_follows_is_not_an_accepted_request() {
        // Nothing here follows a `Location`, so a route that answered `3xx` did
        // not do what was asked — it said where to ask instead. Counted as
        // accepted, an interrupt redirected away would report `served` while
        // the turn ran on, which is the one answer worse than a refusal.
        let (port, server) = serve_once(
            b"HTTP/1.1 302 Found\r\nLocation: /v2/session/ses_01/interrupt\r\nContent-Length: 0\r\n\r\n",
        );
        let response = client(port)
            .send(&HttpRequest::for_test(
                Method::Post,
                "/api/session/ses_01/interrupt",
                None,
            ))
            .unwrap();
        assert_eq!(response.status.get(), 302);
        assert!(!response.ok(), "a redirect nobody follows is not a success");
        let _ = server.join();
    }

    #[test]
    fn an_answer_the_server_closed_before_finishing_is_an_error_rather_than_half_a_body() {
        // A control server that dies mid-answer (or a proxy that drops it)
        // leaves a body its own head says is incomplete. Returning it would
        // hand the caller half a JSON document to parse — a wrong answer,
        // where the error is a missing one the caller can retry.
        let (port, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"id\":\"ses");
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(err.to_string().contains("cut short"), "{err}");
        let _ = server.join();

        // The same for chunked framing, whose terminating chunk never arrives.
        let (port, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"a\":1}\r\n");
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Post, "/api/session", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let _ = server.join();

        // An answer the server framed NEITHER way still ends at the close:
        // that is the one shape whose only end-of-body signal is the close.
        let (port, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}");
        let response = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap();
        assert_eq!(response.body, r#"{"a":1}"#);
        let _ = server.join();
    }

    #[test]
    fn an_answer_whose_framing_cannot_be_read_is_refused_rather_than_read_to_the_close() {
        // The server declares a length no reader can frame a body from, then
        // keeps writing. Read as unframed, the answer would end at the close
        // and hand back the declared body WITH the trailer appended — a JSON
        // document that parses, which is a wrong answer rather than a loud one.
        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: banana\r\n\r\n{\"id\":\"ses_01\"}TRAILING",
        );
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Post, "/api/session", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Content-Length"), "{err}");
        let _ = server.join();

        // The same for a head whose first line is not a status line at all,
        // which is otherwise indistinguishable from a head still arriving.
        let (port, server) = serve_once(b"NOT-HTTP\r\nContent-Type: application/json\r\n\r\n{}");
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = server.join();
    }

    #[test]
    fn an_event_subscription_whose_head_cannot_be_read_says_so_rather_than_going_quiet() {
        // Nothing after an unreadable head can be framed as an event, so the
        // stream reports why. Read as idle, the turn would wait out its whole
        // timeout on a subscription that will never yield a payload.
        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 7\r\nContent-Length: 9\r\n\r\ndata: {}\n",
        );
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Unreadable(StreamUnreadable::Head(HeadUnreadable::ConflictingLengths(
                7, 9
            )))
        );
        let _ = server.join();
    }

    #[test]
    fn an_answer_with_two_body_framings_is_refused_at_the_socket_boundary() {
        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 7\r\n\r\n7\r\n{\"a\":1}\r\n0\r\n\r\n",
        );
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("both Transfer-Encoding: chunked and Content-Length"),
            "{err}"
        );
        let _ = server.join();

        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 9\r\n\r\n7\r\ndata: x\r\n0\r\n\r\n",
        );
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Unreadable(StreamUnreadable::Head(HeadUnreadable::AmbiguousFraming))
        );
        let _ = server.join();
    }

    /// A server that answers `head` and then streams `filler` forever, never
    /// ending and never closing — the shape a peer takes when what it costs the
    /// reader is unbounded rather than merely slow.
    fn serve_endlessly(head: &'static [u8], filler: u8) -> Port {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = Port::new(listener.local_addr().unwrap().port()).unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = io::BufReader::new(socket.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            if socket.write_all(head).is_err() {
                return;
            }
            let blob = vec![filler; 64 * 1024];
            // Bounded only so a regression fails instead of running forever.
            for _ in 0..1024 {
                if socket.write_all(&blob).is_err() {
                    return;
                }
            }
        });
        port
    }

    #[test]
    fn an_answer_that_never_ends_is_refused_instead_of_held() {
        // An unframed body is the one shape whose only ending is the close, so
        // a peer that never closes would otherwise be read until it stopped —
        // it decides the memory, and the reader just waits.
        let port = serve_endlessly(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
            b'x',
        );
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("without ending"), "{err}");

        // The same for chunk framing whose size header never arrives: valid so
        // far, terminating never.
        let port = serve_endlessly(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            b'7',
        );
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Post, "/api/session", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_subscription_whose_head_never_ends_says_so_rather_than_going_quiet() {
        // A head with no blank line in it: read as "still arriving", the turn
        // waits out its whole timeout while the stream keeps growing.
        let port = serve_endlessly(b"HTTP/1.1 200 OK\r\nX-Pad: ", b'p');
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Unreadable(StreamUnreadable::Head(HeadUnreadable::OversizedHead(
                crate::domain::http::MAX_HEAD_BYTES
            )))
        );

        // And one event that never ends is its own answer, not a quiet turn.
        let port = serve_endlessly(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: ",
            b'z',
        );
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Unreadable(StreamUnreadable::Payload(MAX_BODY_BYTES))
        );
    }

    #[test]
    fn an_answer_declared_chunked_that_is_not_chunk_framed_is_refused_not_reinterpreted() {
        // The peer declared `Transfer-Encoding: chunked` and then sent a bare
        // JSON body. Reading it as an unframed body accepts a shape the server
        // said it was not sending — and the same reader would then splice a
        // real answer's size lines and terminators into the caller's data.
        // The server never closes, so nothing here can pass by reaching EOF.
        let (port, server) = serve_once_without_closing(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{\"id\":\"ses_01\"}\r\n",
        );
        let started = std::time::Instant::now();
        let err = client(port)
            .send(&HttpRequest::for_test(
                Method::Post,
                "/api/session",
                Some("{}"),
            ))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("not chunk framing"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "broken framing is not an arrival to wait on"
        );
        drop(server);
    }

    #[test]
    fn a_subscription_whose_chunk_framing_is_broken_says_so_rather_than_going_quiet() {
        // The streaming half of the same contradiction: a caller that read it
        // as quiet would wait out its timeout on a stream that will never
        // yield a payload, and one that read the raw bytes would act on events
        // it never actually received whole.
        let (port, server) = serve_once_without_closing(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\
              \r\n\r\ndata: {\"type\":\"session.idle\"}\r\n\r\n",
        );
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Unreadable(StreamUnreadable::Framing)
        );
        assert!(StreamUnreadable::Framing
            .to_string()
            .contains("not chunk framing"));
        drop(server);
    }

    #[test]
    fn an_answer_that_is_not_http_is_an_error_rather_than_an_empty_body() {
        let (port, server) = serve_once(b"this is not a response");
        let err = client(port)
            .send(&HttpRequest::for_test(Method::Get, "/api/app", None))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = server.join();
    }

    #[test]
    fn an_event_stream_yields_each_payload_as_it_arrives_and_ends_at_close() {
        let (port, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n\
              1f\r\ndata: {\"type\":\"session.idle\"}\n\r\n\
              13\r\ndata: {\"a\":true}\n\r\n0\r\n\r\n",
        );
        let mut stream = client(port)
            .open_stream(
                &HttpRequest::for_test(Method::Get, "/api/event", None),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            stream.poll(),
            StreamPoll::Event(r#"{"type":"session.idle"}"#.to_string())
        );
        assert_eq!(
            stream.poll(),
            StreamPoll::Event(r#"{"a":true}"#.to_string())
        );
        assert_eq!(stream.poll(), StreamPoll::Closed);
        let _ = server.join();
    }
}
