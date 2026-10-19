//! Process-boundary checks for the double's refusal paths: the real client
//! cannot intentionally send these malformed requests through a dispatch.
// llmlint: ignore-file[tests_mirror_real_usage] the subject is fake-codex itself,
// launched at the same executable/socket boundary as the linked client's turn.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::double;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct World {
    root: PathBuf,
    fakes: PathBuf,
}

impl World {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
        let fakes = root.join("fakes");
        std::fs::create_dir_all(&fakes).unwrap();
        Self { root, fakes }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Server {
    child: Child,
    path: PathBuf,
}

impl Server {
    fn start(world: &World, tag: &str, long: bool) -> Self {
        let path = if long {
            world.root.join("long-socket-path-".repeat(12))
        } else {
            PathBuf::from(format!("/tmp/op-wire-{}-{tag}.sock", std::process::id()))
        };
        let child = Command::new(double())
            .args([
                "app-server",
                "--listen",
                &format!("unix://{}", path.display()),
            ])
            .env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the server double starts");
        let mut server = Self { child, path };
        let until = Instant::now() + Duration::from_secs(10);
        while !server.path.exists() {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "server exited before binding"
            );
            assert!(Instant::now() < until, "server never bound its socket");
            std::thread::sleep(Duration::from_millis(10));
        }
        server
    }

    fn connect(&self) -> UnixStream {
        let path = oneharness_core::io::websocket::dial_path(&self.path);
        let until = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(&path) {
                Ok(stream) => break stream,
                Err(error)
                    if Instant::now() < until
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                        ) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("the server accepts no client: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }

    fn websocket(&self) -> UnixStream {
        let mut stream = self.connect();
        stream.write_all(UPGRADE.as_bytes()).unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
        }
        assert!(response.starts_with(b"HTTP/1.1 101 "), "{response:?}");
        stream
    }

    fn exited(&mut self, limit: Duration, refusal: Option<&str>) -> String {
        let until = Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < until, "server did not exit: {refusal:?}");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut stderr = String::new();
        self.child
            .stderr
            .as_mut()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        match refusal {
            Some(reason) => {
                assert!(!status.success(), "accepted {reason}: {stderr}");
                assert!(stderr.contains(reason), "wanted {reason}: {stderr}");
            }
            None => assert!(status.success(), "{status}: {stderr}"),
        }
        stderr
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Only this guard's own child, including on an assertion failure.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let target = oneharness_core::io::websocket::dial_path(&self.path);
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_file(&self.path);
    }
}

const UPGRADE: &str = "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";

fn frame(opcode: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![opcode];
    if bytes.len() < 126 {
        frame.push(0x80 | bytes.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&[0; 4]);
    frame.extend_from_slice(bytes);
    frame
}

fn send(stream: &mut UnixStream, message: Value) {
    stream
        .write_all(&frame(0x81, message.to_string().as_bytes()))
        .unwrap();
}

fn received(stream: &mut UnixStream) -> (u8, Vec<u8>) {
    let mut header = [0; 2];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(header[1] & 0x80, 0);
    let len = if header[1] == 126 {
        let mut extended = [0; 2];
        stream.read_exact(&mut extended).unwrap();
        usize::from(u16::from_be_bytes(extended))
    } else {
        usize::from(header[1])
    };
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).unwrap();
    (header[0] & 15, bytes)
}

