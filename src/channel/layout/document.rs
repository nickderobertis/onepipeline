//! The `planner-channel` layout as the document a host's bus links.
//!
//! The bus reads a layout either as code a program links or as data a program
//! publishes: a schema bundle carrying a [`LayoutDocument`], which a bus
//! configuration names in its `schemas` key and resolves under the bus's
//! schema-link rules. This module is that document for this crate's own layout,
//! built through the bus's own types from what the compiled-in
//! [`PlannerChannel`](super::PlannerChannel) declares — its name, its queues, its
//! operations and authors off [`allowlist`](super::allowlist), and the schemas
//! its queues and steps name off [`registry`](super::registry) — so a host's
//! `onemessagebus` reads and writes the channel directory this engine does.
//!
//! One part of the layout is code rather than data: how an offer to each queue
//! is prepared, which [`PlannerChannel`](super::PlannerChannel)'s `prepare`
//! does in Rust. [`prepare`] declares the same preparation once in the bus's
//! step vocabulary. The engine keeps running its compiled-in `prepare`; the
//! steps are what a host's bus runs, and `tests/layout_document.rs` offers every
//! recorded channel history and every refusal case through both and holds them
//! to the same records — naming, case by case, where the step vocabulary cannot
//! say what this crate's code says.
//!
//! The committed copy is [`DOCUMENT_PATH`], declared at [`DOCUMENT_VERSION`], and
//! `tests/contract.rs` fails when it differs from [`bundle_json`].

use std::collections::BTreeMap;

use onemessagebus::sdk_schema::RegistryDocument;
use onemessagebus::{
    BundleVersion, CheckStep, FieldPath, GrantOps, GrantStep, Grants, LayoutAuthor, LayoutDocument,
    LayoutName, MemberName, NonEmpty, OpWord, Predicate, PrepareStep, RefusalReason, RenameStep,
    Route, RouteStep, SchemaBundle, SchemaId, StampStep, VersionStep,
};
use serde_json::Value;

use super::{
    allowlist, name, queues, registry, source, Op, COMMANDS, COMMAND_OUTCOME_SCHEMA,
    PLANNER_CHANNEL, QUEUED_COMMANDS_SCHEMA, QUEUED_REPLY_SCHEMA, READ_REPLY_ENVELOPE_SCHEMA,
    REPLIES, REPLY_ENVELOPE_VERSION, REPLY_ENVELOPE_VERSIONS_READ, SURFACES, SURFACE_SCHEMA,
};

/// Where the published document is committed, relative to the repository root:
/// what a host's configuration links at
/// `https://raw.githubusercontent.com/nickderobertis/onepipeline/v<version>/<this>`.
pub const DOCUMENT_PATH: &str = "schemas/planner-channel.json";

/// The version the published document declares — what a link's pin is held
/// against. Raised with any change to what the document says: its major where
/// a channel directory one version writes is one the other cannot read.
pub const DOCUMENT_VERSION: &str = "1.0.0";

/// The reply envelope family, whose every version this build reads the bundle
/// carries.
const REPLY_ENVELOPE_SCHEMA: SchemaId = SchemaId::literal("agent", "reply-envelope", 3);

/// Where an envelope sits in a record offered to [`REPLIES`]: at the top level
/// for a bare envelope, under `reply` for a framed one.
#[derive(Clone, Copy)]
enum Envelope {
    Bare,
    Framed,
}

impl Envelope {
    fn at(self, member: &str) -> FieldPath {
        match self {
            Self::Bare => field(member),
            Self::Framed => field(&format!("reply.{member}")),
        }
    }

    /// Whether an offer holds this shape: a framed one carries `reply`.
    fn when(self) -> Predicate {
        let framed = Predicate::Present {
            field: field("reply"),
            present: true,
        };
        match self {
            Self::Bare => Predicate::Not(Box::new(framed)),
            Self::Framed => framed,
        }
    }
}

fn field(text: &str) -> FieldPath {
    text.parse()
        .unwrap_or_else(|_| unreachable!("{text} is a field path"))
}

fn member(text: &str) -> MemberName {
    MemberName::new(text).unwrap_or_else(|why| unreachable!("{text} is a member name: {why}"))
}

fn members(texts: &[&str]) -> NonEmpty<MemberName> {
    NonEmpty::new(texts.iter().map(|text| member(text)).collect())
        .unwrap_or_else(|why| unreachable!("{texts:?} names a member: {why}"))
}

fn refusal(text: &str) -> RefusalReason {
    serde_json::from_value(Value::from(text))
        .unwrap_or_else(|why| unreachable!("{text:?} is a refusal: {why}"))
}

