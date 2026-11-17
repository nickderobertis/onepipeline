//! The shared Codex app-server's pure rules: which environment the server is
//! launched with, what keys its pool entry, how a dispatch's own environment
//! reaches its thread's commands, and which frames on a shared connection are a
//! dispatch's own.
//!
//! A shared server's environment reaches every thread's commands, whichever
//! dispatch they belong to, so it is launched with a scrubbed one: the
//! platform's base variables plus [`CODEX_SERVER_ENV`] as the launching
//! dispatch's variant environment resolves them, and [`MOCK_HARNESS_VAR`] when
//! the dispatch carries it. Everything else a dispatch
//! carries travels per thread, in `thread/start`'s and `thread/resume`'s
//! `config` map as `shell_environment_policy.set`, and a server variable the
//! dispatch lacks travels as `shell_environment_policy.exclude` — so its
//! thread's commands see exactly its environment, never another dispatch's.
//!
//! No I/O here: the caller hands in the environments it read and the version
//! output it probed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::domain::config::{
    APP_SERVER_BASE_ENV_UNIX, APP_SERVER_BASE_ENV_UNIX_PREFIX, APP_SERVER_BASE_ENV_WINDOWS,
    CODEX_SERVER_ENV, CODEX_SERVER_ENV_PREFIX,
};
use crate::domain::control::{pool_key as control_pool_key, PoolKey};

/// The variable naming a dispatch's own commands: a fresh UUID per dispatch,
/// carried in its thread's `shell_environment_policy.set`, so every process its
/// turn starts can be found and stopped by it.
pub const DISPATCH_MARKER_VAR: &str = "ONEHARNESS_DISPATCH_MARKER";

/// The variable naming a shared server's own processes (the launcher shim and
/// the native server it starts): a fresh UUID per server, recorded in the pool
/// entry's `server.json`.
pub const SERVER_MARKER_VAR: &str = "ONEHARNESS_APP_SERVER_MARKER";

/// The variable that makes the `oneharness` binary answer as the mock harness
/// instead of parsing a command line: `--mock-harness` swaps the binary in for
/// a harness and sets this in each turn's environment. A shared server keeps
/// it, so a mock server starts as the mock; and because the server environment
/// keys the pool, a mock server and a real one never share an entry.
pub const MOCK_HARNESS_VAR: &str = "ONEHARNESS_INTERNAL_MOCK_HARNESS";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Real Codex cannot be read in the deterministic gate; the live phases scripts/e2e-control.sh runs on every live-control leg drive these requests against it — pooled-reuse sends thread/start, thread/shellCommand, thread/resume, thread/unsubscribe and thread/loaded/list and fails naming a released thread still loaded, and pooled-env and pooled-kill drive the thread lifecycle and its environment — which is this contract's drift alarm.
/// The JSON-RPC request that releases a dispatch's thread from the server once
/// its lease ends. Codex keeps a finished thread loaded until its last
/// subscriber unsubscribes (`thread/unsubscribe` in codex-rs's app-server v2
/// protocol, `ThreadUnsubscribeParams { threadId }`), then unloads it and emits
/// `thread/closed` once it has had no subscriber and no activity for the
/// server's `thread_unload_delay_secs` (by default 60 s on codex-cli 0.159.2,
/// 1800 s on 0.163.0).
pub const RELEASE_THREAD_METHOD: &str = "thread/unsubscribe";
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Which platform's environment rules apply: the base variable set, and whether
/// names compare case-insensitively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvPlatform {
    Unix,
    Windows,
}

impl EnvPlatform {
    /// The platform this build runs on. A compile-time fact, not a read.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(windows) {
            EnvPlatform::Windows
        } else {
            EnvPlatform::Unix
        }
    }

    fn same_name(self, a: &str, b: &str) -> bool {
        match self {
            EnvPlatform::Unix => a == b,
            EnvPlatform::Windows => a.eq_ignore_ascii_case(b),
        }
    }

    /// Whether `name` is one of this platform's base variables.
    fn is_base(self, name: &str) -> bool {
        match self {
            EnvPlatform::Unix => {
                APP_SERVER_BASE_ENV_UNIX.contains(&name)
                    || name.starts_with(APP_SERVER_BASE_ENV_UNIX_PREFIX)
            }
            EnvPlatform::Windows => APP_SERVER_BASE_ENV_WINDOWS
                .iter()
                .any(|base| base.eq_ignore_ascii_case(name)),
        }
    }
}

