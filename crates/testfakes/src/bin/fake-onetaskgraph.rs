//! A `onetaskgraph` executable that acts out an install answering badly.
//!
//! **Not a double for the store.** Every plan in this suite is read out of a
//! real `onetaskgraph`, against a real folder of Markdown, because a plan read
//! through a stand-in would prove the stand-in. What this stands in for is what a
//! real binary cannot be asked to be: an install of the *wrong version*, one that
//! cannot answer at all, one whose answers this build has to refuse — and, through
//! `onetaskgraph.delegate`, an install at a **different release** of the real one,
//! whose answers are the real store's own carrying a later release's field or
//! missing a field this build reads.
//!
//! A journey that reaches a **scripted** answer asserts a **refusal**, so no
//! scenario here can make a plan read look like it worked: what those are for is
//! the branches on the other side of "the store answered something this build will
//! not act on", which no correct binary reaches and no fixture inside the crate
//! under test could reach either. A **delegated** answer is different in kind —
//! the store is real, the answer is that store's own, and the release it is served
//! at is the one variable — so a journey there may assert that a projection lands.
//!
//! Scripted from the directory every double reads its scenario out of:
//!
//! * `onetaskgraph.version` — what `--version` prints, verbatim. Absent, it
//!   prints nothing at all, which is an install this build cannot read a version
//!   off.
//! * `onetaskgraph.refuse` — makes **every** invocation exit 1, saying the file's
//!   contents on stderr. An install that is broken is not broken per verb.
//! * `onetaskgraph.refuse-reads` — the same, for every invocation **except**
//!   `--version`: an install that says what it is and then cannot answer a query,
//!   which is the one shape that gets past the launch's version check.
//! * `onetaskgraph.<verb>` — what a read answers with on stdout, verbatim, where
//!   `<verb>` is the command line's words joined by `-`: `project-show`,
//!   `task-list`, `task-deps`. A read nothing scripts is refused, because a
//!   double that answered a query no journey stated would be inventing a store.
//! * `onetaskgraph.<verb>.2` — what the **second** call to that verb answers,
//!   and `.3` the third, so a journey can state a walk of several pages.
//! * `onetaskgraph.delegate` — an executable to proxy unscripted calls to. A
//!   matching `onetaskgraph.<verb>.refuse.<n>` injects one refused call before
//!   later calls reach that real executable, for retry journeys at this sibling
//!   subprocess boundary.
//! * `onetaskgraph.<verb>.grow` — a JSON object whose members a *later release* of
//!   the store added to that verb's machine answer. The delegated answer is the
//!   real store's, and these are merged into the response, into every item of it,
//!   into each item's own `item`, and into each label that item carries. This is
//!   the other thing a real binary cannot be asked to be: an install of a release
//!   **newer** than the one this build was written against. `location` on a
//!   project item is the real one — onetaskgraph 0.2.14 added it.
//! * `onetaskgraph.<verb>.shrink` — the opposite, and the reason growth may not be
//!   answered by reading nothing at all: the whitespace-separated field names an
//!   item of that verb's answer **stopped** carrying. A field the projection reads
//!   going missing is still a refusal, and this is what makes a journey able to
//!   tell the two apart.

use onepipeline_testfakes as fake;
use serde_json::{Map, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode};

/// The release a delegated answer is served at: the real store's own answer, plus or
/// minus what another release of it carries.
///
/// Nothing here invents a store. Both halves are applied to bytes the **real** binary
/// produced against a real folder of Markdown, so what a journey drives is that store's
/// own answer at a shape this build was not written against — which is the one thing an
/// install on this host cannot be asked to be.
#[derive(Default)]
struct Release {
    /// Members a later release added to every item of this answer.
    grown: Map<String, Value>,
    /// Fields an item of this answer no longer carries.
    shrunk: Vec<String>,
}

impl Release {
    /// What a journey scripted for one verb, `None` where it scripted neither half, and a
    /// refusal to say where it scripted one this program cannot read.
    ///
    /// A scenario file is external input like any other, so a `.grow` that is not a JSON
    /// object is reported as the scripting mistake it is rather than crashed on: a panic
    /// here would reach the journey as a store that died, which is a different fixture
    /// from the one it asked for.
    fn scripted(dir: &Path, name: &str) -> Result<Option<Self>, String> {
        let grown = match std::fs::read_to_string(dir.join(format!("{name}.grow"))) {
            Err(_) => None,
            Ok(scripted) => match serde_json::from_str(&scripted) {
                Ok(Value::Object(members)) => Some(members),
                _ => {
                    return Err(format!(
                        "`{name}.grow` states the members a later release added to this \
                         answer, as a JSON object"
                    ))
                }
            },
        };
        let shrunk = std::fs::read_to_string(dir.join(format!("{name}.shrink")))
            .ok()
            .map(|scripted| scripted.split_whitespace().map(ToOwned::to_owned).collect());
        Ok((grown.is_some() || shrunk.is_some()).then(|| Self {
            grown: grown.unwrap_or_default(),
            shrunk: shrunk.unwrap_or_default(),
        }))
    }