#[test]
fn codex_server_double_refuses_malformed_upgrades_frames_and_envelopes() {
    let world = World::new("codex-server-refusals");
    for (index, (request, reason)) in [
        (
            UPGRADE.replace("GET /", "POST /"),
            "not a WebSocket upgrade",
        ),
        (
            UPGRADE.replace("Upgrade: websocket\r\n", ""),
            "missing a required header",
        ),
        (
            UPGRADE.replace("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n", ""),
            "carries no Sec-WebSocket-Key",
        ),
        (
            UPGRADE.replace("dGhlIHNhbXBsZSBub25jZQ==", "invalid"),
            "not 16 bytes of base64",
        ),
        ("GET / HTTP/1.1\n".into(), "unterminated"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut server = Server::start(&world, &format!("upgrade-{index}"), false);
        server.connect().write_all(request.as_bytes()).unwrap();
        server.exited(Duration::from_secs(5), Some(reason));
    }
    for (index, (bytes, reason)) in [
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"thread/resume","params":{"threadId":"unloaded","cwd":"/","approvalPolicy":"never","typo":true}}"#), "unknown field"),
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{},"typo":"wire-secret-canary"}"#), "unknown field"),
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"thread/unsubscribe","params":{"threadId":""}}"#), "empty thread identity"),
        (vec![0x81, 0], "unmasked"),
        (vec![0xc1, 0x80], "reserved bits"),
        (frame(0x80, b""), "continues no message"),
        (frame(0x82, b""), "opcode"),
        (frame(0x09, b""), "fragmented or past 125"),
        (frame(0x89, &[0; 126]), "fragmented or past 125"),
        (frame(0x88, &[0]), "one byte"),
        (frame(0x88, &1005u16.to_be_bytes()), "no endpoint sends"),
        (frame(0x88, &[3, 232, 255]), "not UTF-8"),
        (vec![0x81, 0xfe, 0, 1], "fits in 7 bits"),
        (vec![0x81, 0xff, 0, 0, 0, 0, 0, 0, 0, 1], "not minimal"),
        (vec![0x81, 0xff, 0, 0, 0, 0, 1, 0, 0, 1], "message past"),
        (frame(0x81, b"not json"), "not a JSON-RPC"),
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}"#), "not a request"),
        (frame(0x81, br#"{"jsonrpc":"2.0","method":"unknown","params":{}}"#), "not a notification"),
        (frame(0x81, br#"{"jsonrpc":"2.0","method":"initialized","params":{"typo":true}}"#), "unknown field"),
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{},"typo":true}"#), "unknown field"),
        (frame(0x81, br#"{"jsonrpc":"2.0","id":1,"method":"thread/start","params":{"cwd":"/","approvalPolicy":"never","typo":true}}"#), "unknown field"),
    ].into_iter().enumerate() {
        let mut server = Server::start(&world, &format!("frame-{index}"), false);
        let mut stream = server.websocket();
        stream.write_all(&bytes).unwrap();
        let stderr = server.exited(Duration::from_secs(5), Some(reason));
        assert!(!stderr.contains("wire-secret-canary"), "a refused request leaked its payload");
    }
}

fn open_thread(stream: &mut UnixStream) -> Value {
    send(
        stream,
        json!({"jsonrpc":"2.0","id":1,"method":"thread/start","params":{"cwd":"/","approvalPolicy":"never"}}),
    );
    let (_, bytes) = received(stream);
    serde_json::from_slice::<Value>(&bytes).unwrap()["result"]["thread"]["id"].clone()
}

fn turn(thread: Value, input: Value) -> Value {
    json!({"jsonrpc":"2.0","id":2,"method":"turn/start","params":{"threadId":thread,"cwd":"/","approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"},"input":input}})
}

#[test]
fn codex_server_double_checks_thread_membership_input_and_await_script() {
    let world = World::new("codex-server-turn-refusals");
    let script = world
        .fakes
        .join(onepipeline_testfakes::codex_app_server::AWAIT_TURNS_FILE);
    for (index, reason) in [
        "no client opened",
        "carries no input",
        "holds no turn count",
        "cannot read",
    ]
    .into_iter()
    .enumerate()
    {
        let mut server = Server::start(&world, &format!("turn-{index}"), false);
        let mut stream = server.websocket();
        let thread = open_thread(&mut stream);
        let input = json!([{"type":"text","text":"hello"}]);
        let request = match index {
            0 => turn(json!("unopened"), input),
            1 => turn(thread, json!([])),
            2 => {
                std::fs::write(&script, "not-a-count").unwrap();
                turn(thread, input)
            }
            _ => {
                std::fs::remove_file(&script).unwrap();
                std::fs::create_dir(&script).unwrap();
                turn(thread, input)
            }
        };
        send(&mut stream, request);
        server.exited(Duration::from_secs(5), Some(reason));
    }
}

#[test]
fn codex_server_double_supports_fragmentation_ping_long_paths_and_idle_shutdown() {
    let world = World::new("codex-server-control");
    let mut server = Server::start(&world, "control", true);
    assert!(server.path.is_symlink());
    let mut stream = server.websocket();
    stream.write_all(&frame(0x89, b"ping")).unwrap();
    assert_eq!(received(&mut stream), (0xa, b"ping".to_vec()));
    stream.write_all(&frame(0x8a, b"pong")).unwrap();
    let init = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"wire-test","title":"test","version":"1"}}}"#;
    stream.write_all(&frame(0x01, &init[..20])).unwrap();
    stream.write_all(&frame(0x80, &init[20..])).unwrap();
    let (_, reply) = received(&mut stream);
    assert!(
        serde_json::from_slice::<Value>(&reply).unwrap()["result"]["userAgent"]
            .as_str()
            .unwrap()
            .contains("wire-test")
    );
    // No await script means the first turn completes without a second caller.
    let thread = open_thread(&mut stream);
    send(
        &mut stream,
        turn(thread, json!([{"type":"text","text":"hello"}])),
    );
    for _ in 0..4 {
        received(&mut stream);
    }
    stream
        .write_all(&frame(0x88, &1000u16.to_be_bytes()))
        .unwrap();
    drop(stream);
    server.exited(Duration::from_secs(10), None);
}