/// Whether a shared server keeps `name` from its launching dispatch's
/// environment: a base variable, one Codex reads for itself, or the mock
/// harness's marker.
#[must_use]
pub fn in_server_env(name: &str, platform: EnvPlatform) -> bool {
    platform.is_base(name)
        || platform.same_name(MOCK_HARNESS_VAR, name)
        || CODEX_SERVER_ENV
            .iter()
            .any(|codex| platform.same_name(codex, name))
        || match platform {
            EnvPlatform::Unix => name.starts_with(CODEX_SERVER_ENV_PREFIX),
            EnvPlatform::Windows => name
                .get(..CODEX_SERVER_ENV_PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(CODEX_SERVER_ENV_PREFIX)),
        }
}

/// The environment a shared server is launched with, from the launching
/// dispatch's resolved one: only the variables [`in_server_env`] keeps, sorted
/// by name so two dispatches that resolved the same values in a different order
/// describe one server. The server marker is added by the launcher, not here —
/// it is fresh per server and must not move the pool key.
#[must_use]
pub fn server_env(
    dispatch_env: &[(String, String)],
    platform: EnvPlatform,
) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = dispatch_env
        .iter()
        .filter(|(name, _)| in_server_env(name, platform))
        .cloned()
        .collect();
    kept.sort();
    kept.dedup_by(|a, b| platform.same_name(&a.0, &b.0));
    kept
}

/// The environment a pool entry's sweeper is started with, from its starting
/// dispatch's: the platform's base variables and `XDG_STATE_HOME`, from which
/// it finds the pool again, and nothing else.
///
/// The sweeper belongs to the pool, as the server does, so it carries nothing
/// that names the dispatch. A launcher that ends a dispatch's tree by a stamp
/// in its environment — `onepipeline` reaps every process carrying the
/// member's `ONEAGENTGRAPH_SCRATCH_DIR` — otherwise ends the sweeper with it
/// and leaves the server, which never carried the stamp, with nobody to stop
/// it.
#[must_use]
pub fn sweeper_env(
    dispatch_env: &[(String, String)],
    platform: EnvPlatform,
) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = dispatch_env
        .iter()
        .filter(|(name, _)| platform.is_base(name) || platform.same_name(name, "XDG_STATE_HOME"))
        .cloned()
        .collect();
    kept.sort();
    kept.dedup_by(|a, b| platform.same_name(&a.0, &b.0));
    kept
}

/// An identity the pool persists and reads back to select processes or name
/// a thread — a server or dispatch marker, a thread or turn id: never empty
/// and never carrying a control character, so a record read back can only
/// name something a process scan or a server could.
macro_rules! note_identity {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("`value` as a ", $what, ", or `None` when it is empty or carries a control character.")]
            #[must_use]
            pub fn new(value: impl Into<String>) -> Option<Self> {
                let value = value.into();
                (!value.is_empty() && !value.chars().any(char::is_control)).then_some(Self(value))
            }

            /// The identity as written.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, String> {
                Self::new(value.clone()).ok_or_else(|| format!("{value:?} is not a {}", $what))
            }
        }

        impl From<$name> for String {
            fn from(identity: $name) -> String {
                identity.0
            }
        }
    };
}

note_identity!(
    /// A server marker's value (`ONEHARNESS_APP_SERVER_MARKER`).
    ServerMarker,
    "server marker"
);
note_identity!(
    /// A dispatch marker's value (`ONEHARNESS_DISPATCH_MARKER`).
    DispatchMarker,
    "dispatch marker"
);
note_identity!(
    /// An app-server thread's id.
    ThreadId,
    "thread id"
);
note_identity!(
    /// An app-server turn's id.
    TurnId,
    "turn id"
);