/// The grant checks this crate's `allows` and `ensure_declared` make, in their
/// words: an undeclared author, a word that is no op, and an op the author is
/// not granted.
fn grant(author: FieldPath, ops: GrantOps, when: Option<Predicate>) -> GrantStep {
    GrantStep {
        author,
        default_author: Some(onemessagebus::Author::from("planner")),
        ops,
        refusal: Some(refusal(
            "'{op}' is not an op the {author} may issue: {reason}. Surface it to the planner \
             instead",
        )),
        unknown: Some(refusal(
            "'{op}' is not an op of the planner channel; the ops are: {ops}",
        )),
        undeclared: Some(refusal(
            "the envelope's author `{author}` is not declared; the declared authors are: \
             {authors}",
        )),
        malformed: Some(refusal("the envelope's author: {why}")),
        when,
    }
}

/// The checks one reply envelope passes where it sits in the offer, in the order
/// this crate's `route_envelope` makes them: its shape, its author, a completion
/// only an author granted `complete` may declare, the version an edit envelope
/// must be read at, and each command's op.
///
/// A bare envelope's version is read at [`REPLY_ENVELOPE_VERSION`] where it is
/// one this build reads, as the envelope routed on is; a framed one is checked
/// and kept whole, so a version this build reads passes it unrewritten.
fn envelope_checks(at: Envelope) -> Vec<PrepareStep> {
    let when = at.when();
    let also = |predicate: Predicate| Some(Predicate::All(vec![when.clone(), predicate]));
    let older: Vec<u64> = REPLY_ENVELOPE_VERSIONS_READ
        .iter()
        .filter(|version| **version != REPLY_ENVELOPE_VERSION)
        .map(|version| u64::from(*version))
        .collect();
    let edits = Predicate::NonEmpty {
        field: at.at("commands"),
        non_empty: true,
    };
    let (reads, required_when) = match at {
        Envelope::Bare => (older, edits),
        Envelope::Framed => {
            let read_as_current = older
                .into_iter()
                .map(|version| Predicate::equals(at.at("version"), version))
                .collect();
            (
                Vec::new(),
                Predicate::All(vec![
                    edits,
                    Predicate::Not(Box::new(Predicate::Any(read_as_current))),
                ]),
            )
        }
    };
    vec![
        PrepareStep::Check(CheckStep {
            schema: READ_REPLY_ENVELOPE_SCHEMA,
            at: match at {
                Envelope::Bare => None,
                Envelope::Framed => Some(field("reply")),
            },
            refusal: refusal("the reply is malformed: {why}"),
            when: Some(when.clone()),
        }),
        PrepareStep::Grant(grant(
            at.at("author"),
            GrantOps::AuthorOnly,
            Some(when.clone()),
        )),
        PrepareStep::Grant(GrantStep {
            refusal: Some(refusal(
                "declaring the run complete is not something the {author} may do: {reason}. \
                 Surface it to the planner instead",
            )),
            ..grant(
                at.at("author"),
                GrantOps::Word(OpWord(Op::Complete.word().to_owned())),
                also(Predicate::equals(at.at("completion"), true)),
            )
        }),
        PrepareStep::Version(VersionStep {
            at: at.at("version"),
            value: u64::from(REPLY_ENVELOPE_VERSION),
            reads,
            required_when: Some(required_when),
            refusal: refusal("an edit envelope requires version {value}"),
            when: Some(when.clone()),
        }),
        PrepareStep::Grant(grant(
            at.at("author"),
            GrantOps::Each {
                each: at.at("commands"),
                op: field("op"),
            },
            Some(when),
        )),
    ]
}

/// How an offer to each queue is prepared, in the bus's step vocabulary: what
/// [`PlannerChannel`](super::PlannerChannel)'s `prepare` does in code.
///
/// - a surface's `about` becomes its `workstream`, and it is stamped
///   `queued_at` when it names none;
/// - a reply is checked where its envelope sits, and a bare one is routed by
///   its halves — its commands to [`COMMANDS`] and its verdict, stamped `at`,
///   to [`REPLIES`], an envelope carrying neither reaching [`REPLIES`];
/// - a command envelope is checked op by op against its author.
#[must_use]
pub fn prepare() -> BTreeMap<onemessagebus::QueueName, Vec<PrepareStep>> {
    let verdict = ["completion", "message", "reason"];
    let mut replies = envelope_checks(Envelope::Framed);
    replies.extend(envelope_checks(Envelope::Bare));
    replies.push(PrepareStep::Route(RouteStep {
        routes: NonEmpty::new(vec![
            Route {
                queue: name(COMMANDS),
                on: members(&["commands"]),
                take: members(&["author", "commands"]),
                under: None,
                stamp: Vec::new(),
            },
            Route {
                queue: name(REPLIES),
                on: members(&verdict),
                take: members(&[
                    "version",
                    "author",
                    "completion",
                    "message",
                    "reason",
                    "commands",
                ]),
                under: Some(member("reply")),
                stamp: vec![member("at")],
            },
        ])
        .unwrap_or_else(|why| unreachable!("two routes: {why}")),
        fallback: Some(name(REPLIES)),
        when: Some(Envelope::Bare.when()),
    }));
    BTreeMap::from([
        (
            name(SURFACES),
            vec![
                PrepareStep::Rename(RenameStep {
                    from: member(onemessagebus::ask::ABOUT),
                    to: member("workstream"),
                    when: None,
                }),
                PrepareStep::Stamp(StampStep {
                    member: member("queued_at"),
                    when: None,
                }),
            ],
        ),
        (name(REPLIES), replies),
        (
            name(COMMANDS),
            vec![PrepareStep::Grant(grant(
                field("author"),
                GrantOps::Each {
                    each: field("commands"),
                    op: field("op"),
                },
                None,
            ))],
        ),
    ])
}

