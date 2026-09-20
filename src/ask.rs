//! `onepipeline ask` — a dispatched agent's blocking question to its manager,
//! over the run's own planner channel.
//!
//! What the verb promises is entry 87 of `docs/contract-divergences.md`; it is
//! not restated here, on the terms [`crate::unwatched`] keeps beside its own
//! entry. What belongs beside the code is the seam: the question is one
//! `planner-question` frame raised through the **linked** bus under the policy
//! the run's launch record carries — the same [`crate::channel::ChannelState`]
//! the manager's reply verbs write through — and the answer on standard output
//! is the bus's own one-line object, in the shape its command line prints, so
//! an asker that read `onemessagebus ask` reads this without change.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use onemessagebus::{Address, Answer, Asker, Correlation, Pending};
use serde_json::{json, Value};

use crate::channel::{source, ChannelState, Surface};
use crate::cli::AskArgs;
use crate::error::{Error, Result, EXIT_QUEUED, EXIT_SUCCESS};
use crate::ledger::{self, LaunchRecord, RunPaths};

/// The kind every question this verb raises carries.
pub(crate) const QUESTION_KIND: &str = "planner-question";

/// The environment variable naming the run whose channel is asked on.
pub(crate) const RUN_ID_ENV: &str = crate::agentgraph::RUN_ID_ENV;

/// Where the question came from, for a refusal that names the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Arguments,
    File,
    Stdin,
}

impl Form {
    fn as_str(self) -> &'static str {
        match self {
            Self::Arguments => "the argument words",
            Self::File => "the file named with --file",
            Self::Stdin => "standard input",
        }
    }
}

/// The question, from whichever of the three forms carried it: the argument
/// words joined by one space, the named file, or standard input when neither
/// is given.
///
/// # Errors
///
/// A blank question, one carrying a NUL byte — which no frame can carry — and
/// a file that cannot be read, each named with the form it came in, and
/// refused before anything is raised.
pub(crate) fn question(args: &AskArgs) -> Result<String> {
    let (form, text) = match (&args.file, args.text.is_empty()) {
        (Some(path), _) => (Form::File, read_file(path)?),
        (None, false) => (Form::Arguments, args.text.join(" ")),
        (None, true) => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| {
                    Error::Invalid(format!(
                        "the question on standard input could not be read: {error}"
                    ))
                })?;
            (Form::Stdin, text)
        }
    };
    if text.contains('\0') {
        return Err(Error::Invalid(format!(
            "the question from {} carries a NUL byte, which no frame can carry; remove it and \
             ask again — nothing was asked",
            form.as_str()
        )));
    }
    if text.trim().is_empty() {
        return Err(Error::Invalid(format!(
            "the question from {} is blank; state the decision fork and what each branch would \
             change, so the manager can answer it in one reply — nothing was asked",
            form.as_str()
        )));
    }
    Ok(text)
}

fn read_file(path: &PathBuf) -> Result<String> {
    std::fs::read_to_string(path).map_err(|error| {
        Error::Invalid(format!(
            "the question file {} could not be read: {error}; check the path, or pipe the \
             question in on standard input instead",
            path.display()
        ))
    })
}

/// The run to ask on, from `ONEPIPELINE_RUN_ID`.
///
/// # Errors
///
/// A variable that is absent or blank, named: a question with no run to ask on
/// is refused before anything is raised.
pub(crate) fn run_id() -> Result<String> {
    std::env::var(RUN_ID_ENV)
        .ok()
        .filter(|run| !run.trim().is_empty())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{RUN_ID_ENV} is not set, so there is no run whose channel to ask on; run this \
                 from inside a dispatch, which exports it, or export the run id yourself"
            ))
        })
}

/// Who asks, from `ONEPIPELINE_CHANNEL_ASKER`: `None` when it is unset, and a
/// refusal when it is set to something naming nobody.
///
/// # Errors
///
/// A blank value: unset means nobody, but a value that is there and blank is a
/// caller that meant to name an asker and did not.
pub(crate) fn asker() -> Result<Option<Asker>> {
    match std::env::var_os(crate::channel::ASKER_ENV) {
        None => Ok(None),
        Some(word) => Asker::named(&word, crate::channel::ASKER_ENV)
            .map(Some)
            .map_err(|refusal| {
                Error::Invalid(format!(
                    "{refusal}; unset it, or set it to the word a later session of this dispatch \
                     asks as"
                ))
            }),
    }
}

/// What the question is about, from `--about`, in the bus's own bound: at most
/// 512 bytes, not blank, and free of control characters.
///
/// # Errors
///
/// A value the bus would refuse, refused here by that name before anything is
/// raised.
pub(crate) fn about(args: &AskArgs) -> Result<Option<Address>> {
    args.about
        .as_deref()
        .map(|text| {
            text.parse::<Address>()
                .map_err(|refusal| Error::Invalid(format!("--about: {refusal}")))
        })
        .transpose()
}