/// What one dispatch's thread carries so its commands see exactly its own
/// environment on a server launched with another's. Built only by
/// [`Self::for_dispatch`], so its marker is the dispatch's own and the server's
/// is always excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnvironmentPolicy {
    set: BTreeMap<String, String>,
    exclude: Vec<String>,
}

impl ShellEnvironmentPolicy {
    /// Every variable of the dispatch's environment absent from or differing
    /// from the server's, plus its [`DISPATCH_MARKER_VAR`].
    #[must_use]
    pub fn set(&self) -> &BTreeMap<String, String> {
        &self.set
    }

    /// Every server variable the dispatch lacks (or removed through
    /// `unset_env`), the server marker among them.
    #[must_use]
    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    /// The policy for a dispatch whose environment is `dispatch_env`, on a
    /// server whose environment is `server_env` (the marker included), with
    /// `marker` as the dispatch's own marker value.
    #[must_use]
    pub fn for_dispatch(
        dispatch_env: &[(String, String)],
        server_env: &[(String, String)],
        marker: &DispatchMarker,
        platform: EnvPlatform,
    ) -> Self {
        let server_value = |name: &str| {
            server_env
                .iter()
                .find(|(server, _)| platform.same_name(server, name))
                .map(|(_, value)| value.as_str())
        };
        let mut set = BTreeMap::new();
        for (name, value) in dispatch_env {
            if name == DISPATCH_MARKER_VAR || name == SERVER_MARKER_VAR {
                continue;
            }
            if server_value(name) != Some(value.as_str()) {
                set.insert(name.clone(), value.clone());
            }
        }
        set.insert(DISPATCH_MARKER_VAR.to_string(), marker.as_str().to_string());
        let mut exclude: Vec<String> = server_env
            .iter()
            .filter(|(name, _)| {
                name == SERVER_MARKER_VAR
                    || !dispatch_env
                        .iter()
                        .any(|(dispatch, _)| platform.same_name(dispatch, name))
            })
            .map(|(name, _)| name.clone())
            .collect();
        exclude.sort();
        exclude.dedup();
        ShellEnvironmentPolicy { set, exclude }
    }

    /// The `config` map a `thread/start` / `thread/resume` carries it in.
    #[must_use]
    pub fn thread_config(&self) -> Value {
        json!({
            "shell_environment_policy": {
                "set": self.set,
                "exclude": self.exclude,
            }
        })
    }
}

/// Everything fixed at a shared server's start, and so its pool key: the
/// executable actually run and what it says it is, the environment it is
/// launched with, every process-level argument after `app-server`, and the
/// resolved `plugins`. Thread-level settings (cwd, model, sandbox, approval
/// policy, instructions) travel per thread and per turn, so dispatches that
/// differ only in those share one server.
#[must_use]
pub fn pool_key(
    executable: &str,
    version_output: &str,
    server_env: &[(String, String)],
    process_args: &[String],
    plugins: bool,
) -> PoolKey {
    let mut material: Vec<(String, Option<String>)> = vec![
        ("executable".to_string(), Some(executable.to_string())),
        (
            "version".to_string(),
            Some(version_output.trim().to_string()),
        ),
        ("plugins".to_string(), Some(plugins.to_string())),
    ];
    material.extend(
        server_env
            .iter()
            .map(|(name, value)| (format!("env:{name}"), Some(value.clone()))),
    );
    control_pool_key("codex-app-server", &material, process_args)
}

