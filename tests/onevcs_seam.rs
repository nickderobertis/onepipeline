//! The `onevcs` seam `src/vcs.rs` runs on, exercised through it.
//!
//! This crate reaches `onevcs` by **calling it**. All four operations a lifecycle
//! node performs are library entry points that take the seam's own
//! [`Providers`]: [`Vcs::open_session`], [`onevcs::publish`],
//! [`onevcs::close_session`], and [`onevcs::EventStream`]. This file drives each
//! of them against `onevcs-testing`'s two providers, so the seam `src/vcs.rs`
//! depends on is proven in code — with no git, no host, and no process.
//!
//! It replaces a tripwire. Until `onevcs 0.2.1` the seam served exactly one of
//! the four: `publish` and `session close` started from a private on-disk record
//! that only the real [`onevcs::Git`] wrote, so they refused a session a provider
//! had just opened, and reading a session's stream had no library entry point at
//! all. This file asserted each of those refusals and said to delete the case
//! when it stopped holding. They stopped holding; the cases are gone and
//! `src/vcs.rs` is on the seam.
//!
//! # Why the *journeys* still do not run here
//!
//! The migration above does **not** unblock moving the e2e journeys onto the
//! providers, and no release of `onevcs` can: an e2e here reaches `onepipeline`
//! as a spawned process — `AGENTS.md` fixes that, an in-process `main()` is not
//! an e2e — so for one to run on [`MemoryVcs`] the shipped binary would have to
//! link `onevcs-testing` and select it at runtime. Both this crate's `Cargo.toml`
//! and that crate's own documentation forbid exactly that: what it implements
//! must never be reachable from a release binary. Nor is there a seam to inject
//! through, `vcs` being a private module, and making it public would add an item
//! `docs/contract.md` does not name.
//!
//! So the providers' place in this repository is a test *inside* the crate —
//! this one — which reaches the seam directly and adds no public surface. The
//! journeys drive the real `onevcs` against a real git origin instead, which is
//! what `tests/e2e/real_vcs.rs` and `tests/smoke/` do.
//!
//! Offline and hermetic: the providers touch nothing but a scratch state root.

use onevcs::registry::Identity;
use onevcs::rules::Approvals;
use onevcs::{
    Check, CheckState, EventStream, Lifecycle, MergePolicy, Providers, PublishOutcome,
    PublishRequest, SessionRequest, Vcs,
};
use onevcs_testing::{HostState, MemoryHost, MemoryVcs, VcsState};

/// The body a publication carries, as a drafting dispatch would have written it.
const DRAFTED: &str = "## What\nIt landed.\n\n## Why\nUsers were waiting.";

