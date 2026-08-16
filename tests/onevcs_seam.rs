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

use onevcs::registry::{Identity, RepoType, Workflow};
use onevcs::{
    EventStream, Lifecycle, MergePolicy, Providers, PublishOutcome, PublishRequest, SessionRequest,
    Vcs,
};
use onevcs_testing::{HostState, MemoryHost, MemoryVcs, VcsState};

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
            workflow: Workflow::Remote,
            repo_type: RepoType::SingleOwner,
            gate: "true".to_owned(),
        }],
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
            base: Some("main".to_owned()),
            execution_checkout: None,
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
            .any(|envelope| envelope.kind == onevcs::EventKind::SessionOpened),
        "the session's own opening is not on its stream: {opening:?}"
    );
    assert!(
        opening
            .iter()
            .all(|envelope| envelope.stream == token.0 && envelope.source == onevcs::Source::Vcs),
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
            body: None,
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

    // `src/vcs.rs::publish` names no body, and since `onevcs` 0.4.0 nothing
    // downstream composes one. The title is the control: both are recorded at the
    // same point, so a title present and a body absent is the field being carried
    // rather than nothing being recorded.
    let host_state = host.state();
    assert_eq!(
        host_state
            .titles
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["feat: land it"],
        "the title the publication was given is not the one the host opened under"
    );
    assert!(
        host_state.bodies.is_empty(),
        "a publication that named no body opened a change request carrying one: {:?}",
        host_state.bodies
    );

    // What the publication wrote reaches the reader as the records appended since
    // the last read — never the whole stream again, which is what would put every
    // earlier record into the merged store twice.
    let publishing = stream.read().expect("the publication is on the stream");
    assert!(
        publishing
            .iter()
            .any(|envelope| envelope.kind == onevcs::EventKind::ChangeOpened),
        "the change request the publication opened is not on the stream: {publishing:?}"
    );
    assert!(
        !publishing
            .iter()
            .any(|envelope| envelope.kind == onevcs::EventKind::SessionOpened),
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

    let _ = std::fs::remove_dir_all(&root);
}
