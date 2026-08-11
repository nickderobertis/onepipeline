//! What `onevcs`'s provider seam can serve, established by running it.
//!
//! `src/vcs.rs` reaches `onevcs` by spawning its CLI, and the standing intent is
//! to reach it by calling its library instead: [`onevcs::Providers`] takes a
//! `&dyn Vcs` and a `&dyn Hosting`, and `onevcs-testing` supplies a pair, so a
//! journey could drive real `onevcs` code with no git, no host, and no process.
//!
//! That migration is blocked on the sibling, and this is the file that says so in
//! code rather than in prose. Of the four operations `src/vcs.rs` performs, the
//! seam serves **one**: [`Vcs::open_session`]. `publish` and `session close`
//! start from a private on-disk session record that only the real
//! [`onevcs::Git`] writes, so handed a session a provider just opened they refuse
//! it as a session nobody opened — the seam is declared but those two commands go
//! around it. Reading a session's stream has no library entry point at all:
//! `onevcs events` writes it to process stdout and the reader behind it is
//! private, so an orchestrator following several publications at once cannot
//! attribute what it reads.
//!
//! **Every assertion here fails when the sibling closes the gap.** That is the
//! point: the next worker learns the migration is unblocked from a red test
//! naming the operation that grew, not from re-deriving it. When one fails, do
//! not adjust it — move `src/vcs.rs` onto the seam and delete the case.
//!
//! Offline and hermetic: the providers touch nothing but a scratch state root.

use clap::Parser;
use onevcs::cli::Cli;
use onevcs::registry::{Identity, RepoType, Workflow};
use onevcs::{Providers, SessionRequest, Vcs};
use onevcs_testing::{HostState, MemoryHost, MemoryVcs, VcsState};

/// The exit code `onevcs` gives invalid input, which is what refusing an unknown
/// session token is.
const INVALID: u8 = 2;

/// One test, not four, because every case here needs `ONEVCS_HOME` pointed at a
/// scratch root and that variable is process-global: four tests would set it from
/// four threads at once and read one another's state root.
#[test]
fn the_provider_seam_serves_session_open_and_nothing_else_this_crate_needs() {
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

    // Served. This is the one call `src/vcs.rs::session_open` could make today,
    // and it hands back every field that function needs.
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
    let token = session.token.0.clone();

    // And the session is real to `onevcs` itself: its own `events` command reads
    // the stream the provider wrote. So the refusals below are not a token the
    // state root has never heard of.
    assert_eq!(
        run(&["events", &token], &vcs, &host),
        0,
        "`onevcs events` could not read the stream a provider-opened session wrote, so the \
         refusals below prove nothing about the seam"
    );

    // Not served. `publish` loads a private session record that only the real
    // `Git::open_session` writes, so it refuses the session it was just handed.
    // While this holds, a publication journey cannot run on the providers, which
    // is the whole of what blocks the migration.
    assert_eq!(
        run(&["publish", &token], &vcs, &host),
        INVALID,
        "`onevcs publish` now accepts a session opened through the `Vcs` seam — the migration \
         in src/vcs.rs is unblocked for publication; move it and delete this case"
    );

    // Not served, for the same reason: a node that settles has to release its
    // session, and closing goes around the seam too.
    assert_eq!(
        run(&["session", "close", &token], &vcs, &host),
        INVALID,
        "`onevcs session close` now accepts a session opened through the `Vcs` seam — the \
         migration in src/vcs.rs is unblocked for closing; move it and delete this case"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Run one `onevcs` command line in this process against the two providers,
/// returning its exit code.
fn run(argv: &[&str], vcs: &dyn Vcs, hosting: &dyn onevcs::Hosting) -> u8 {
    let mut line = vec!["onevcs"];
    line.extend_from_slice(argv);
    onevcs::run_with(&Cli::parse_from(line), Providers { vcs, hosting })
}