/// The arguments that put a server on the socket the pool picked, with the
/// path still spelled `{address}` for the pool to render.
pub const LISTEN_ARGS: [&str; 2] = ["--listen", "unix://{address}"];

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This is the one in-repo statement of Codex's socket rendezvous: the shared-server launch (`prepare_daemon_dir`) and the mock harness's `daemon_dir` both derive from it, and the live check's expected values are held to it in the gate by `the_live_rendezvous_check_expects_the_rendezvous_the_launch_prepares` (io::app_server), so no copy can drift from it. Where real Codex binds can only be read from a running Codex, which the deterministic gate does not have; that reconciliation is the live codex suite's (`oh_codex_rendezvous_check` in scripts/e2e-codex.sh holds a pooled server's bound socket to this directory and mode), as `oh_codex_feature_flag_order` is for Codex's flag precedence.
/// Codex's socket rendezvous directory for `euid` under the canonical temp
/// directory `tmp`: Codex binds every `--listen unix://` socket there, whatever
/// path it is told, and refuses to start unless it is a directory that user
/// owns at [`CODEX_DAEMON_DIR_MODE`].
pub fn codex_daemon_dir(tmp: &Path, euid: u32) -> PathBuf {
    tmp.join(format!("codex-daemon-{euid}"))
}

/// The one mode Codex accepts on its socket rendezvous directory.
pub const CODEX_DAEMON_DIR_MODE: u32 = 0o700;
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The thread a server frame belongs to — `params.threadId`, or
/// `params.thread.id` on a frame that carries the whole thread — or `None` for
/// a frame scoped to the connection or the server rather than to a thread.
#[must_use]
pub fn frame_thread(frame: &Value) -> Option<&str> {
    let params = frame.get("params")?;
    params.get("threadId").and_then(Value::as_str).or_else(|| {
        params
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
    })
}

/// Whom a server frame is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameScope<'a> {
    /// The connection or the server: no thread named.
    Connection,
    /// The thread it names.
    Thread(&'a str),
    /// A thread field that is present but not a string id: owned by no one.
    Malformed,
}

/// The scope of a server frame, read off `params.threadId` or
/// `params.thread.id`.
#[must_use]
pub fn frame_scope(frame: &Value) -> FrameScope<'_> {
    let Some(params) = frame.get("params") else {
        return FrameScope::Connection;
    };
    let named = params.get("threadId").or_else(|| {
        params
            .get("thread")
            .map(|thread| thread.get("id").unwrap_or(&Value::Null))
    });
    match named {
        None => FrameScope::Connection,
        Some(Value::String(id)) if !id.is_empty() => FrameScope::Thread(id),
        Some(_) => FrameScope::Malformed,
    }
}

/// Whether a frame a dispatch received on its shared connection is its own:
/// one scoped to no thread (a response to its own request, or a connection- or
/// server-wide notice), or one naming its thread. Before the dispatch knows its
/// thread, a frame naming any thread is not yet known to be its own, and a
/// frame whose thread field is malformed is no one's.
#[must_use]
pub fn is_own_frame(frame: &Value, own_thread: Option<&str>) -> bool {
    match frame_scope(frame) {
        FrameScope::Connection => true,
        FrameScope::Thread(thread) => own_thread == Some(thread),
        FrameScope::Malformed => false,
    }
}

/// How a server answered an interrupt, read off its JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptReply {
    /// The interrupt was accepted.
    Accepted,
    /// The server no longer has the thread: `thread not found` or
    /// `thread not loaded` — nothing is running it.
    ThreadGone(String),
    /// Any other refusal, verbatim.
    Refused(String),
}