/// What one question carries beside its text.
#[derive(Debug)]
pub(crate) struct Request {
    /// The question.
    pub message: String,
    /// Who asks, when the environment named one.
    pub asker: Option<Asker>,
    /// The node the question is about, when one was named.
    pub about: Option<Address>,
    /// The reply window, when `--timeout` named one.
    pub timeout: Option<u64>,
}

/// A question raised on the channel, waiting for its answer — or one the bus
/// refused to raise.
pub(crate) enum Question {
    /// Raised, with the handle its answer arrives on and the window it waits.
    Pending {
        /// The bus's handle, boxed: it is two queues and their transport, and
        /// the refused arm beside it is one sentence.
        pending: Box<Pending<Value>>,
        /// How long the wait is.
        window: Duration,
    },
    /// The bus refused the question; nothing was raised.
    Refused(String),
}

impl Question {
    /// Raise `request` on the run's `surfaces` queue, under the bus policy its
    /// launch record carries, and hand back the handle its answer arrives on.
    ///
    /// The launch record is read first, because it is what the policy comes
    /// from: a run whose record cannot be read is refused before anything is
    /// raised.
    ///
    /// # Errors
    ///
    /// A launch record that cannot be read.
    pub(crate) fn raise(paths: &RunPaths, request: Request) -> Result<Self> {
        let launch: LaunchRecord = ledger::read_json(&paths.launch())?;
        let window = request
            .timeout
            .map(Duration::from_secs)
            .or_else(|| reply_window(launch.bus_config.as_ref()))
            .unwrap_or(onemessagebus::DEFAULT_REPLY_WINDOW);
        let channel = ChannelState::of_run(paths, &launch);
        let question = Surface {
            id: 0,
            kind: QUESTION_KIND.to_owned(),
            message: request.message,
            source: source::PROPOSAL.to_owned(),
            blocking: true,
            queued_at: crate::sys::now_millis(),
            abandoned: false,
            asker: request.asker,
            // Stamped by the bus off `about`, never written here.
            workstream: None,
            correlation: None,
        };
        Ok(match channel.ask(question, request.about) {
            Ok(pending) => Self::Pending {
                pending: Box::new(pending),
                window,
            },
            Err(error) => Self::Refused(error.to_string()),
        })
    }

    /// How long the wait is: the resolved window, or none for a question that
    /// was never raised.
    pub(crate) fn window(&self) -> Duration {
        match self {
            Self::Pending { window, .. } => *window,
            Self::Refused(_) => Duration::ZERO,
        }
    }

    /// The correlation the answer echoes, once the question is raised.
    pub(crate) fn correlation(&self) -> Option<&Correlation> {
        match self {
            Self::Pending { pending, .. } => Some(pending.correlation()),
            Self::Refused(_) => None,
        }
    }

    /// Block for the reply window and answer what the bus answered.
    ///
    /// A wait that elapses leaves the question standing and **marks it
    /// abandoned**, exactly as the bus's own command line does: nothing is
    /// listening for the answer now, and a later listener of the same asker
    /// takes it back. An elapsed wait is never a ruling.
    pub(crate) fn answer(self) -> Asked {
        match self {
            Self::Refused(reason) => Asked {
                answer: Answer::Refused(onemessagebus::ask::Refusal {
                    kind: onemessagebus::RefusalKind::Capability,
                    reason,
                }),
                correlation: None,
            },
            Self::Pending { pending, window } => {
                let answer = pending.wait(window);
                if matches!(answer, Answer::Timeout) {
                    // A mark that cannot be made changes nothing about the
                    // answer: the question stands either way, and the wait
                    // still elapsed.
                    let _ = pending.abandon();
                }
                Asked {
                    answer,
                    correlation: Some(pending.correlation().clone()),
                }
            }
        }
    }
}

/// The reply window the run's bus configuration names for the `surfaces`
/// queue, where it names one.
///
/// A codec's `reply_window_seconds` is how long *that* codec's questions on its
/// queue wait; the one this verb takes is the longest named by a codec serving
/// the queue the question is raised on, so the question waits at least as long
/// as any listener the configuration expects a manager to answer.
fn reply_window(config: Option<&onemessagebus::Config>) -> Option<Duration> {
    config?
        .codecs
        .values()
        .filter(|codec| {
            codec
                .queue
                .as_ref()
                .is_some_and(|queue| queue.as_str() == onemessagebus_agent::channel::SURFACES)
        })
        .filter_map(|codec| codec.reply_window_seconds)
        .map(|seconds| Duration::from_secs(seconds.get()))
        .max()
}

/// What `onepipeline ask` answered: the bus's answer, and the correlation of
/// the question it answers.
#[derive(Debug)]
pub(crate) struct Asked {
    /// The bus's answer.
    pub answer: Answer<Value>,
    /// The question's correlation, once it was raised.
    pub correlation: Option<Correlation>,
}