/// One test, not four, because every case here needs `ONEVCS_HOME` pointed at a
/// scratch root and that variable is process-global: four tests would set it from
/// four threads at once and read one another's state root.
#[test]
fn every_operation_this_crate_performs_is_served_by_the_provider_seam() {
    let root = std::env::temp_dir().join(format!("onepipeline-seam-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch state root");
    std::env::set_var("ONEVCS_HOME", &root);

    let vcs = MemoryVcs::seeded(VcsState {
        identities: vec![Identity {
            origin: "github.com/owner/repo".to_owned(),
            gate: "true".to_owned(),
        }],
        // `change-auto` is the repository's own, so section 5's draft is held
        // under the one policy that would otherwise have armed the host's merge
        // on it — which is what "a draft is unmergeable, and this crate keeps it
        // so" has to be proven against. Section 3 narrows to `change-open`, which
        // a per-run policy may do.
        policy: Some(MergePolicy::ChangeAuto),
        // No approval required, so section 3's green `change-open` publication is
        // lifted and ends `ChangeOpen`; section 7 is where a team repository keeps
        // its green draft for review.
        approvals: Some(Approvals::None),
        ..VcsState::default()
    });
    let host = MemoryHost::seeded(HostState::default());
    let providers = Providers {
        vcs: &vcs,
        hosting: &host,
    };

    // 1. `src/vcs.rs::session_open`. Every field that function hands on is here.
    let session = vcs
        .open_session(SessionRequest {
            repo: "owner/repo".to_owned(),
            branch: Some("feature".to_owned()),
            branch_name: None,
            branch_prefix: None,
            base: Some("main".to_owned()),
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
            refuse_conflicts: false,
        })
        .expect("the seam opens a session");
    assert_eq!(session.branch, "feature");
    assert_eq!(session.base, "main");
    let token = session.token.clone();

    // 2. `src/vcs.rs::follow` and `::events`. A session's stream is readable as
    //    *values* — attributed to the session that wrote them, which is what lets
    //    an orchestrator following several publications at once tell whose record
    //    it is holding — and a second read hands back only what was appended
    //    since. That cursor is the whole of the follow.
    let mut stream = EventStream::open(&token).expect("the seam reads a session's stream");
    let opening = stream.read().expect("the opening is on the stream");
    assert_eq!(stream.session(), &token);
    assert!(
        opening
            .iter()
            .any(|envelope| envelope.kind
                == onemessagebus::Kind::from(onevcs::EventKind::SessionOpened)),
        "the session's own opening is not on its stream: {opening:?}"
    );
    assert!(
        opening.iter().all(|envelope| envelope.stream == token.0
            && envelope.source.as_str() == onevcs::SOURCE_WORD),
        "an envelope on this session's stream is attributed elsewhere: {opening:?}"
    );

    // 3. `src/vcs.rs::publish`. The session a provider opened publishes through
    //    the seam, and the answer is a value: a case to match on rather than the
    //    line of prose the command prints, which is what this crate used to read.
    let published = onevcs::publish(
        &providers,
        &token,
        &PublishRequest {
            policy: Some(MergePolicy::ChangeOpen),
            title: Some("feat: land it".parse().expect("a usable subject")),
            // The prose a reviewer reads, which `src/vcs.rs` hands on exactly as
            // it was drafted: nothing here composes or validates one, so this is
            // the whole of what the host is given.
            body: Some(DRAFTED.to_owned()),
            // Not a draft: this publication is the ordinary one, and section 5
            // below is where a reason is carried.
            draft: None,
        },
    )
    .expect("the seam publishes a session it opened");
    assert_eq!(published.session, token);
    assert_eq!(published.branch, "feature");
    assert_eq!(published.policy, MergePolicy::ChangeOpen);
    let PublishOutcome::ChangeOpen(url) = &published.outcome else {
        panic!("a change-open publication ended as {:?}", published.outcome);
    };
    assert!(url.as_str().contains("owner/repo"), "{url}");
    // And both reached the host, on the change request it opened. The sibling
    // used to compose a body of its own — the branch's subject echoed back — for
    // a request that named none, and this crate would have been shipping that as
    // its reviewers' description; a body this crate passed and the host never saw
    // would be a drafted change request nobody reads.
    let opened = host.state();
    assert_eq!(
        opened.bodies.values().collect::<Vec<_>>(),
        vec![DRAFTED],
        "the change request was not opened with the body the request carried"
    );
    assert!(
        opened.titles.values().any(|title| title == "feat: land it"),
        "the title the request named is not the one the change request carries: {:?}",
        opened.titles
    );

    // What the publication wrote reaches the reader as the records appended since
    // the last read — never the whole stream again, which is what would put every
    // earlier record into the merged store twice.
    let publishing = stream.read().expect("the publication is on the stream");
    assert!(
        publishing
            .iter()
            .any(|envelope| envelope.kind
                == onemessagebus::Kind::from(onevcs::EventKind::ChangeOpened)),
        "the change request the publication opened is not on the stream: {publishing:?}"
    );
    assert!(
        !publishing
            .iter()
            .any(|envelope| envelope.kind
                == onemessagebus::Kind::from(onevcs::EventKind::SessionOpened)),
        "a second read handed back records the first already relayed: {publishing:?}"
    );

    // 4. `src/vcs.rs::follow`'s ending condition, and `::session_close`. The
    //    record is what says a session is still open, and closing releases it.
    assert_eq!(
        onevcs::session(&providers, &token)
            .expect("the seam reads the record of a session it opened")
            .lifecycle,
        Lifecycle::Open
    );
    let closed = onevcs::close_session(&providers, &token)
        .expect("the seam closes a session it opened")
        .token;
    assert_eq!(closed, token);
    assert_eq!(
        onevcs::session(&providers, &token)
            .expect("a closed session is still addressable")
            .lifecycle,
        Lifecycle::Closed
    );

    // 5. `src/vcs.rs::publish`'s draft half, which is what a fast-adoption node
    //    settling complete-but-draft crosses this seam with. The whole of it is
    //    one field, and it decides three things a caller acts on: the outcome is
    //    `ChangeDraft` and not `ChangeOpen`, the host is holding the change, and
    //    **nothing merged it** — under `change-auto`, which is the policy that
    //    would otherwise have armed the host's own merge on it.
    let drafting = vcs
        .open_session(SessionRequest {
            repo: "owner/repo".to_owned(),
            branch: Some("adopts-early".to_owned()),
            branch_name: None,
            branch_prefix: None,
            base: Some("main".to_owned()),
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
            refuse_conflicts: false,
        })
        .expect("the seam opens a second session");
    let reason = onevcs::DraftReason::AwaitingRelease {
        awaiting: "github.com/owner/engine".to_owned(),
        target: "crate".parse().expect("a target name"),
        reference: "onevcs/s-1".to_owned(),
        because: "pinned to a branch until the engine releases".to_owned(),
    };
    let held = onevcs::publish(
        &providers,
        &drafting.token,
        &PublishRequest {
            policy: Some(MergePolicy::ChangeAuto),
            title: Some("feat: adopt early".parse().expect("a usable subject")),
            body: None,
            draft: Some(reason.clone()),
        },
    )
    .expect("the seam publishes a draft");
    let PublishOutcome::ChangeDraft(drafted_url) = &held.outcome else {
        panic!("a drafted publication ended as {:?}", held.outcome);
    };
    let holding = host.state();
    let drafted_id = holding
        .changes
        .iter()
        .find(|change| change.url == *drafted_url)
        .map(|change| change.id.clone())
        .expect("the host opened the change this publication reports");
    assert_eq!(
        holding.drafts.get(&drafted_id),
        Some(&reason),
        "the host was not given the reason the change is not ready: {:?}",
        holding.drafts
    );
    assert!(
        !holding.merges.contains_key(&drafted_id) && !holding.made_ready.contains(&drafted_id),
        "a change held as a draft was merged or made ready: {:?} {:?}",
        holding.merges,
        holding.made_ready
    );

    // And a publication of the same branch carrying **no** reason is what lifts
    // it — there is no second verb, because the caller that republishes with the
    // pin moved is the one saying the reason no longer holds.
    let lifted = onevcs::publish(
        &providers,
        &drafting.token,
        &PublishRequest {
            policy: Some(MergePolicy::ChangeAuto),
            title: Some("feat: adopt early".parse().expect("a usable subject")),
            body: None,
            draft: None,
        },
    )
    .expect("the seam lifts the draft it held");
    assert!(
        !matches!(lifted.outcome, PublishOutcome::ChangeDraft(_)),
        "the change is still reported as a draft after the reason was withdrawn: {:?}",
        lifted.outcome
    );
    assert!(
        host.state().made_ready.contains(&drafted_id),
        "the host was never asked to take the change out of its draft state"
    );

    // 6. `src/vcs.rs::opening_for`: a branch a session **cuts** is proposed as
    //    `branch_name`, a name to cut, and never as `branch`, a name to continue.
    //    The seam takes the proposal as the branch it opens on, and refuses a
    //    request naming both — the two answers to one question this crate never
    //    gives at once. What makes a proposal a ref — the sanitizer, the host's
    //    prefix and the collision suffix — is the real library's, and
    //    `tests/e2e/branch_template.rs` drives it over real git.
    let cut = vcs
        .open_session(SessionRequest {
            repo: "owner/repo".to_owned(),
            branch: None,
            branch_name: Some("ENG-123/build".to_owned()),
            branch_prefix: None,
            base: Some("main".to_owned()),
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
            refuse_conflicts: false,
        })
        .expect("the seam cuts a session at a proposed name");
    assert_eq!(cut.branch, "ENG-123/build");
    let refused = vcs
        .open_session(SessionRequest {
            repo: "owner/repo".to_owned(),
            branch: Some("feature".to_owned()),
            branch_name: Some("ENG-123/build".to_owned()),
            branch_prefix: None,
            base: Some("main".to_owned()),
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
            refuse_conflicts: false,
        })
        .expect_err("a request naming a branch to continue and a name to cut is refused");
    assert!(
        refused.to_string().contains("two answers to one question"),
        "{refused}"
    );

    // 7. `src/vcs.rs::outcome_of` and `src/lifecycle.rs`'s settlement, over the
    //    draft lifecycle: a reasonless publication opens its change request as a
    //    draft while the required checks run, and what it ends in is what a node
    //    settles on. Each identity below is its own scratch state root, because a
    //    provider numbers its sessions from one.
    the_draft_lifecycle_answers_what_a_node_settles_on(&root);

    let _ = std::fs::remove_dir_all(&root);
}

/// One reasonless publication of a fresh session, on its own state root.
fn published_through_the_lifecycle(
    root: &std::path::Path,
    name: &str,
    vcs: &MemoryVcs,
    host: &MemoryHost,
) -> (onevcs::Publication, Vec<onevcs::Envelope>) {
    let home = root.join(name);
    std::fs::create_dir_all(&home).expect("a scratch state root");
    std::env::set_var("ONEVCS_HOME", &home);
    let providers = Providers { vcs, hosting: host };
    let session = vcs
        .open_session(SessionRequest {
            repo: "owner/repo".to_owned(),
            branch: Some(name.to_owned()),
            branch_name: None,
            branch_prefix: None,
            base: Some("main".to_owned()),
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
            refuse_conflicts: false,
        })
        .expect("the seam opens a session");
    let published = onevcs::publish(
        &providers,
        &session.token,
        &PublishRequest {
            policy: None,
            title: Some("feat: land it".parse().expect("a usable subject")),
            body: None,
            draft: None,
        },
    )
    .expect("the seam publishes through the lifecycle");
    let events = EventStream::open(&session.token)
        .expect("the seam reads a session's stream")
        .read()
        .expect("the publication is on the stream");
    (published, events)
}

/// A repository side publishing under `policy` and `approvals`, with `drafts`.
fn lifecycle_vcs(
    policy: MergePolicy,
    approvals: Approvals,
    drafts: Option<onevcs::rules::Drafts>,
) -> MemoryVcs {
    MemoryVcs::seeded(VcsState {
        identities: vec![Identity {
            origin: "github.com/owner/repo".to_owned(),
            gate: "true".to_owned(),
        }],
        policy: Some(policy),
        approvals: Some(approvals),
        drafts,
        ..VcsState::default()
    })
}

/// One check as a host reports it.
fn check(name: &str, conclusion: &str, started_at: &str) -> Check {
    Check {
        name: name.to_owned(),
        status: "completed".to_owned(),
        conclusion: Some(conclusion.to_owned()),
        required: true,
        head: None,
        url: None,
        started_at: Some(started_at.to_owned()),
        completed_at: None,
    }
}

/// The records of one kind a stream carried.
fn of_kind(events: &[onevcs::Envelope], kind: onevcs::EventKind) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|envelope| envelope.kind == onemessagebus::Kind::from(kind))
        .map(|envelope| serde_json::Value::Object(envelope.payload.clone()))
        .collect()
}