impl InterruptReply {
    /// Read a `turn/interrupt` response.
    #[must_use]
    pub fn of(response: &Value) -> Self {
        let Some(error) = response.get("error") else {
            return match reply_result("turn/interrupt", response) {
                Ok(_) => InterruptReply::Accepted,
                Err(why) => InterruptReply::Refused(
                    why.strip_prefix("it ")
                        .map_or(why.clone(), |rest| format!("the app-server {rest}")),
                ),
            };
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the app-server refused the interrupt")
            .to_string();
        if message.contains("thread not found") || message.contains("thread not loaded") {
            InterruptReply::ThreadGone(message)
        } else {
            InterruptReply::Refused(message)
        }
    }
}

/// The `result` object of a JSON-RPC response to `method`, or why there is
/// none: the server's own error, verbatim, or an answer carrying neither.
///
/// # Errors
///
/// The refusal or the malformed answer, worded for a diagnostic.
pub fn reply_result<'a>(method: &str, response: &'a Value) -> Result<&'a Value, String> {
    if let Some(error) = response.get("error") {
        return Err(format!("it refused {method}: {error}"));
    }
    match response.get("result") {
        Some(result) if result.is_object() => Ok(result),
        Some(other) => Err(format!(
            "it answered {method} with a result that is not an object: {other}"
        )),
        None => Err(format!(
            "it answered {method} with neither a result nor an error"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_s_socket_rendezvous_is_named_for_the_euid_under_the_canonical_temp_dir() {
        assert_eq!(
            codex_daemon_dir(Path::new("/private/tmp"), 501),
            Path::new("/private/tmp/codex-daemon-501")
        );
        assert_eq!(CODEX_DAEMON_DIR_MODE, 0o700);
    }

    #[test]
    fn a_frame_with_a_malformed_thread_field_is_no_one_s() {
        for frame in [
            json!({"method": "item/completed", "params": {"threadId": 7}}),
            json!({"method": "item/completed", "params": {"threadId": ""}}),
            json!({"method": "thread/started", "params": {"thread": {"id": null}}}),
            json!({"method": "thread/started", "params": {"thread": "t"}}),
        ] {
            assert_eq!(frame_scope(&frame), FrameScope::Malformed, "{frame}");
            assert!(!is_own_frame(&frame, Some("t")), "{frame}");
            assert!(!is_own_frame(&frame, None), "{frame}");
        }
        let own = json!({"method": "turn/started", "params": {"threadId": "t"}});
        assert!(is_own_frame(&own, Some("t")));
        let wide = json!({"method": "account/updated", "params": {}});
        assert!(is_own_frame(&wide, Some("t")));
    }

    #[test]
    fn a_reply_yields_its_result_object_and_names_anything_else() {
        let ok = json!({"id": 1, "result": {"status": "unsubscribed"}});
        assert_eq!(
            reply_result("thread/unsubscribe", &ok),
            Ok(&json!({"status": "unsubscribed"}))
        );
        let refused = json!({"id": 1, "error": {"message": "no such thread"}});
        assert!(reply_result("thread/unsubscribe", &refused)
            .unwrap_err()
            .contains("refused thread/unsubscribe: {\"message\":\"no such thread\"}"));
        let empty = json!({"id": 1});
        assert!(reply_result("initialize", &empty)
            .unwrap_err()
            .contains("neither a result nor an error"));
        let scalar = json!({"id": 1, "result": true});
        assert!(reply_result("initialize", &scalar)
            .unwrap_err()
            .contains("not an object: true"));
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_server_keeps_only_base_and_codex_variables() {
        let dispatch = env(&[
            ("PATH", "/bin"),
            ("HOME", "/h"),
            ("LC_ALL", "C"),
            ("CODEX_HOME", "/h/.codex"),
            ("CODEX_FOO", "1"),
            ("OPENAI_API_KEY", "k"),
            ("https_proxy", "http://p"),
            ("SSL_CERT_FILE", "/c"),
            ("ONEPIPELINE_RUN_ID", "run-a"),
            ("XDG_RUNTIME_DIR", "/run/a"),
            ("MOCK_KNOB", "1"),
        ]);
        let kept = server_env(&dispatch, EnvPlatform::Unix);
        let names: Vec<&str> = kept.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "CODEX_FOO",
                "CODEX_HOME",
                "HOME",
                "LC_ALL",
                "OPENAI_API_KEY",
                "PATH",
                "SSL_CERT_FILE",
                "https_proxy"
            ]
        );
    }

    #[test]
    fn the_sweeper_keeps_only_base_variables_and_its_state_root() {
        let dispatch = env(&[
            ("PATH", "/bin"),
            ("HOME", "/h"),
            ("XDG_STATE_HOME", "/s"),
            ("CODEX_HOME", "/h/.codex"),
            ("ONEAGENTGRAPH_SCRATCH_DIR", "/tmp/oneagentgraph-1"),
            ("ONEPIPELINE_RUN_ID", "run-a"),
            (MOCK_HARNESS_VAR, "1"),
            (DISPATCH_MARKER_VAR, "lease-uuid"),
        ]);
        let names = |platform| -> Vec<String> {
            sweeper_env(&dispatch, platform)
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        assert_eq!(names(EnvPlatform::Unix), ["HOME", "PATH", "XDG_STATE_HOME"]);
        assert_eq!(names(EnvPlatform::Windows), ["PATH", "XDG_STATE_HOME"]);
    }

    #[test]
    fn windows_matches_base_and_codex_names_case_insensitively() {
        let dispatch = env(&[
            ("Path", "C:\\bin"),
            ("SYSTEMROOT", "C:\\Windows"),
            ("codex_home", "C:\\c"),
            ("HOME", "C:\\h"),
        ]);
        let kept = server_env(&dispatch, EnvPlatform::Windows);
        let names: Vec<&str> = kept.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Path", "SYSTEMROOT", "codex_home"]);
    }

    #[test]
    fn a_dispatch_sets_what_differs_and_excludes_what_it_lacks() {
        let server = env(&[
            ("PATH", "/bin"),
            ("HOME", "/h"),
            ("CODEX_HOME", "/h/.codex"),
            ("CODEX_ONLY_FIRST", "1"),
            (SERVER_MARKER_VAR, "server-uuid"),
        ]);
        let dispatch = env(&[
            ("PATH", "/bin"),
            ("HOME", "/other"),
            ("CODEX_HOME", "/h/.codex"),
            ("ONEPIPELINE_RUN_ID", "run-b"),
        ]);
        let policy = ShellEnvironmentPolicy::for_dispatch(
            &dispatch,
            &server,
            &DispatchMarker::new("lease-uuid").unwrap(),
            EnvPlatform::Unix,
        );
        assert_eq!(
            policy.set,
            BTreeMap::from([
                ("HOME".to_string(), "/other".to_string()),
                ("ONEPIPELINE_RUN_ID".to_string(), "run-b".to_string()),
                (DISPATCH_MARKER_VAR.to_string(), "lease-uuid".to_string()),
            ])
        );
        assert_eq!(policy.exclude, ["CODEX_ONLY_FIRST", SERVER_MARKER_VAR]);
        assert_eq!(
            policy.thread_config(),
            json!({"shell_environment_policy": {
                "set": {"HOME": "/other", "ONEPIPELINE_RUN_ID": "run-b", DISPATCH_MARKER_VAR: "lease-uuid"},
                "exclude": ["CODEX_ONLY_FIRST", SERVER_MARKER_VAR],
            }})
        );
    }

    #[test]
    fn a_dispatch_cannot_smuggle_a_marker_of_its_own() {
        let policy = ShellEnvironmentPolicy::for_dispatch(
            &env(&[
                (DISPATCH_MARKER_VAR, "forged"),
                (SERVER_MARKER_VAR, "forged"),
            ]),
            &env(&[(SERVER_MARKER_VAR, "real")]),
            &DispatchMarker::new("mine").unwrap(),
            EnvPlatform::Unix,
        );
        assert_eq!(policy.set[DISPATCH_MARKER_VAR], "mine");
        assert!(!policy.set.contains_key(SERVER_MARKER_VAR));
        assert_eq!(policy.exclude, [SERVER_MARKER_VAR]);
    }

    #[test]
    fn the_pool_key_moves_with_every_process_level_input_and_nothing_else() {
        let base_env = env(&[("PATH", "/bin"), ("CODEX_HOME", "/a")]);
        let args = vec!["-c".to_string(), "features.plugins=false".to_string()];
        let key = pool_key("/usr/bin/codex", "codex-cli 1.0\n", &base_env, &args, false);
        assert!(key.as_str().starts_with("codex-app-server-"), "{key}");
        assert_eq!(
            key,
            pool_key("/usr/bin/codex", "codex-cli 1.0", &base_env, &args, false),
            "trailing whitespace of the version output is not a different release"
        );
        for other in [
            pool_key("/opt/codex", "codex-cli 1.0", &base_env, &args, false),
            pool_key("/usr/bin/codex", "codex-cli 1.1", &base_env, &args, false),
            pool_key(
                "/usr/bin/codex",
                "codex-cli 1.0",
                &env(&[("PATH", "/bin"), ("CODEX_HOME", "/b")]),
                &args,
                false,
            ),
            pool_key("/usr/bin/codex", "codex-cli 1.0", &base_env, &[], false),
            pool_key("/usr/bin/codex", "codex-cli 1.0", &base_env, &args, true),
        ] {
            assert_ne!(key, other);
        }
    }

    #[test]
    fn a_mock_harness_server_keeps_its_marker_and_keys_apart_from_a_real_one() {
        let real = env(&[("PATH", "/bin"), ("MOCK_STDOUT", "{}")]);
        let mut mocked = real.clone();
        mocked.push((MOCK_HARNESS_VAR.to_string(), "1".to_string()));
        for platform in [EnvPlatform::Unix, EnvPlatform::Windows] {
            let kept = server_env(&mocked, platform);
            assert!(
                kept.iter().any(|(name, _)| name == MOCK_HARNESS_VAR),
                "{platform:?}: {kept:?}"
            );
        }
        let key = |dispatch: &[(String, String)]| {
            pool_key(
                "/usr/bin/oneharness",
                "oneharness 1.0",
                &server_env(dispatch, EnvPlatform::Unix),
                &[],
                false,
            )
        };
        assert_ne!(key(&mocked), key(&real));
    }

    #[test]
    fn a_frame_belongs_to_the_thread_it_names_or_to_the_connection() {
        let own = Some("t-1");
        assert!(is_own_frame(&json!({"id": 3, "result": {}}), own));
        assert!(is_own_frame(
            &json!({"method": "account/rateLimits/updated", "params": {}}),
            own
        ));
        assert!(is_own_frame(
            &json!({"method": "turn/started", "params": {"threadId": "t-1"}}),
            own
        ));
        assert!(is_own_frame(
            &json!({"method": "thread/started", "params": {"thread": {"id": "t-1"}}}),
            own
        ));
        assert!(!is_own_frame(
            &json!({"method": "thread/status/changed", "params": {"threadId": "t-2"}}),
            own
        ));
        assert!(!is_own_frame(
            &json!({"id": 9, "method": "item/commandExecution/requestApproval", "params": {"threadId": "t-2"}}),
            own
        ));
        assert!(!is_own_frame(
            &json!({"method": "turn/started", "params": {"threadId": "t-1"}}),
            None
        ));
    }

    #[test]
    fn an_interrupt_reply_names_a_thread_the_server_no_longer_has() {
        assert_eq!(
            InterruptReply::of(&json!({"id": 1, "result": {}})),
            InterruptReply::Accepted
        );
        assert!(matches!(
            InterruptReply::of(&json!({"id": 1})),
            InterruptReply::Refused(_)
        ));
        assert_eq!(
            InterruptReply::of(&json!({"id": 1, "result": null})),
            InterruptReply::Refused(
                "the app-server answered turn/interrupt with a result that is not an object: null"
                    .to_string()
            )
        );
        assert!(matches!(
            InterruptReply::of(
                &json!({"id": 1, "error": {"code": -32600, "message": "thread not found: t"}})
            ),
            InterruptReply::ThreadGone(_)
        ));
        assert!(matches!(
            InterruptReply::of(&json!({"id": 1, "error": {"message": "thread not loaded"}})),
            InterruptReply::ThreadGone(_)
        ));
        assert_eq!(
            InterruptReply::of(
                &json!({"id": 1, "error": {"message": "expected active turn id x"}})
            ),
            InterruptReply::Refused("expected active turn id x".to_string())
        );
    }
}