/// The `planner-channel` layout as data: the compiled-in layout's name, queues,
/// operations and authors, and [`prepare`]'s steps.
///
/// # Panics
///
/// Never for this build's own layout: the bus's own checks over it are what
/// `tests/contract.rs` holds.
#[must_use]
pub fn document() -> LayoutDocument {
    let allowlist = allowlist();
    let vocabulary: Vec<OpWord> = Op::ALL
        .iter()
        .map(|op| OpWord(op.word().to_owned()))
        .collect();
    let authors = allowlist
        .authors()
        .into_iter()
        .map(|author| {
            let granted = allowlist.granted(&author);
            let grants = if granted.len() == Op::ALL.len() {
                Grants::EveryOp
            } else {
                Grants::Only(
                    granted
                        .iter()
                        .map(|op| OpWord(op.word().to_owned()))
                        .collect(),
                )
            };
            let refusals = Op::ALL
                .iter()
                .filter(|op| !granted.contains(op))
                .filter_map(|op| {
                    let reason = allowlist.allows(&author, op).err()?.reason;
                    Some((OpWord(op.word().to_owned()), refusal(&reason)))
                })
                .collect();
            (author, LayoutAuthor { grants, refusals })
        })
        .collect();
    LayoutDocument::new(
        LayoutName::new(PLANNER_CHANNEL).unwrap_or_else(|why| unreachable!("{why}")),
        Some(format!(
            "onepipeline's planner channel: the surfaces a planner is asked about \
             (`{SURFACES}`, answered on `{REPLIES}`), the replies and command envelopes it \
             writes, and what each envelope was answered with. Generated from the layout \
             this engine compiles in; a surface of source `{}` supersedes a waiting one.",
            source::CHECK_IN
        )),
        NonEmpty::new(queues()).unwrap_or_else(|why| unreachable!("four queues: {why}")),
        vocabulary,
        authors,
        prepare(),
    )
    .unwrap_or_else(|why| panic!("the planner-channel layout is not a layout document: {why}"))
}

/// The schemas the document's queues and steps name: its four records, the
/// envelope as its steps read one, and the reply envelope at every version this
/// build reads — off the compiled-in layout's own registry.
fn schemas() -> Vec<RegistryDocument> {
    let registry = registry();
    let mut ids = vec![
        SURFACE_SCHEMA,
        QUEUED_REPLY_SCHEMA,
        QUEUED_COMMANDS_SCHEMA,
        COMMAND_OUTCOME_SCHEMA,
    ];
    ids.push(READ_REPLY_ENVELOPE_SCHEMA);
    ids.extend(REPLY_ENVELOPE_VERSIONS_READ.iter().rev().map(|version| {
        REPLY_ENVELOPE_SCHEMA.at((*version)
            .try_into()
            .unwrap_or_else(|_| unreachable!("a reply envelope version is above zero")))
    }));
    ids.into_iter()
        .map(|id| RegistryDocument {
            schema: registry
                .schema(&id)
                .cloned()
                .unwrap_or_else(|| unreachable!("the layout registers {id}")),
            id,
        })
        .collect()
}

/// The schema bundle published at [`DOCUMENT_PATH`]: [`document`] and the
/// schemas it names, at [`DOCUMENT_VERSION`].
///
/// # Panics
///
/// Never for this build's own layout, as [`document`].
#[must_use]
pub fn bundle() -> SchemaBundle {
    SchemaBundle::with_layouts(
        DOCUMENT_VERSION
            .parse::<BundleVersion>()
            .unwrap_or_else(|why| unreachable!("{why}")),
        Some(
            "The planner-channel layout onepipeline publishes for a host's onemessagebus to link."
                .to_owned(),
        ),
        schemas(),
        vec![document()],
    )
    .unwrap_or_else(|why| panic!("the planner-channel bundle is not a schema bundle: {why}"))
}

/// [`bundle`] as the text committed at [`DOCUMENT_PATH`]: pretty-printed, with
/// a trailing newline.
///
/// # Panics
///
/// Never: a bundle serializes.
#[must_use]
pub fn bundle_json() -> String {
    let mut text = serde_json::to_string_pretty(&bundle()).expect("a bundle serializes");
    text.push('\n');
    text
}