#[test]
fn codex_server_double_bounds_unused_servers_and_unmet_concurrency() {
    let world = World::new("codex-server-backstops");
    let mut unused = Server::start(&world, "unused", false);
    let mut waiting = Server::start(&world, "waiting", false);
    std::fs::write(
        world
            .fakes
            .join(onepipeline_testfakes::codex_app_server::AWAIT_TURNS_FILE),
        "2",
    )
    .unwrap();
    let mut stream = waiting.websocket();
    let thread = open_thread(&mut stream);
    send(
        &mut stream,
        turn(thread, json!([{"type":"text","text":"hello"}])),
    );
    // The double's bounded wait is part of the two-server regression proof:
    // a lone turn eventually answers with the fact that it saw only itself.
    stream
        .set_read_timeout(Some(Duration::from_secs(75)))
        .unwrap();
    for _ in 0..4 {
        received(&mut stream);
    }
    let record = std::fs::read_to_string(
        world
            .fakes
            .join(onepipeline_testfakes::codex_app_server::TURNS_FILE),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(record.trim()).unwrap()["turns_started"],
        1
    );
    unused.exited(Duration::from_secs(10), None);
    drop(stream);
    waiting.exited(Duration::from_secs(10), None);
}

#[test]
fn codex_server_double_resumes_an_unloaded_thread_on_another_connection() {
    use onepipeline_testfakes::codex_app_server::{RESUMES_FILE, TURNS_FILE};
    let world = World::new("codex-server-resume");
    let mut server = Server::start(&world, "resume", false);
    let mut first = server.websocket();
    let thread = open_thread(&mut first);
    send(
        &mut first,
        json!({"jsonrpc":"2.0","id":2,"method":"thread/unsubscribe","params":{"threadId":thread}}),
    );
    let (_, reply) = received(&mut first);
    assert_eq!(
        serde_json::from_slice::<Value>(&reply).unwrap()["result"]["status"],
        "unsubscribed"
    );
    first.write_all(&frame(0x88, &[])).unwrap();
    drop(first);

    let mut next = server.websocket();
    send(
        &mut next,
        json!({"jsonrpc":"2.0","id":3,"method":"thread/resume","params":{
            "threadId":thread,"cwd":"/","approvalPolicy":"never","model":"recording-model",
            "config":{"shell_environment_policy":{"set":{"WIRE":"value"},"exclude":["UNUSED"]}}
        }}),
    );
    let (_, reply) = received(&mut next);
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["result"]["thread"]["id"], thread);
    assert_eq!(reply["result"]["model"], "recording-model");
    send(
        &mut next,
        turn(thread.clone(), json!([{"type":"text","text":"continue"}])),
    );
    for _ in 0..4 {
        received(&mut next);
    }
    for file in [RESUMES_FILE, TURNS_FILE] {
        let record: Value = serde_json::from_str(
            std::fs::read_to_string(world.fakes.join(file))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(record["thread"], thread);
        assert_eq!(record["server_pid"], server.child.id());
    }
    next.write_all(&frame(0x88, &[])).unwrap();
    drop(next);
    server.exited(Duration::from_secs(10), None);
}