    fn apply(&self, object: &mut Value) {
        let Some(object) = object.as_object_mut() else {
            return;
        };
        for field in &self.shrunk {
            object.remove(field);
        }
        for (key, value) in &self.grown {
            object.insert(key.clone(), value.clone());
        }
    }

    /// The delegated answer as this release would have written it.
    ///
    /// An answer that is not one of this store's paged JSON responses is handed back
    /// byte for byte: there is no item shape to move, and rewriting it would put this
    /// program between a journey and bytes the real binary wrote.
    fn answered(&self, stdout: &[u8]) -> Vec<u8> {
        let Ok(Value::Object(mut response)) = serde_json::from_slice::<Value>(stdout) else {
            return stdout.to_owned();
        };
        if let Some(items) = response.get_mut("items").and_then(Value::as_array_mut) {
            for entry in items {
                self.apply(entry);
                if let Some(item) = entry.get_mut("item") {
                    self.apply(item);
                    if let Some(labels) = item.get_mut("labels").and_then(Value::as_array_mut) {
                        for label in labels {
                            self.apply(label);
                        }
                    }
                }
            }
        }
        for (key, value) in &self.grown {
            response.insert(key.clone(), value.clone());
        }
        match serde_json::to_vec(&Value::Object(response)) {
            Ok(mut written) => {
                written.push(b'\n');
                written
            }
            Err(_) => stdout.to_owned(),
        }
    }
}

fn delegate(dir: &Path, args: &[String], release: Option<&Release>) -> Option<ExitCode> {
    let binary = std::fs::read_to_string(dir.join("onetaskgraph.delegate")).ok()?;
    let binary = binary.trim();
    // Only a journey that scripted a release captures the answer. Every other delegated
    // call keeps the streams it always had, so what those journeys drive is untouched.
    let Some(release) = release else {
        let status = Command::new(binary).args(args).status().ok()?;
        return Some(code(status.code()));
    };
    let answered = Command::new(binary).args(args).output().ok()?;
    // Flushed here rather than left to process exit, where a failure is silent: a caller
    // reading a truncated machine answer under the delegate's own success code is the one
    // ending a proxy must not have, so a stream this program could not hand on is a
    // refusal instead.
    let handed = (|| -> std::io::Result<()> {
        let mut stderr = std::io::stderr();
        stderr.write_all(&answered.stderr)?;
        stderr.flush()?;
        let mut stdout = std::io::stdout();
        stdout.write_all(&release.answered(&answered.stdout))?;
        stdout.flush()
    })();
    if let Err(error) = handed {
        return Some(fake::refuse(&format!(
            "the delegated answer could not be handed on: {error}"
        )));
    }
    Some(code(answered.status.code()))
}

fn code(code: Option<i32>) -> ExitCode {
    ExitCode::from(code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "onetaskgraph", &args);

    if let Ok(reason) = std::fs::read_to_string(dir.join("onetaskgraph.refuse")) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }
    if args == ["--version"] {
        if dir.join("onetaskgraph.version-invalid-utf8").is_file() {
            return match std::io::Write::write_all(&mut std::io::stdout(), &[0xff]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::from(1),
            };
        }
        if let Some(status) = delegate(&dir, &args, None) {
            return status;
        }
        let printed = std::fs::read_to_string(dir.join("onetaskgraph.version")).unwrap_or_default();
        print!("{printed}");
        return ExitCode::SUCCESS;
    }
    if let Ok(reason) = std::fs::read_to_string(dir.join("onetaskgraph.refuse-reads")) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }

    // The verb is the leading words that are not flags or their values, which is
    // how this binary's own surface is shaped: `project show ID --json`.
    let verb: Vec<&str> = args
        .iter()
        .take_while(|word| !word.starts_with("--"))
        .take(2)
        .map(String::as_str)
        .collect();
    if verb.len() < 2 {
        return fake::refuse(&format!("this double speaks no such command: {args:?}"));
    }
    let name = format!("onetaskgraph.{}", verb.join("-"));

    let nth = fake::count(&dir, &name);
    if let Ok(reason) = std::fs::read_to_string(dir.join(format!("{name}.refuse.{nth}"))) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }
    let answer = (nth > 1)
        .then(|| std::fs::read_to_string(dir.join(format!("{name}.{nth}"))).ok())
        .flatten()
        .or_else(|| std::fs::read_to_string(dir.join(&name)).ok());
    match answer {
        Some(scripted) => {
            print!("{scripted}");
            ExitCode::SUCCESS
        }
        None if dir.join("onetaskgraph.delegate").is_file() => {
            match Release::scripted(&dir, &name) {
                Err(why) => fake::refuse(&why),
                Ok(release) => delegate(&dir, &args, release.as_ref()).unwrap_or_else(|| {
                    fake::refuse("the scripted onetaskgraph delegate could not run")
                }),
            }
        }
        None => fake::refuse(&format!(
            "no scenario scripts `{name}`, so this double has no store to answer for"
        )),
    }
}