impl Asked {
    /// `0` for a reply; `1` for a timeout, an abandoned listener, or a refusal.
    pub(crate) const fn exit_code(&self) -> i32 {
        match self.answer {
            Answer::Reply(_) => EXIT_SUCCESS,
            Answer::Timeout | Answer::Abandoned | Answer::Refused(_) => EXIT_QUEUED,
        }
    }

    /// The bus's one-line answer, in the shape its command line prints:
    /// `{"answer":"reply","correlation":…,"reply":…}`, or `timeout`,
    /// `abandoned` and `refused` naming the correlation where there is one and
    /// the reason where there is one.
    pub(crate) fn render(&self) -> String {
        let mut object = json!({"answer": self.answer.word()});
        if let Some(correlation) = &self.correlation {
            object["correlation"] = json!(correlation);
        }
        match &self.answer {
            Answer::Reply(reply) => object["reply"] = reply.clone(),
            Answer::Refused(refusal) => object["reason"] = json!(refusal.reason),
            Answer::Timeout | Answer::Abandoned => {}
        }
        object.to_string()
    }

    /// What to do next, for standard error, where the answer is not a reply.
    pub(crate) fn advice(&self, run: &str, window: Duration) -> Option<String> {
        let correlation = self
            .correlation
            .as_ref()
            .map_or_else(|| "<correlation>".to_owned(), ToString::to_string);
        match &self.answer {
            Answer::Reply(_) => None,
            Answer::Timeout => Some(format!(
                "no reply echoing {correlation} arrived within {} seconds; the question stands \
                 on the channel, marked abandoned, and a manager may still answer it with \
                 `onepipeline reply {run} --correlation {correlation}`",
                window.as_secs()
            )),
            Answer::Abandoned => Some(format!(
                "the question {correlation} was abandoned and nobody re-attended it; ask again"
            )),
            Answer::Refused(refusal) => {
                Some(format!("the question was refused: {}", refusal.reason))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendering is the bus command line's own shape for each of the four
    /// answers, and the status is `0` for a reply alone.
    #[test]
    fn each_answer_renders_as_the_bus_prints_it() {
        let correlation: Correlation = "c-1".parse().expect("a correlation");
        let reply = Asked {
            answer: Answer::Reply(json!({"id": 1, "reply": {"completion": false}})),
            correlation: Some(correlation.clone()),
        };
        assert_eq!(
            reply.render(),
            r#"{"answer":"reply","correlation":"c-1","reply":{"id":1,"reply":{"completion":false}}}"#
        );
        assert_eq!(reply.exit_code(), EXIT_SUCCESS);
        assert!(reply.advice("r", Duration::from_secs(1)).is_none());

        let timeout = Asked {
            answer: Answer::Timeout,
            correlation: Some(correlation.clone()),
        };
        assert_eq!(
            timeout.render(),
            r#"{"answer":"timeout","correlation":"c-1"}"#
        );
        assert_eq!(timeout.exit_code(), EXIT_QUEUED);
        assert!(timeout
            .advice("r", Duration::from_secs(7))
            .expect("advice")
            .contains("within 7 seconds"));

        let abandoned = Asked {
            answer: Answer::Abandoned,
            correlation: Some(correlation),
        };
        assert_eq!(
            abandoned.render(),
            r#"{"answer":"abandoned","correlation":"c-1"}"#
        );
        assert_eq!(abandoned.exit_code(), EXIT_QUEUED);

        let refused = Asked {
            answer: Answer::Refused(onemessagebus::ask::Refusal {
                kind: onemessagebus::RefusalKind::Validator,
                reason: "no".into(),
            }),
            correlation: None,
        };
        assert_eq!(refused.render(), r#"{"answer":"refused","reason":"no"}"#);
        assert_eq!(refused.exit_code(), EXIT_QUEUED);
    }

    /// The reply window is the longest a codec on the `surfaces` queue names,
    /// and none where no codec names one.
    #[test]
    fn the_reply_window_is_the_longest_a_surfaces_codec_names() {
        let config = |codecs: &str| -> onemessagebus::Config {
            serde_norway::from_str(&format!(
                "version: 1\ntransport: {{kind: local}}\nprofile: planner-channel\ncodecs:\n{codecs}"
            ))
            .expect("a configuration")
        };
        let two = config(
            "  a: {queue: surfaces, reply_window_seconds: 30, select: op, frames: {}}\n  b: {queue: surfaces, reply_window_seconds: 3000, select: op, frames: {}}\n  c: {queue: replies, reply_window_seconds: 9000, select: op, frames: {}}\n",
        );
        assert_eq!(reply_window(Some(&two)), Some(Duration::from_secs(3000)));
        let none = config("  a: {queue: surfaces, select: op, frames: {}}\n");
        assert_eq!(reply_window(Some(&none)), None);
        assert_eq!(reply_window(None), None);
    }
}