fn the_draft_lifecycle_answers_what_a_node_settles_on(root: &std::path::Path) {
    let first = onevcs::ChangeId("1".to_owned());

    // Green on a team repository: kept as a draft for its user's review, which is
    // its own case — this crate settles it `done` under `change-review-draft`.
    let host = MemoryHost::seeded(HostState {
        checks: [(first.clone(), vec![check("lint", "success", "draft")])].into(),
        ..HostState::default()
    });
    let vcs = lifecycle_vcs(MergePolicy::ChangeOpen, Approvals::Required, None);
    let (published, events) = published_through_the_lifecycle(root, "kept", &vcs, &host);
    let PublishOutcome::ChangeReviewDraft(url) = &published.outcome else {
        panic!("a green team change ended as {:?}", published.outcome);
    };
    assert!(url.as_str().contains("owner/repo"), "{url}");
    let held = host.state();
    assert!(held.awaiting_checks.contains(&first), "{held:?}");
    assert!(
        held.made_ready.is_empty() && held.merges.is_empty(),
        "{held:?}"
    );
    let drafted = of_kind(&events, onevcs::EventKind::ChangeDrafted);
    assert_eq!(drafted.len(), 1, "{events:?}");
    assert_eq!(drafted[0]["kind"], "awaiting-checks");
    assert_eq!(
        of_kind(&events, onevcs::EventKind::DraftKeptForReview).len(),
        1
    );
    assert!(of_kind(&events, onevcs::EventKind::DraftLifted).is_empty());

    // A draft whose required check is skipped has not run: it is lifted early, the
    // lift says whether it warned, and the run the lift triggered is what settles.
    let host = MemoryHost::seeded(HostState {
        checks: [(first.clone(), vec![check("lint", "skipped", "draft")])].into(),
        checks_after_lift: [(first.clone(), vec![check("lint", "success", "ready")])].into(),
        ..HostState::default()
    });
    let vcs = lifecycle_vcs(MergePolicy::ChangeOpen, Approvals::None, None);
    let (published, events) = published_through_the_lifecycle(root, "early", &vcs, &host);
    assert!(
        matches!(published.outcome, PublishOutcome::ChangeOpen(_)),
        "{:?}",
        published.outcome
    );
    let early = of_kind(&events, onevcs::EventKind::DraftLiftedEarly);
    assert_eq!(early.len(), 1, "{events:?}");
    assert_eq!(early[0]["warned"], true, "{}", early[0]);
    let settled = of_kind(&events, onevcs::EventKind::ChecksSettled);
    assert_eq!(settled.len(), 1, "{events:?}");
    assert_eq!(settled[0]["verdict"], "passed");

    // A ready change whose required check concluded skipped settles as
    // `passed-with-skipped`, naming it — and every `change-check` carries the state
    // `onevcs` classified it as, with `skipped` distinct from `passed`.
    let host = MemoryHost::seeded(HostState {
        checks: [(
            first.clone(),
            vec![
                check("lint", "skipped", "ready"),
                check("test", "success", "ready"),
            ],
        )]
        .into(),
        ..HostState::default()
    });
    let vcs = lifecycle_vcs(
        MergePolicy::ChangeOpen,
        Approvals::None,
        Some(onevcs::rules::Drafts {
            disabled: Some(true),
            warn_on_early_lift: None,
        }),
    );
    let (published, events) = published_through_the_lifecycle(root, "skipped", &vcs, &host);
    assert!(
        matches!(published.outcome, PublishOutcome::ChangeOpen(_)),
        "{:?}",
        published.outcome
    );
    assert!(of_kind(&events, onevcs::EventKind::ChangeDrafted).is_empty());
    let settled = of_kind(&events, onevcs::EventKind::ChecksSettled);
    assert_eq!(settled.len(), 1, "{events:?}");
    assert_eq!(settled[0]["verdict"], "passed-with-skipped");
    assert_eq!(settled[0]["skipped"], serde_json::json!(["lint"]));
    // Every check the host reported is read through `onevcs`'s one classifier, and
    // its wire spelling — the `state` a relayed `change-check` carries — keeps
    // `skipped` apart from `passed`. The provider emits no `change-check` of its
    // own; `tests/e2e/draft_lifecycle.rs` reads the real one off a run's stream.
    let states: std::collections::BTreeMap<String, serde_json::Value> = host.state().checks[&first]
        .iter()
        .map(|check| {
            (
                check.name.clone(),
                serde_json::to_value(check.state()).expect("a state serializes"),
            )
        })
        .collect();
    assert_eq!(states["lint"], "skipped", "{states:?}");
    assert_eq!(states["test"], "passed", "{states:?}");
    assert_eq!(
        serde_json::to_value(CheckState::Skipped).expect("a state serializes"),
        "skipped"
    );
    assert_ne!(CheckState::Skipped, CheckState::Passed);
    assert_eq!(
        check("lint", "skipped", "ready").state(),
        CheckState::Skipped
    );
    assert!(!check("lint", "skipped", "ready").green());
}
