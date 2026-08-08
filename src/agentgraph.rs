//! The `oneagentgraph` seam.
//!
//! Agent, harness, and model selection stay in that library, so this crate
//! reaches it the way any other caller does: through its CLI. Composition, not
//! reimplementation — nothing here decides a harness, a chain, or a model, and
//! the envelopes it produces are relayed into the merged stream exactly as it
//! emitted them.
//!
//! The binary is resolved from [`BINARY_ENV`] so an operator can point at a
//! specific build, and so a test can compose against a real executable standing
//! in for one.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};

/// The environment variable naming the `oneagentgraph` executable.
pub const BINARY_ENV: &str = "ONEPIPELINE_ONEAGENTGRAPH_BIN";

/// The executable's name when the environment names none.
pub const DEFAULT_BINARY: &str = "oneagentgraph";

/// The environment variable the dag-scope graph substitutes the run id into.
pub const RUN_ID_ENV: &str = "ONEPIPELINE_RUN_ID";

/// The member of the shipped dag-scope graph that drives the run.
pub const ORCHESTRATOR_MEMBER: &str = "orchestrator";

/// The member of the shipped dag-scope graph that paces planner updates.
pub const CHECK_IN_MEMBER: &str = "check-in";

/// The executable this process invokes.
pub fn binary() -> String {
    std::env::var(BINARY_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.to_string())
}

fn sibling(message: impl Into<String>) -> Error {
    Error::Sibling {
        tool: "oneagentgraph",
        message: message.into(),
    }
}

/// Render the reserved label keys as the `k=v` pairs the CLI takes.
pub fn label_args(labels: &Labels) -> Vec<String> {
    let mut args = Vec::new();
    let mut push = |key: &str, value: String| args.push(format!("{key}={value}"));
    if let Some(run) = &labels.run_id {
        push("run_id", run.clone());
    }
    if let Some(round) = labels.round {
        push("round", round.to_string());
    }
    if let Some(node) = &labels.node {
        push("node", node.clone());
    }
    if let Some(step) = &labels.step {
        push("step", step.clone());
    }
    if let Some(persona) = &labels.persona {
        push("persona", persona.clone());
    }
    args
}

/// One `oneagentgraph run`, started and streaming.
#[derive(Debug)]
pub struct GraphRun {
    child: Child,
}

impl GraphRun {
    /// Start a graph, streaming its envelopes on stdout.
    pub fn start(
        graph: &str,
        task: &str,
        dir: Option<&Path>,
        labels: &Labels,
        env: &[(String, String)],
    ) -> Result<Self> {
        let mut command = Command::new(binary());
        command.arg("run").arg(graph);
        command.arg("--task").arg(task);
        command.arg("--output").arg("json");
        if let Some(dir) = dir {
            command.arg("--dir").arg(dir);
        }
        for label in label_args(labels) {
            command.arg("--label").arg(label);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command
            .spawn()
            .map_err(|e| sibling(format!("cannot start `{} run`: {e}", binary())))?;
        Ok(Self { child })
    }

    /// The envelopes it has produced, taken once.
    ///
    /// A line the envelope schema does not accept is skipped rather than ending
    /// the stream: a sibling emitting a kind this build does not know is not a
    /// reason to stop relaying the ones it does.
    pub fn events(&mut self) -> Box<dyn Iterator<Item = Result<Envelope>> + Send> {
        let Some(stdout) = self.child.stdout.take() else {
            return Box::new(std::iter::empty());
        };
        Box::new(
            BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<Envelope>(&line).ok())
                .map(Ok),
        )
    }

    /// Block until the graph settles, and report whether it succeeded.
    pub fn wait(&mut self) -> Result<Settled> {
        let status = self
            .child
            .wait()
            .map_err(|e| sibling(format!("waiting for `{} run`: {e}", binary())))?;
        let stderr = self
            .child
            .stderr
            .take()
            .map(|mut pipe| {
                use std::io::Read;
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
            .unwrap_or_default();
        Ok(Settled {
            code: status.code(),
            stderr,
        })
    }

    /// Ask the graph to stop.
    pub fn cancel(&mut self, kill: bool) {
        if kill {
            let _ = self.child.kill();
        } else {
            // Cooperative cancellation is the sibling's own verb: it gives the
            // member a chance to preserve its work, which killing the process
            // does not.
            let _ = Command::new(binary())
                .arg("cancel")
                .arg(self.child.id().to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    /// The started process's id, for the ledger's record of what is running.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// How a graph run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    /// Its exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// What it wrote to stderr, for the failure's own evidence.
    pub stderr: String,
}

impl Settled {
    /// Whether the graph completed successfully.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Restart a resettable schedule's clock.
///
/// This is the whole pacemaker-reset contract: a surface a planner actually
/// read is what restarts the check-in clock, so a run that is already reporting
/// does not also get a pacemaker surface.
pub fn reset_timer(run: &str, member: &str) -> Result<()> {
    let output = Command::new(binary())
        .arg("reset-timer")
        .arg(run)
        .arg(member)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} reset-timer`: {e}", binary())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(sibling(format!(
        "reset-timer {run} {member} exited {}: {}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Check a graph config without running it.
pub fn validate(graph: &str) -> Result<()> {
    let output = Command::new(binary())
        .arg("validate")
        .arg(graph)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} validate`: {e}", binary())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(sibling(format!(
        "{graph} is not a valid graph: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// The provider-health block a view reports, sourced from `oneagentgraph
/// health`.
///
/// A health probe that cannot run is silence rather than a failure: a view whose
/// provider block is missing still reports everything else it knows.
pub fn health() -> Option<String> {
    let output = Command::new(binary())
        .arg("health")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binary_comes_from_the_environment_or_falls_back() {
        // The variable is read per call rather than cached, so a test harness
        // and an operator both reach the executable they named.
        assert_eq!(
            std::env::var(BINARY_ENV)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_BINARY.to_string()),
            binary()
        );
    }

    #[test]
    fn only_the_reserved_labels_the_contract_names_are_rendered() {
        let labels = Labels {
            run_id: Some("demo".into()),
            round: Some(2),
            node: Some("build".into()),
            step: Some("implement".into()),
            persona: Some("engineer".into()),
            extra: serde_json::Map::new(),
        };
        assert_eq!(
            label_args(&labels),
            vec![
                "run_id=demo",
                "round=2",
                "node=build",
                "step=implement",
                "persona=engineer",
            ]
        );
        assert!(label_args(&Labels::default()).is_empty());
    }

    #[test]
    fn a_settled_run_reports_only_a_zero_exit_as_success() {
        assert!(Settled {
            code: Some(0),
            stderr: String::new()
        }
        .succeeded());
        assert!(!Settled {
            code: Some(1),
            stderr: String::new()
        }
        .succeeded());
        assert!(!Settled {
            code: None,
            stderr: String::new()
        }
        .succeeded());
    }
}
