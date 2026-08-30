//! Adoption: when a node launches relative to its dependencies' **releases**.
//!
//! Every journey here drives the real repository side. `onevcs` is a library this
//! crate calls, so nothing substitutes what a publication did, what a release
//! probe answered, or what an acknowledgement recorded: the probe is a real
//! script committed into a real repository and run as a real subprocess, and the
//! human step is the sibling's own `acknowledge` operation, called the way a
//! person's `onevcs release acknowledge` calls it.
//!
//! The three behaviours, one journey each: a plan naming neither field behaving
//! exactly as it did before there were fields, a fast-adoption node receiving its
//! reference block and then its arrival note, and a published-adoption node held
//! and then started when its dependency's release answers. A fourth drives the
//! two release **styles** side by side, and proves the only differences between
//! them are where the readiness answer comes from and what is reported.
//!
//! Three more are about the **record of a release** rather than about when a node
//! starts: the sibling's three release kinds reaching this run's store through
//! the one reader and the one address this crate has — a session token — a launch
//! filter keeping them out again, and a host that releases nothing holding the
//! store it always held.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary, and the sibling these journeys are about — `onevcs` — is the real library, over
// real git, a real origin on disk, a real probe subprocess, and its own real acknowledge
// operation. `oneagentgraph` is substituted at its subprocess boundary so a journey states
// a dispatch outcome rather than paying for a model turn. `harness.rs` carries the same
// suppression and the full rationale.

use std::path::Path;

use crate::harness::{lifecycle, plan_of, project_id, Repository, World, REFUSED};
use onepipeline::plan::CROSS_REPO_REFERENCES_HEADING;
use serde_json::{json, Value};

/// The repository the *dependency* lands in, which is the one that releases.
const ENGINE: &str = "engine";

/// A release-targets document declaring one automated target for the engine
/// repository, answered by the probe the journey committed into it.
fn automated(script: &str) -> String {
    document(script, "")
}

/// The same, plus a **human-step** target beside the automated one — a target no
/// probe can answer, whose version is whatever a person records afterwards.
fn both_styles(script: &str) -> String {
    document(
        script,
        &format!("    - name: wheel\n      style: human-step\n      action: \"{ACTION}\""),
    )
}

/// This host's release-targets document: what the engine repository releases, and
/// what every other repository adopts.
///
/// Written at the one conventional path under the state root, which is the only
/// place `onevcs` looks — deliberately not reachable through a key on the
/// registry, because every build already in the field refuses a key it does not
/// know and the first host to configure a release target would stop them all.
fn document(script: &str, extra: &str) -> String {
    let extra: String = extra.lines().map(|line| format!("{line}\n")).collect();
    format!(
        "{}{extra}default:\n\x20 adoption: fast\n",
        repositories(script)
    )
}

/// The same document, plus a **second** repository that releases: a journey about
/// a node awaiting more than one release needs two things to await.
fn two_that_release(engine: &str, other_alias: &str, other: &str) -> String {
    format!(
        "{}\x20 - match: {{host: github.com, owner: owner, name: {other_alias}}}\n\
         \x20   default_target: crate\n\
         \x20   targets:\n\
         \x20   - name: crate\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {other}, timeout_seconds: 30}}\n\
         default:\n\x20 adoption: fast\n",
        repositories(engine),
    )
}

/// The document's version and its one rule for the engine repository, up to but
/// not including whatever a journey states after them.
fn repositories(script: &str) -> String {
    format!(
        "version: 1\n\
         repositories:\n\
         \x20 - match: {{host: github.com, owner: owner, name: engine}}\n\
         \x20   default_target: crate\n\
         \x20   targets:\n\
         \x20   - name: crate\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {script}, timeout_seconds: 30}}\n"
    )
}

/// What a person has to do for the human-step target, as the document states it
/// and as every rendering of the wait must carry it.
const ACTION: &str = "build the wheel and upload it to PyPI, then run onevcs release acknowledge";

/// A world with two repositories: the one that releases, and the one whose node
/// depends on it.
///
/// The dependency lands *outside* the consumer's repository, which is the whole
/// condition every behaviour here is keyed to.
fn two_repositories(world: &World) -> (Repository, Repository) {
    let consumer = world.repository("local-direct", &[]);
    let engine = world.extra_repository(ENGINE);
    (engine, consumer)
}

/// The consumer node: a lifecycle node in the *other* repository, depending on
/// the engine node.
fn consumer(adoption: Option<&str>) -> Value {
    let mut node = lifecycle("consumer", &["engine"]);
    if let Some(adoption) = adoption {
        node["adoption"] = json!(adoption);
    }
    node
}

/// The engine node: a lifecycle node in the repository that releases.
fn engine() -> Value {
    let mut node = lifecycle(ENGINE, &[]);
    node["repo"] = json!(ENGINE);
    node
}

/// Say what the probe answers from now on.
///
/// Whole or not at all, because the probe reads this file while a journey writes
/// it: a plain write truncates and then fills, and a probe that opens it in
/// between finds it there and holding nothing — which is neither the answer just
/// given nor the absent file that means no release. A rename is one step to every
/// reader, so a probe run gets the answer before or the answer after and never
/// half of either.
fn releases_at(answer: &Path, version: &str) {
    // Written whole, under a name no probe reads, and then moved onto the one
    // every probe does: what this avoids is a *reader* seeing half an answer, so
    // the file here is the complete one and the rename is the single step.
    let whole = answer.with_extension("next");
    std::fs::write(&whole, format!("{version}\n")).expect("the probe's answer is written");
    // Replacing a file another process holds open is refused rather than queued on
    // one of the two platforms this suite runs on, and a probe here opens this one
    // every poll. So the replacement is retried for longer than a probe run can
    // hold it, and says which file it could not replace if it never gets in.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match std::fs::rename(&whole, answer) {
            Ok(()) => return,
            Err(failure) if std::time::Instant::now() >= deadline => panic!(
                "the probe's answer {} could not replace the one before it: {failure}",
                answer.display()
            ),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// The three kinds this crate emits reach a planner through the **shipped
/// profile with no filter change**, and the sibling's own relayed kind is in the
/// store beside them.
///
/// Two halves of one promise, and they are different halves. `release-wait`,
/// `release-arrived`, and `release-adopted` are this crate's own, so the shipped
/// `planner` profile — `include: [{source: pipeline}]` — admits them without
/// anybody editing a filter. `release-probed` is `onevcs`'s, so the same profile
/// leaves it out exactly as it leaves out every other sibling kind, and an
/// unfiltered read is where it is seen.
#[test]
fn the_three_release_kinds_reach_a_planner_through_the_shipped_profile() {
    let world = watching("adoption-profile");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let waiting_repo = world.extra_repository("tool");
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    releases_at(&answer, "0.1.0");

    // One fast node, to be told when the release arrives, and one published node
    // awaiting the human-step target nobody will acknowledge, so its wait is
    // raised while the other's arrival is reported.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let mut waiter = consumer(Some("published"));
    waiter["id"] = json!("waiter");
    waiter["repo"] = json!("tool");
    waiter["title"] = json!("feat: ship waiter");
    waiter["consumes"] = json!({"engine": "wheel"});
    let run = start(
        &world,
        "adoption-profile",
        vec![engine(), consumer(Some("fast")), waiter],
    );

    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });
    releases_at(&answer, "0.2.0");
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        world.until(&format!("a {kind} to be recorded"), |world| {
            !world.events_of(&run, kind).is_empty()
        });
    }

    // The shipped profile, which is what a reader naming none is given.
    let planner = world.run(&["monitor", &run]);
    planner.exited(0);
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        assert!(
            planner.stdout.contains(kind),
            "`{kind}` did not reach a planner reading through the shipped profile:\n{}",
            planner.stdout
        );
    }
    assert!(
        !planner.stdout.contains("release-probed"),
        "the planner profile admitted a sibling's kind, so it is not `source: pipeline`:\n{}",
        planner.stdout
    );

    // And the sibling's own kind is in the store, which an unfiltered read shows.
    world
        .run(&["monitor", &run, "--all"])
        .exited(0)
        .out_has("release-probed");

    world.release("consumer.go");
    world.run(&["stop", &run]).exited(0);
    let _ = waiting_repo;
}

/// The sibling's `release-probed` reaches this run's store **exactly as its
/// producer wrote it**.
///
/// Held against the sibling's own copy rather than against a shape restated here:
/// the same envelope is read back through `onevcs`'s own reader, out of the
/// session stream it was written on, and compared field by field with the one in
/// the merged store. The single documented difference is the `node` label, which
/// this crate stamps because the producer cannot know it — and which the contract
/// permits precisely because an enricher never rewrites a key the producer
/// stamped.
#[test]
fn the_siblings_release_probed_is_relayed_exactly_as_its_producer_wrote_it() {
    let world = watching("adoption-relay");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let run = start(&world, "adoption-relay", vec![engine()]);
    world.until("the engine to settle", |world| {
        !world.events_of(&run, "node-settled").is_empty()
    });

    // What the producer wrote, read through the producer's own reader.
    let token = session_of(&world, &run, ENGINE);
    let written: Vec<Value> = sibling_stream(&world, &token)
        .into_iter()
        .filter(|event| event["kind"] == "release-probed")
        .collect();
    assert_eq!(
        written.len(),
        1,
        "the publication's baseline capture probed once: {written:?}"
    );

    let relayed = world.events_of(&run, "release-probed");
    assert_eq!(relayed.len(), 1, "{relayed:?}");
    for field in [
        "v",
        "ts",
        "stream",
        "seq",
        "source",
        "kind",
        "phase",
        "payload",
        "artifacts",
    ] {
        assert_eq!(
            relayed[0][field], written[0][field],
            "the relay rewrote `{field}`"
        );
    }
    // It is the *session's* own record, numbered in that stream's series: the
    // publication captured its baselines while the follow was reading it.
    assert_eq!(relayed[0]["stream"], json!(token.0));
    // The one key the relay adds, and the only one: what the producer stamped is
    // still exactly what it stamped.
    assert_eq!(relayed[0]["labels"]["node"], json!("engine"));
    for (key, value) in written[0]["labels"]
        .as_object()
        .expect("the producer stamped labels")
    {
        assert_eq!(
            &relayed[0]["labels"][key], value,
            "the relay rewrote `{key}`"
        );
    }
}

/// The sibling's `release-observed` and `release-acknowledged` reach this run's
/// store, through the **public session reader** and one address this crate
/// already holds.
///
/// Both are recorded on the *identity's* own release record rather than on any
/// session's — a release happens long after the dispatch that produced the work
/// has ended, outside every session, which is why `release-observed` carries the
/// landing commit as the only thing that could correlate it. `onevcs` 0.14.0
/// makes that correlation itself: `EventStream::open_filtered` takes the session
/// token this crate already has and hands back the releases that carried *that
/// session's* landing beside the session's own records. So nothing here — in
/// `src/` or in this file — spells, derives, or knows the name of the second
/// stream, which is what `docs/contract-divergences.md` entry 40 refused to do.
///
/// Both kinds are produced for real: the first by the sibling's own
/// `release status`, the way a person's `onevcs release status` asks, and the
/// second by its own `acknowledge`. And the identity's record is **shared** by
/// every session in that repository, so a stranger's landing is put on it first:
/// what must reach this run is its own landing's releases and only those.
#[test]
fn the_siblings_other_two_release_kinds_reach_this_run_through_the_public_session_reader() {
    let world = watching("adoption-relayed-releases");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    releases_at(&answer, "0.1.0");

    // Somebody else's work in the same repository, landed by a run of its own —
    // a run that is **still going** when this journey reads what it left, held
    // open by a node of its own exactly as the run below is.
    //
    // A finished run is not the same arrangement, and it is not one this journey
    // can be written against. `onevcs` forgets the record of a session whose
    // owner process has gone and whose run root nobody is inside, and the next
    // launch that asks who holds the engine repository — which is the launch of
    // the run below — is what forgets it. That record is what the sibling's
    // own reader resolves a session's landing through, so reading this session
    // after it went would hand back a stream with no releases correlated to it
    // at all: a release that was recorded, and a journey that could only report
    // it as one that never happened.
    let stranger = elsewhere_in_the_engine_repository();
    world.script("keeper.wait", "hold");
    let other_run = start(
        &world,
        "adoption-relayed-stranger",
        vec![stranger, crate::harness::agent("keeper", &[])],
    );
    world.until("the stranger's work to land", |world| {
        world
            .events_of(&other_run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "stranger")
    });
    let strangers_landing = landing_commit(&world, &other_run, "stranger");
    let strangers_branch = branch_of(&world, &other_run, "stranger");
    let strangers_token = session_of(&world, &other_run, "stranger");

    // The run this journey is about: work that lands, and a node held on a
    // **human-step** release nobody has taken yet — so the run is still going
    // when the releases below happen, which is when they always happen.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let mut waiting = consumer(Some("published"));
    waiting["consumes"] = json!({"engine": "wheel"});
    let run = start(&world, "adoption-relayed-releases", vec![engine(), waiting]);
    world.until("the engine to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "engine")
    });
    let token = session_of(&world, &run, ENGINE);
    let landed = branch_of(&world, &run, "engine");
    let landing = landing_commit(&world, &run, "engine");

    // A release the probe can see, asked about both landings: the automated
    // target answers for each, so the identity's record carries one apiece.
    releases_at(&answer, "0.2.0");
    for reference in [&strangers_branch, &landed] {
        let answered = world.on_onevcs(|| {
            onevcs::release_status(reference, None).expect("the sibling answers about the landing")
        });
        // Held where the release is made rather than only where it is read back:
        // every other answer this can give — not landed, not released, a probe
        // that could not answer — leaves the identity's record with nothing on it
        // for this landing, and a journey that noticed that later could only say
        // that a release had not arrived. Here it says which reference was not
        // released and what `onevcs` said instead.
        assert!(
            matches!(answered, onevcs::ReleaseStatus::Released { .. }),
            "the sibling did not read {reference} as released, so nothing recorded a release \
             of that landing: {answered:?}"
        );
    }
    // And the human step somebody records, which is the second kind — and the
    // one that ends the held node's wait.
    world.on_onevcs(|| {
        onevcs::acknowledge_release(
            &landed,
            &"wheel".parse().expect("a target name"),
            "1.0.0",
            false,
        )
        .expect("the release is acknowledged")
    });

    for kind in ["release-observed", "release-acknowledged"] {
        world.until(&format!("a {kind} to reach the run"), |world| {
            !world.events_of(&run, kind).is_empty()
        });
    }

    // Read back through the same public reader the run relays through, and held
    // field for field: what is in the store is what the producer wrote, with the
    // context the producer could not know filled in beside it.
    let written = sibling_stream(&world, &token);
    let observed = world.events_of(&run, "release-observed");
    let acknowledged = world.events_of(&run, "release-acknowledged");
    assert_eq!(
        acknowledged.len(),
        1,
        "the human step was acknowledged once: {acknowledged:?}"
    );
    for relayed in observed.iter().chain(&acknowledged) {
        let producers = written
            .iter()
            .find(|event| event["stream"] == relayed["stream"] && event["seq"] == relayed["seq"])
            .unwrap_or_else(|| panic!("the reader does not hand back {relayed:?}"));
        for field in [
            "v",
            "ts",
            "stream",
            "seq",
            "source",
            "kind",
            "phase",
            "payload",
            "artifacts",
        ] {
            assert_eq!(
                relayed[field], producers[field],
                "the relay rewrote `{field}`"
            );
        }
        assert_eq!(relayed["phase"], json!("release"));
        for (key, value) in producers["labels"]
            .as_object()
            .expect("the producer stamped labels")
        {
            assert_eq!(&relayed["labels"][key], value, "the relay rewrote `{key}`");
        }
        // The context the producer could not have: which run, and whose work the
        // release carried. Filled in beside what it stamped, never over it.
        assert_eq!(relayed["labels"]["run_id"], json!(run));
        assert_eq!(relayed["labels"]["node"], json!("engine"));
        // And it is *this* landing's release. The stranger's is on the same
        // record, one line away, and is not this run's.
        assert_eq!(relayed["payload"]["landing_commit"], json!(landing));
    }

    // Both targets answered for this landing: the machine one that was probed,
    // and the one a person took the step for.
    let targets: Vec<String> = observed
        .iter()
        .filter_map(|event| event["payload"]["target"].as_str().map(str::to_string))
        .collect();
    for target in ["crate", "wheel"] {
        assert!(
            targets.contains(&target.to_string()),
            "no release was observed for the {target} target: {targets:?}"
        );
    }

    // The stranger's release really was recorded, one line away on the same
    // identity's record: the absence below is a correlation that held rather
    // than a release that never happened.
    assert!(
        sibling_stream(&world, &strangers_token)
            .iter()
            .any(|event| {
                event["kind"] == "release-observed"
                    && event["payload"]["landing_commit"] == json!(strangers_landing)
            }),
        "nothing released the stranger's landing, so this journey proves nothing about \
         which landing a release belongs to"
    );

    // And it is absent from this run entirely — not relabelled, not filtered
    // later: never relayed.
    for event in world.journal(&run) {
        assert_ne!(
            event["payload"]["landing_commit"],
            json!(strangers_landing),
            "another landing's release reached this run: {event}"
        );
    }

    // The releases keep the record's own identity and numbering, which is not
    // the session's: `seq` is a series over every session in that repository.
    let streams: Vec<String> = observed
        .iter()
        .chain(&acknowledged)
        .filter_map(|event| event["stream"].as_str().map(str::to_string))
        .collect();
    assert!(
        streams.iter().all(|stream| *stream == streams[0]),
        "the releases were not relayed as one producer's stream: {streams:?}"
    );
    assert_ne!(
        streams[0], token.0,
        "a release was relayed as though the session had written it"
    );

    // And the probe stays exactly one record. It is written on the session's own
    // stream while the follow is reading it, so a second reader that counted it
    // again would report one ask as two.
    let probed = world.events_of(&run, "release-probed");
    assert_eq!(
        probed.len(),
        1,
        "the probe arrived more than once: {probed:?}"
    );
    assert_eq!(probed[0]["stream"], json!(token.0));
    no_record_arrived_twice(&world, &run);

    // A reader naming no profile sees them, because they are in the store.
    world
        .run(&["monitor", &run, "--all"])
        .exited(0)
        .out_has("release-observed")
        .out_has("release-acknowledged");

    world.release("consumer.go");
    world.release("keeper.go");
    world.run(&["stop", &run]).exited(0);
    world.run(&["stop", &other_run]).exited(0);
}

/// A branch two sessions have worked on has its release attributed through the
/// **newest** of them, not the one it superseded.
///
/// The shape a retry leaves behind: a run lands work on a pinned branch and
/// finishes, and a later run pinned to the same name continues it — so `onevcs`
/// cuts a *second* session onto that branch and records the first as superseded.
/// Two run clones of one branch name then exist, each holding a landing, and
/// only one of them is the work.
///
/// It is the dangerous half of the correlation and it is why this is a journey
/// rather than an assertion: what a release is measured against is the landing
/// the branch resolves to, so a reader answering from the superseded copy names
/// the *previous* landing — and a release of the newest work then correlates to
/// nothing and is silently never reported. Both directions are held: the release
/// that reaches the run carries the second landing and not the first, and the
/// superseded session's own record resolves to that same landing when it is read
/// through the sibling's public reader.
#[test]
fn a_release_of_retried_work_is_attributed_through_the_newest_session_of_its_branch() {
    let world = watching("adoption-retried-release");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // The run that goes first: it lands its work on a pinned branch and leaves
    // that branch — and the session that made it — behind, **still going** while
    // this journey reads what it left, held open by a node of its own.
    //
    // Held for the reason the stranger's run above is, and the reading below is
    // the one that needs it: `onevcs` forgets the record of a session whose
    // owner process has gone and whose run root nobody is inside, and the launch
    // that asks who holds the engine repository — the run below — is what
    // forgets it. That record is what the sibling's reader resolves a session's
    // landing through, so a superseded session read after its run went is handed
    // a stream with no releases correlated to it at all.
    //
    // Which of the two questions retains a record is not the same on every host.
    // Being *inside* a run root is a question a host has to be able to answer
    // about a process it did not start, and one that cannot answer it at all
    // reads every run root as empty — so there the owner process is the only
    // thing that can retain a record, and a run allowed to end takes the
    // superseded copy's answer with it. A held run is the one arrangement that
    // retains it wherever this journey runs.
    let mut stranded = lifecycle("stranded", &[]);
    stranded["repo"] = json!(ENGINE);
    stranded["branch"] = json!(RETRIED);
    world.script("keeper.wait", "hold");
    let first = start(
        &world,
        "adoption-retried-first",
        vec![stranded, crate::harness::agent("keeper", &[])],
    );
    world.until("the first attempt to land", |world| {
        world
            .events_of(&first, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "stranded")
    });
    let superseded = session_of(&world, &first, "stranded");
    let first_landing = landing_commit(&world, &first, "stranded");

    // The retry: the same branch, in a run of its own, which continues it by
    // cutting a **second** session onto its tip. Its own work, because a tree
    // its base already carries publishes nothing and would land no second time.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let mut retried = engine();
    retried["branch"] = json!(RETRIED);
    let run = start(
        &world,
        "adoption-retried-release",
        vec![retried, consumer(Some("published"))],
    );
    world.until("the retry to land", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == ENGINE)
    });
    let newest = session_of(&world, &run, ENGINE);
    let landing = landing_commit(&world, &run, ENGINE);

    // Two sessions, one branch, two landings: the state this is about.
    assert_ne!(
        newest.0, superseded.0,
        "the retry took up the first run's session instead of continuing its branch, so \
         there is no superseded copy to answer from"
    );
    assert_ne!(
        landing, first_landing,
        "both runs landed the same commit, so no answer could tell them apart"
    );

    // The release happens, and it is the run's own watch that asks — the held
    // node is waiting on exactly this.
    releases_at(&answer, "0.2.0");
    world.until("a release-observed to reach the run", |world| {
        !world.events_of(&run, "release-observed").is_empty()
    });

    let observed = world.events_of(&run, "release-observed");
    for event in &observed {
        assert_eq!(
            event["payload"]["landing_commit"],
            json!(landing),
            "the release was correlated to the superseded copy's landing"
        );
        assert_ne!(event["payload"]["landing_commit"], json!(first_landing));
        assert_eq!(event["labels"]["node"], json!(ENGINE));
        assert_eq!(event["labels"]["run_id"], json!(run));
        assert_eq!(event["phase"], json!("release"));
    }
    no_record_arrived_twice(&world, &run);

    // And the other direction, at the seam the relay reads through: the
    // superseded session's own record resolves along its retry chain to the
    // newest, so a reader handed *either* token answers about the same landing.
    // A reader that stopped at the superseded record would answer that this
    // branch had not landed — which is the answer that invites re-running work
    // that already merged.
    for token in [&superseded, &newest] {
        assert!(
            sibling_stream(&world, token).iter().any(|event| {
                event["kind"] == "release-observed"
                    && event["payload"]["landing_commit"] == json!(landing)
            }),
            "the session {token:?} does not resolve to the landing its branch reached"
        );
    }

    world.release("consumer.go");
    world.release("keeper.go");
    world.run(&["stop", &run]).exited(0);
    world.run(&["stop", &first]).exited(0);
}

/// The branch two sessions work on, one after the other.
const RETRIED: &str = "feature/retried";

/// A launch whose `vcs` filter excludes the release kinds relays none of them,
/// and is otherwise the run it was.
///
/// The control an operator already has, through the seam the source filter
/// already crosses: the filter is handed to the sibling's own reader, which
/// applies it to the releases it correlated as well as to the session's own
/// records. Nothing new is declared anywhere to say so.
#[test]
fn a_launch_that_excludes_the_release_kinds_relays_none_of_them() {
    let world = watching("adoption-relay-excluded");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    releases_at(&answer, "0.1.0");

    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let mut waiting = consumer(Some("published"));
    waiting["consumes"] = json!({"engine": "wheel"});
    let run = start_with(
        &world,
        "adoption-relay-excluded",
        vec![engine(), waiting],
        &["--filter-vcs", r#"{"exclude": [{"kind": "release-*"}]}"#],
    );
    world.until("the engine to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "engine")
    });
    let token = session_of(&world, &run, ENGINE);
    let landed = branch_of(&world, &run, "engine");

    releases_at(&answer, "0.2.0");
    world.on_onevcs(|| {
        onevcs::release_status(&landed, None).expect("the sibling answers about the landing")
    });
    world.on_onevcs(|| {
        onevcs::acknowledge_release(
            &landed,
            &"wheel".parse().expect("a target name"),
            "1.0.0",
            false,
        )
        .expect("the release is acknowledged")
    });

    // The releases really happened, and the reader really reaches them: this is
    // a filter keeping them out rather than nothing having been written.
    let kinds: Vec<String> = sibling_stream(&world, &token)
        .into_iter()
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect();
    for kind in ["release-probed", "release-observed", "release-acknowledged"] {
        assert!(
            kinds.contains(&kind.to_string()),
            "the sibling recorded no `{kind}`, so this journey proves nothing about a filter: \
             {kinds:?}"
        );
    }

    // The held node starting is this run having seen the acknowledgement, so
    // every pass that could have relayed one has run.
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });
    for kind in ["release-probed", "release-observed", "release-acknowledged"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "`{kind}` reached a store whose launch excluded it"
        );
    }
    // Narrowed, not silenced: the same session's other records are all there.
    assert!(
        !world.events_of(&run, "session-closed").is_empty(),
        "the filter silenced the session rather than narrowing it"
    );
    no_record_arrived_twice(&world, &run);

    world.release("consumer.go");
    world.run(&["stop", &run]).exited(0);
}

/// A node in the repository that releases, belonging to somebody else's run.
fn elsewhere_in_the_engine_repository() -> Value {
    let mut node = lifecycle("stranger", &[]);
    node["repo"] = json!(ENGINE);
    node
}

/// The `onevcs` session one node published from, as the run recorded it opening.
fn session_of(world: &World, run: &str, node: &str) -> onevcs::SessionToken {
    world
        .journal(run)
        .into_iter()
        .find(|event| {
            event["source"] == "vcs"
                && event["labels"]["node"] == node
                && event["payload"]["clone"].is_string()
        })
        .and_then(|event| event["payload"]["token"].as_str().map(str::to_string))
        .map(onevcs::SessionToken)
        .unwrap_or_else(|| panic!("nothing recorded the session {node} published from"))
}

/// No two records of one run's store are the same producer's same record.
///
/// A relayed record arriving twice is the same defect as one lost, seen from the
/// other side — and a run that reads one session through two readers is exactly
/// where it would happen. `(stream, seq)` is what says so: every producer numbers
/// its own stream monotonically, so one pair is one record.
fn no_record_arrived_twice(world: &World, run: &str) {
    let mut seen: std::collections::BTreeSet<(String, u64)> = std::collections::BTreeSet::new();
    for event in world.journal(run) {
        let Some(stream) = event["stream"].as_str() else {
            continue;
        };
        let Some(seq) = event["seq"].as_u64() else {
            continue;
        };
        assert!(
            seen.insert((stream.to_owned(), seq)),
            "{stream} #{seq} is in the store twice: {event}"
        );
    }
}

/// A plan whose `consumes` names something the node does not depend on is
/// refused where the plan is read, and told which key and why.
///
/// The refusal a planner actually meets: silently dropping the target would
/// launch the node against the wrong artifact and say nothing about it.
#[test]
fn a_plan_consuming_a_dependency_it_does_not_have_is_refused_by_name() {
    let world = World::new("adoption-refusal");
    let mut node = lifecycle("consumer", &["engine"]);
    node["consumes"] = json!({"packager": "crate"});
    let path = world.plan("refused", &plan_of("refused", vec![engine(), node]));
    world
        .run(&["start", &path, "--detach"])
        .exited(REFUSED)
        .err_has("node 'consumer'")
        .err_has("`consumes` names 'packager'")
        .err_has("not one of this node's deps");
}

/// A `published` node whose repository declares release targets but **not the one
/// it consumes** holds indefinitely, and says so: an unanswerable question is not
/// an answer that the release has happened.
#[test]
fn a_target_this_host_cannot_name_holds_the_node_rather_than_releasing_it() {
    let world = watching("adoption-unanswerable");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // A release is out and past the baseline, so nothing about the *release* is
    // what holds this node.
    releases_at(&answer, "9.9.9");

    let mut node = consumer(Some("published"));
    node["consumes"] = json!({"engine": "wheel"});
    let run = start(&world, "adoption-unanswerable", vec![engine(), node]);
    world.until("the wait to be surfaced", |world| {
        !wait_surface(world, &run, "consumer").is_empty()
    });

    assert!(
        !dispatched(&world, &run, "consumer"),
        "a node awaiting a target this host cannot name was started anyway"
    );
    let entries = awaiting(&world, &run, "consumer");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0]["target"],
        json!("wheel"),
        "the wait does not name the target the plan asked for"
    );
    assert_eq!(
        entries[0]["style"],
        json!(null),
        "a target this host declares nothing for was given a style"
    );
    assert_eq!(
        entries[0]["last_answer"],
        json!("not-answered"),
        "a question that could not be put was recorded as an answer"
    );
    let surface = wait_surface(&world, &run, "consumer");
    assert!(
        surface.contains("no release target this host can name"),
        "the surface does not say why nothing can answer:\n{surface}"
    );
    // A question that can never be put is the other shape a wait takes, and a
    // reader is owed the same thing about it: no record of this wait was written
    // before a surface had told somebody the same.
    every_wait_was_surfaced_before_it_was_recorded(&world, &run, "consumer");
    world.run(&["stop", &run]).exited(0);
}

/// A probe that **answered unusably** holds the node exactly as one that answered
/// "not yet" does, and is reported as the different thing it is.
///
/// The whole distinction, driven against a real probe: "not answered" never
/// releases a hold and is never recorded as "not released" anywhere — not in the
/// scheduler, not in the payload, and not in the surface a person reads.
#[test]
fn a_probe_that_could_not_answer_holds_the_node_and_is_never_read_as_not_released() {
    let world = watching("adoption-unanswered");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // A version at the landing, so the baseline the publication captures is one
    // a later answer can be compared against.
    releases_at(&answer, "0.1.0");

    let run = start(
        &world,
        "adoption-unanswered",
        vec![engine(), consumer(Some("published"))],
    );
    world.until("the probe's answer to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
    });

    // Now the probe answers something that is not a version at all. The sibling
    // cannot say whether it carries the change, so it does not — and neither
    // does this run.
    releases_at(&answer, "whatever-the-nightly-was");
    world.until("the probe's failure to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-answered".to_owned())
    });
    assert!(
        !dispatched(&world, &run, "consumer"),
        "a probe that could not answer started a node"
    );
    let surface = wait_surface(&world, &run, "consumer");
    assert!(
        surface.contains("last answer: not-answered"),
        "the surface reports a probe that could not answer as something else:\n{surface}"
    );
    assert!(
        !surface.contains("last answer: not-released"),
        "a probe that could not answer was read as a release that has not happened:\n{surface}"
    );
    // And the surface beside *every* record this run wrote said what that record
    // said — which is the same promise, held where a reader's timing cannot
    // decide whether it holds. The two asserts above read the newest of each and
    // agree only if the store is caught between the two appends of one report.
    every_wait_was_surfaced_before_it_was_recorded(&world, &run, "consumer");

    // And only a release starts it, which is what says the hold was the hold and
    // not the probe being broken.
    releases_at(&answer, "0.2.0");
    world.until("the release to start the held node", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    for event in world.events_of(&run, "node-settled") {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a probe that could not answer failed a node: {event}"
        );
    }
}

/// An answer this host **cannot read** is never read as a release that has not
/// happened — driven at the one state of the probe where the two readings meet.
///
/// The sibling journey to
/// [`a_probe_that_could_not_answer_holds_the_node_and_is_never_read_as_not_released`],
/// and the reason it exists is that this is where the distinction is *lossy*. A
/// probe that prints something unusable is unanswered all the way down; a probe
/// that prints **nothing** on exit 0 is `onevcs`'s spelling of "this target has no
/// release", and a host holding a baseline reports that as **not released**. So an
/// answer file that is there and empty — which is exactly what a write caught half
/// done leaves behind, on every platform, and which the slowest one loses most
/// often — would arrive at a reader as a release that has not happened rather than
/// as a question this host got nothing back from.
///
/// Nothing here needs a particular platform to reach it: the state is put there
/// outright rather than raced for, so the verdict lands the same everywhere the
/// suite runs.
#[test]
fn an_answer_this_host_cannot_read_is_never_read_as_a_release_that_has_not_happened() {
    let world = watching("adoption-unreadable");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // A version at the landing, so the baseline the publication captures is one a
    // later answer is compared against — which is the whole condition: with no
    // baseline there is nothing for "no release" to be reported as not past.
    releases_at(&answer, "0.1.0");

    let run = start(
        &world,
        "adoption-unreadable",
        vec![engine(), consumer(Some("published"))],
    );
    world.until("the probe's answer to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
    });

    // The answer file, there and holding nothing. No release is spelled by the
    // file not being there at all, so this is a probe with nothing to say rather
    // than a target with nothing released.
    std::fs::write(&answer, "").expect("the half-written answer is left behind");
    world.until("the probe's failure to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-answered".to_owned())
    });
    assert!(
        !dispatched(&world, &run, "consumer"),
        "a probe with no answer to give started a node"
    );
    let surface = wait_surface(&world, &run, "consumer");
    assert!(
        surface.contains("last answer: not-answered"),
        "the surface reports an answer this host cannot read as something else:\n{surface}"
    );
    assert!(
        !surface.contains("last answer: not-released"),
        "an answer this host cannot read was reported as a release that has not \
         happened:\n{surface}"
    );
    // And no record of this wait ever said it either, at any moment of the run —
    // the reading this journey refuses is one a reader's timing must not be able
    // to catch.
    every_wait_was_surfaced_before_it_was_recorded(&world, &run, "consumer");
    for event in world.events_of(&run, "release-wait") {
        for entry in event["payload"]["awaiting"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_ne!(
                entry["last_answer"],
                json!("released"),
                "a probe with no answer to give released a hold: {event}"
            );
        }
    }

    // And the hold was the hold: a readable answer past the baseline starts it.
    releases_at(&answer, "0.2.0");
    world.until("the release to start the held node", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    for event in world.events_of(&run, "node-settled") {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a probe with no answer to give failed a node: {event}"
        );
    }
}

/// An unusable poll or surface bound falls back to the shipped one rather than to
/// zero or to no bound at all, and the run behaves.
///
/// Zero is the reading that matters: a poll of zero spends the host on probes,
/// and a surface interval of zero raises the wait on every reconcile pass — which
/// is the one line this host's operating rules say may never be filtered.
#[test]
fn an_unusable_bound_leaves_the_run_behaving_as_the_shipped_one_does() {
    let world = watching("adoption-bounds")
        .with_env("ONEPIPELINE_RELEASE_POLL_SECONDS", "nonsense")
        .with_env("ONEPIPELINE_RELEASE_SURFACE_SECONDS", "0");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let run = start(
        &world,
        "adoption-bounds",
        vec![engine(), consumer(Some("published"))],
    );
    // The watch was not disabled: the hold is on and the wait is raised.
    world.until("the wait to be surfaced", |world| {
        !wait_surface(world, &run, "consumer").is_empty()
    });
    assert!(!dispatched(&world, &run, "consumer"));

    // Nor was it read as **zero**, which is the reading that matters: the
    // reconcile loop runs a pass every 25ms, so a surface interval of zero would
    // raise the wait dozens of times over the two reads below — each of which is
    // what a person waiting on a held run actually does.
    for _ in 0..2 {
        world.run(&["status", &run]).exited(0);
    }
    let surfaced = world
        .events_of(&run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "release-wait")
        .count();
    assert_eq!(
        surfaced, 1,
        "an unusable surface bound was read as zero, so the wait was raised on every pass"
    );
    world.run(&["stop", &run]).exited(0);
}

/// Every envelope the sibling wrote about one session, read back through
/// the sibling's own reader.
///
/// One reader and one address: the **session's** token, which this crate already
/// holds. `onevcs` joins the identity's own release record to the session whose
/// landing commit it names, so what comes back is that session's records and the
/// releases that carried its work — and nothing here spells, derives, or knows
/// the name of the second stream.
fn sibling_stream(world: &World, token: &onevcs::SessionToken) -> Vec<Value> {
    world.on_onevcs(|| {
        let mut stream = onevcs::EventStream::open(token)
            .unwrap_or_else(|error| panic!("the sibling's stream {token:?} reads: {error}"));
        stream
            .read()
            .expect("the sibling's own reader reads its own stream")
            .into_iter()
            .map(|envelope| serde_json::to_value(envelope).expect("an envelope serializes"))
            .collect()
    })
}

/// The task prose one node's dispatch was handed, read off the `--task` the
/// launch really carried.
///
/// Off the invocation rather than off the stream, because a journey that holds a
/// turn open asserts on the prose *while it is held* — and a held turn has not
/// reported an activity yet. The launch's own argv is what the dispatch was
/// composed with, which is exactly the question.
fn task_of(world: &World, node: &str) -> String {
    tasks_of(world, node).pop().unwrap_or_default()
}

/// The task prose every one of a node's dispatches was handed, in order.
fn tasks_of(world: &World, node: &str) -> Vec<String> {
    // Either shape a node's own prose takes here: `lifecycle`'s and `agent`'s.
    let mine = [
        format!("## What\nShip {node}."),
        format!("## What\nDo {node}."),
    ];
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| mine.iter().any(|prose| task.starts_with(prose)))
        .collect()
}

/// Whether a node has been dispatched at all.
fn dispatched(world: &World, run: &str, node: &str) -> bool {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == node)
}

/// The `awaiting` entries of the last `release-wait` raised about one node.
fn awaiting(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "release-wait")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)
        .and_then(|event| event["payload"]["awaiting"].as_array().cloned())
        .unwrap_or_default()
}

/// The answer the last `release-wait` recorded for the one release a node awaits.
///
/// `None` before any wait has been raised about it, which is a run that has not
/// got there yet rather than a wait with no answer — `not-answered` is a thing
/// the payload says out loud.
fn answered(world: &World, run: &str, node: &str) -> Option<String> {
    awaiting(world, run, node)
        .first()?
        .get("last_answer")?
        .as_str()
        .map(str::to_owned)
}

/// The text of the last release-wait surface raised about one node.
fn wait_surface(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .rfind(|event| {
            event["payload"]["kind"] == "release-wait" && event["labels"]["node"] == node
        })
        .map(|event| {
            event["payload"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// Every wait this run **recorded** about one node had already been surfaced,
/// saying the same thing, by the time the record was written.
///
/// Which is the promise held where a reader's timing cannot decide whether it
/// holds. The record and the surface are two appends saying one thing, so a
/// journey that reads the newest of each and hopes they agree only fails on a
/// host slow enough between the two to be caught in the middle. The order they
/// were written in is in the store afterwards, at any speed.
fn every_wait_was_surfaced_before_it_was_recorded(world: &World, run: &str, node: &str) {
    let mut surface = String::new();
    let mut recorded = 0usize;
    for event in world.journal(run) {
        if event["labels"]["node"] != json!(node) {
            continue;
        }
        match event["kind"].as_str().unwrap_or_default() {
            "planner-surface-queued" if event["payload"]["kind"] == json!("release-wait") => {
                surface = event["payload"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
            }
            "release-wait" => {
                recorded += 1;
                for entry in event["payload"]["awaiting"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                {
                    let identity = entry["identity"].as_str().unwrap_or_default();
                    let named = match entry["target"].as_str() {
                        Some(target) => format!("{identity} {target}"),
                        None => identity.to_owned(),
                    };
                    let said = entry["last_answer"].as_str().unwrap_or_default();
                    let line = surface
                        .lines()
                        .find(|line| line.starts_with(&format!("- {named} ")))
                        .unwrap_or_default();
                    assert!(
                        line.ends_with(&format!("last answer: {said}")),
                        "'{node}' recorded {named} at '{said}', and the surface a reader had \
                         beside that record said something else:\n{surface}"
                    );
                }
            }
            _ => {}
        }
    }
    assert!(
        recorded > 0,
        "no wait was recorded about '{node}' at all, so this proved nothing"
    );
}

/// Start a run detached, so the journey can move the world under a live loop.
///
/// Every node writes a file, because a lifecycle node whose dispatch changed
/// nothing publishes nothing — and a dependency that never landed has no release
/// to ask about. What each of these journeys is keyed to is a *landing*, so each
/// node's work has to be real.
fn start(world: &World, name: &str, nodes: Vec<Value>) -> String {
    start_with(world, name, nodes, &[])
}

/// The same, for a launch that also declares something about its own events.
///
/// The source filter is a **launch** decision — declared once, before a session
/// is cut — so a journey about what a filter keeps out of the store has to state
/// it here rather than after the run is going.
fn start_with(world: &World, name: &str, nodes: Vec<Value>, extra: &[&str]) -> String {
    for node in &nodes {
        let id = node["id"].as_str().expect("every node has an id");
        world.script(&format!("{id}.work"), &format!("{id} did its work\n"));
    }
    let path = world.plan(name, &plan_of(name, nodes));
    let mut argv: Vec<String> = vec!["start".to_owned(), path, "--detach".to_owned()];
    argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
    world
        .run(&argv.iter().map(String::as_str).collect::<Vec<&str>>())
        .exited(0);
    name.to_string()
}

/// How often a journey here lets this host run **one** release's probe.
///
/// [`watching`] is what sets it, from here rather than beside it: a journey that
/// counts probe runs against this bound and a world that ran under another one
/// would agree on nothing.
const POLL_SECONDS: u64 = 1;

/// How many asks
/// [`nodes_awaiting_one_release_put_one_question_and_are_answered_together`]
/// counts before it judges the rate. Enough that a host spending the poll budget
/// once per waiting node has to have spent it faster than the budget allows, and
/// small enough that the window is a few seconds.
const ASKS: usize = 5;

/// A world whose release watch answers on this journey's timescale rather than on
/// an operator's.
///
/// The two bounds are the shipped ones — 120 seconds between probes and 900
/// between surfaces — which are right for a run that waits days for a release and
/// wrong for a test that has to see both happen. Nothing else about the watch
/// changes: one hold, indefinite, released only by an answer of released.
fn watching(name: &str) -> World {
    World::new(name)
        .with_env(
            "ONEPIPELINE_RELEASE_POLL_SECONDS",
            &POLL_SECONDS.to_string(),
        )
        .with_env("ONEPIPELINE_RELEASE_SURFACE_SECONDS", "1")
}

/// A plan naming neither new field produces exactly the run it produced before
/// there were fields: no reference block, no hold, and a task that is the node's
/// own prose and nothing else.
///
/// The compatibility promise, driven where it is actually at risk — a run over
/// **two repositories**, which is the shape that grows a reference block the
/// moment a node opts in. A host with no release-targets document at all is the
/// other half of it, and this journey is that too: no repository here has a
/// release target, so nothing releases, nothing is recorded about a release, and
/// the sessions this run followed end where they always ended.
#[test]
fn a_plan_naming_neither_field_runs_exactly_as_it_did() {
    let world = World::new("adoption-unchanged");
    world.write_graphs();
    let (_engine, _consumer) = two_repositories(&world);

    let run = start(&world, "adoption-unchanged", vec![engine(), consumer(None)]);
    world.until("both nodes to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    let task = task_of(&world, "consumer");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a node naming no adoption gained a reference block: {task}"
    );
    assert_eq!(
        task,
        lifecycle("consumer", &["engine"])["task"]
            .as_str()
            .expect("the node states its task"),
        "the rendered task is not byte-identical to the node's own prose"
    );
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "a plan naming neither field recorded a `{kind}`"
        );
    }
    // And nothing the *sibling* records about releases either: a repository with
    // no release targets releases nothing, so there is no probe to relay and no
    // release record to read — the store is the store this run always held.
    for kind in ["release-probed", "release-observed", "release-acknowledged"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "a repository that releases nothing put a `{kind}` in the store"
        );
    }
    no_record_arrived_twice(&world, &run);
    // The run ended on its own, with both sessions closed: a follow that had
    // gone on reading for a release that cannot happen is a run that never
    // settles.
    assert_eq!(
        world.events_of(&run, "session-closed").len(),
        2,
        "a session this run followed did not close"
    );
    world.run(&["results", &run]).exited(0).out_has("merged");
}

/// A fast-adoption node launches on its dependency's **branch** readiness, is
/// handed the git references of the work it cannot yet pin a version to, and is
/// told — while it is still running — the moment the release arrives.
///
/// The whole arc in one journey, because the two halves are one promise: the
/// worker is given something to pin against *and* the correction that moves it
/// off that pin, without a person noticing and intervening.
#[test]
fn a_fast_node_pins_against_git_and_is_told_when_the_release_arrives() {
    let world = watching("adoption-fast");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // What is released when the engine's work lands, which is the baseline the
    // arrival is measured against.
    releases_at(&answer, "0.1.0");

    // The consumer's turn is held open, so the note has a running turn to reach.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");

    let run = start(
        &world,
        "adoption-fast",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });

    // It launched on the branch, not on a version: the block names the
    // repository, the branch, the landing commit, and the target.
    let task = task_of(&world, "consumer");
    let block = task
        .split_once(CROSS_REPO_REFERENCES_HEADING)
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or_else(|| panic!("the dispatched task carries no reference block:\n{task}"));
    assert!(
        block.contains("| dependency | repository | branch | commit | release target |"),
        "the block carries no table:\n{block}"
    );
    let row = block
        .lines()
        .find(|line| line.starts_with("| engine |"))
        .unwrap_or_else(|| panic!("no row for the engine dependency:\n{block}"));
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(cells[1], "github.com/owner/engine", "row: {row}");
    assert!(!cells[2].is_empty(), "the branch cell is empty: {row}");
    assert_eq!(
        cells[3],
        landing_commit(&world, &run, "engine"),
        "the commit cell is not the landing the run observed: {row}"
    );
    assert_eq!(cells[4], "crate", "row: {row}");
    assert!(
        task.contains("Pin against the git references below rather than against a version"),
        "the block does not say what it is for:\n{task}"
    );

    // Nothing has arrived yet: the probe answers exactly the baseline.
    assert!(
        world.events_of(&run, "release-adopted").is_empty(),
        "a note was delivered before any release arrived"
    );

    // The release happens. The still-running node is told, once, into its live
    // turn — no person, no reply, no dispatch of its own.
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });

    let arrived = world.events_of(&run, "release-arrived");
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(arrived[0]["payload"]["node"], json!("consumer"));
    assert_eq!(arrived[0]["payload"]["dep"], json!("engine"));
    assert_eq!(
        arrived[0]["payload"]["identity"],
        json!("github.com/owner/engine")
    );
    assert_eq!(arrived[0]["payload"]["target"], json!("crate"));
    assert_eq!(arrived[0]["payload"]["style"], json!("automated"));
    assert_eq!(arrived[0]["payload"]["version"], json!("0.2.0"));

    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(
        adopted[0]["payload"]["delivery"],
        json!("live"),
        "the note did not reach the running turn"
    );
    assert_eq!(
        adopted[0]["payload"]["versions"],
        json!([{"identity": "github.com/owner/engine", "target": "crate", "version": "0.2.0"}])
    );

    // The lever really was pulled, and the sibling's own record of it reached the
    // merged store stamped with the node it was about.
    let interrupted = world.events_of(&run, "turn-interrupted");
    eprintln!("DIAG kinds={:?}", world.kinds(&run));
    eprintln!(
        "DIAG invocations={:?}",
        world
            .invocations()
            .iter()
            .filter(|c| c["tool"] == "oneagentgraph")
            .map(|c| c["args"][0].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(true));
    assert_eq!(interrupted[0]["labels"]["node"], json!("consumer"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    // And the worker was told what the versions are, in a note that adds no bar.
    // Its own task prose cannot have carried this — it was rendered before the
    // release existed — so the redirection is the only way it got there.
    let note = redirected(&world, &run, "consumer");
    assert!(
        note.contains("github.com/owner/engine — crate 0.2.0")
            && note.contains("Move from the git pin to that released version"),
        "the running turn was not told which version arrived:\n{note}"
    );
    assert!(
        !note.to_lowercase().contains("acceptance criteria"),
        "the arrival note reads as a new bar:\n{note}"
    );
}

/// What one node's running turn was redirected with.
fn redirected(world: &World, run: &str, node: &str) -> String {
    world
        .journal(run)
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"] == node
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
                && event["payload"]["redirected"].is_string()
        })
        .map(|event| {
            event["payload"]["redirected"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// A fast-adoption node whose running turn has no lever is **not** told the note
/// reached one: the lever is really pulled, it really answers that there is no
/// turn, and the note is owed to the node's next dispatch instead.
///
/// The other half of `auto`, and the compatibility half: a harness with no
/// out-of-band turn control is what every `context` edit written before delivery
/// had modes ran under, and the note must be owed rather than lost. What a
/// deferred note then does — ride the next dispatch and be consumed by it — is
/// `a_deferred_arrival_note_rides_the_next_dispatch_and_is_consumed_by_it`, just
/// below.
#[test]
fn an_arrival_note_with_no_live_turn_to_reach_is_owed_to_the_next_dispatch() {
    let world = watching("adoption-deferred");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // A member on a harness with no out-of-band turn control: it runs, and there
    // is nothing to redirect.
    world.script("consumer.no-lever", "");
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(
        &world,
        "adoption-deferred",
        vec![engine(), consumer(Some("fast"))],
    );

    // After the engine has landed — so the baseline its publication captured is
    // the version that was out then rather than the one about to be — and after
    // the consumer's turn has opened, so what the note meets is a turn that
    // exists and has no lever rather than a dispatch that has not spoken yet.
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });

    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(
        adopted[0]["payload"]["delivery"],
        json!("next"),
        "a note with no turn to reach was recorded as having reached one"
    );
    // The lever was pulled and answered, which is what tells this apart from a
    // note nobody tried to deliver.
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));
    assert_eq!(interrupted[0]["labels"]["node"], json!("consumer"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    // The turn that was running never saw it, and its own prose could not have
    // carried it — the task was rendered before the release existed.
    let dispatched = tasks_of(&world, "consumer");
    assert_eq!(dispatched.len(), 1, "{dispatched:?}");
    assert!(
        !dispatched[0].contains("0.2.0"),
        "a version that did not exist at launch reached the dispatch that launched:\n{}",
        dispatched[0]
    );
    assert!(redirected(&world, &run, "consumer").is_empty());
}

/// A `run:<id>#<node>` dependency is pinned against git like any other outside
/// the node's repository, and its row is read out of the **upstream run's own
/// ledger**.
///
/// It is out-of-repository whatever repository it lands in: the branch belongs to
/// another run, so the stacked-branch machinery this crate has cannot reach it
/// and a git pin is the only thing a worker can hold.
#[test]
fn a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger() {
    let world = watching("adoption-crossdag");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // The upstream lands the engine's work in a run of its own, and settles.
    let upstream = start(&world, "adoption-upstream", vec![engine()]);
    world.until("the upstream to settle", |world| {
        world.run_file(&upstream, "result.json").is_file()
    });

    // A second run, whose one node depends on that node of that run.
    let mut across = consumer(Some("fast"));
    across["deps"] = json!([format!("run:{upstream}#engine")]);
    across["consumes"] = json!({format!("run:{upstream}#engine"): "crate"});
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(&world, "adoption-crossdag", vec![across]);
    world.until("the consumer's turn to open", |world| {
        !world.events_of(&run, "turn-started").is_empty()
    });

    // The row is the upstream run's, read off its ledger: this run never
    // dispatched the engine and has no settlement of its own to read.
    let task = task_of(&world, "consumer");
    let row = task
        .lines()
        .find(|line| line.starts_with(&format!("| run:{upstream}#engine |")))
        .unwrap_or_else(|| panic!("no row for the cross-DAG dependency:\n{task}"))
        .to_owned();
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(cells[1], "github.com/owner/engine", "row: {row}");
    assert_eq!(
        cells[2],
        branch_of(&world, &upstream, "engine"),
        "the branch cell is not the branch the upstream published from: {row}"
    );
    assert_eq!(
        cells[3],
        landing_commit(&world, &upstream, "engine"),
        "the commit cell is not the landing the upstream observed: {row}"
    );
    assert_eq!(cells[4], "crate", "row: {row}");

    // And the release it is waiting on reaches it, named by the dependency the
    // plan wrote rather than by a node this graph has.
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });
    let arrived = world.events_of(&run, "release-arrived");
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(
        arrived[0]["payload"]["dep"],
        json!(format!("run:{upstream}#engine"))
    );
    assert_eq!(arrived[0]["payload"]["version"], json!("0.2.0"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "node-settled").is_empty()
    });
}

/// A delivery that was attempted and **broke** leaves the note owed rather than
/// recorded, and the node is told once the lever works again.
///
/// The one answer that is neither "it reached the turn" nor "there was no turn to
/// reach": a run that recorded the note as delivered when the lever failed would
/// never try again, and the worker would go on pinning against git with the
/// release out.
#[test]
fn a_delivery_that_broke_leaves_the_note_owed_and_is_tried_again() {
    let world = watching("adoption-lever-broken");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let broken = world.fakes.join("interrupt.fail");
    world.script("interrupt.fail", "");
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(
        &world,
        "adoption-lever-broken",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });

    releases_at(&answer, "0.2.0");
    // The release arrives and the delivery breaks: the arrival is reported, and
    // the adoption is not — because it has not happened.
    world.until("the release to arrive", |world| {
        !world.events_of(&run, "release-arrived").is_empty()
    });
    assert!(
        world.events_of(&run, "release-adopted").is_empty(),
        "a note whose delivery broke was recorded as delivered"
    );

    // Mend the lever, and the note this run still owes is delivered.
    std::fs::remove_file(&broken).expect("the broken lever is mended");
    world.until("the note to be delivered", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });
    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(adopted[0]["payload"]["delivery"], json!("live"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    assert!(redirected(&world, &run, "consumer").contains("crate 0.2.0"));
}

/// The note a running turn could not take **rides the node's next dispatch**, and
/// is consumed by it.
///
/// The other half of the deferred delivery: the journey above proves the note is
/// *owed*, and this one proves the owing is honoured. What puts the node back on
/// the frontier is the pair the contract already documents for idling one and
/// picking it up again — `cancel` then `requeue` — and the second dispatch is
/// handed the note under `## Planner context`, disclaiming itself exactly as a
/// planner's own note does.
///
/// A direct agent node, so the second dispatch is a dispatch and nothing else:
/// a lifecycle node put back on the frontier re-opens a session and republishes a
/// branch its base already carries, which is a different journey's subject.
#[test]
fn a_deferred_arrival_note_rides_the_next_dispatch_and_is_consumed_by_it() {
    let world = watching("adoption-carried");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // No lever, so the note is deferred; and a second node held open beside it,
    // because the loop returns as soon as nothing can move and a requeue needs a
    // driver to pick it up.
    world.script("consumer.no-lever", "");
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    world.script("keeper.wait", "hold");
    let mut carried = crate::harness::agent("consumer", &[ENGINE]);
    carried["adoption"] = json!("fast");
    let run = start(
        &world,
        "adoption-carried",
        vec![engine(), carried, crate::harness::agent("keeper", &[])],
    );

    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });
    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted[0]["payload"]["delivery"], json!("next"));

    // Idle it and put it back: what it is handed the second time is what the
    // note was owed to. `cancel` takes a node that is pending or running, so it
    // comes while the turn is still held; `requeue` refuses while the dispatch it
    // asked to stop is still in flight, so it is retried until that one has gone.
    let envelope = |commands: Value| json!({"version": 1, "commands": commands}).to_string();
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "consumer"}])),
        )
        .exited(0);
    world.release("consumer.go");
    world.until("the cancelled dispatch to have gone", |world| {
        world
            .run_with_stdin(
                &["reply", &run],
                &envelope(json!([{"op": "requeue", "id": "consumer"}])),
            )
            .code
            == 0
    });
    world.until("the node to be dispatched again", |world| {
        tasks_of(world, "consumer").len() == 2
    });

    let dispatched = tasks_of(&world, "consumer");
    assert!(
        dispatched[1].contains("## Planner context")
            && dispatched[1].contains("github.com/owner/engine — crate 0.2.0")
            && dispatched[1].contains("Move from the git pin to that released version"),
        "the next dispatch was not handed the note it was owed:\n{}",
        dispatched[1]
    );
    assert!(
        dispatched[1].contains("adds no acceptance criteria"),
        "the carried note did not disclaim itself:\n{}",
        dispatched[1]
    );
    assert!(
        !dispatched[0].contains("0.2.0"),
        "a version that did not exist at launch reached the dispatch that launched"
    );

    // And it carried exactly one dispatch: nothing owes it again.
    world.release("keeper.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    assert_eq!(
        tasks_of(&world, "consumer")
            .iter()
            .filter(|task| task.contains("crate 0.2.0"))
            .count(),
        1,
        "the note outlived the dispatch that took it"
    );
}

/// A fast-adoption node whose dependency lands in its **own** repository gets no
/// reference block and waits for no release: the lifecycle already puts that
/// dependency's work under it, and nothing here changes that.
///
/// The other half of the fast-adoption promise, and the one that is easy to
/// break: the block exists for work a worker cannot reach from its own branch,
/// and a dependency in the same repository is exactly the work it can.
#[test]
fn a_dependency_inside_the_nodes_own_repository_is_not_pinned_against_git() {
    let world = watching("adoption-same-repo");
    world.write_graphs();
    let repository = world.repository("local-direct", &[]);
    let (script, answer) = world.probe_in(&repository, "service");
    // The consumer's *own* repository releases something, so nothing here is
    // spared a row by there being no release to wait for — which is what would
    // make this journey pass without proving anything.
    world.releases(&document(&script, "").replace("name: engine", "name: service"));
    releases_at(&answer, "0.1.0");
    let declares = world.on_onevcs(|| onevcs::release_targets("service"));
    assert!(
        declares.is_ok_and(|releases| !releases.targets.is_empty()),
        "this journey's own repository declares no release target, so it proves nothing"
    );

    let mut first = lifecycle("first", &[]);
    first["title"] = json!("feat: ship first");
    let mut second = lifecycle("second", &["first"]);
    second["title"] = json!("feat: ship second");
    second["adoption"] = json!("fast");
    let run = start(&world, "adoption-same-repo", vec![first, second]);
    world.until("both nodes to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    let task = task_of(&world, "second");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a dependency in the node's own repository was rendered as a git pin:\n{task}"
    );
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "a dependency in the node's own repository started a `{kind}`"
        );
    }
    // And both nodes' work reached the base, which is the second having been cut
    // from a base that already carried the first — the stacking this crate has
    // always done, unchanged.
    for node in ["first", "second"] {
        assert!(
            repository.base_file(&format!("{node}.md")).is_some(),
            "{node}'s work did not reach the base"
        );
    }
}

/// A published-adoption node is **not scheduled at all** while its
/// out-of-repository dependency is unreleased, and is started by nothing but an
/// answer of released.
#[test]
fn a_published_node_is_held_until_the_release_answers_and_by_nothing_else() {
    let world = watching("adoption-published");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let run = start(
        &world,
        "adoption-published",
        vec![engine(), consumer(Some("published"))],
    );
    world.until("the engine to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "engine")
    });
    // The wait is surfaced, and goes on being surfaced once the probe has
    // answered: a probe that ran and said the version has not moved is
    // `not-released`, which is an answer and still not a release.
    world.until("the probe's answer to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
    });

    // Its dependency has settled `done`, so it is ready by every rule the graph
    // has — and it has not been dispatched, because the release has not arrived.
    assert!(
        !dispatched(&world, &run, "consumer"),
        "a published node was dispatched with its dependency unreleased"
    );
    let entries = awaiting(&world, &run, "consumer");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["identity"], json!("github.com/owner/engine"));
    assert_eq!(entries[0]["target"], json!("crate"));
    assert_eq!(entries[0]["style"], json!("automated"));
    assert!(
        entries[0]["waited_seconds"].is_number() && entries[0]["since"].is_string(),
        "the wait does not say how long it has been: {entries:?}"
    );
    assert!(
        entries[0].get("action").is_none(),
        "an automated wait carries an action nobody has to perform: {entries:?}"
    );
    let surface = wait_surface(&world, &run, "consumer");
    assert!(
        surface.contains("automated release") && surface.contains("waiting on 1 release"),
        "the surface does not name what is awaited or how:\n{surface}"
    );
    assert!(
        surface.contains("Nothing times this out and nothing will fail the node"),
        "the surface does not say the wait is indefinite:\n{surface}"
    );

    // The wait is repeated rather than stated once, so it cannot go silent.
    world.until("the wait to be surfaced again", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == "release-wait")
            .count()
            > 1
    });
    // And nothing about the elapsed time started it.
    assert!(
        !dispatched(&world, &run, "consumer"),
        "waiting longer is what started a held node"
    );

    // Only the release does.
    releases_at(&answer, "0.2.0");
    world.until("the held node to run", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    assert!(dispatched(&world, &run, "consumer"));
    // It launched *after* the release, so it has a version to pin against and no
    // git reference block telling it otherwise.
    let task = task_of(&world, "consumer");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a node that waited for the release was told it had launched without one:\n{task}"
    );
    let settled = world.events_of(&run, "node-settled");
    for event in &settled {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a node the wait held was failed: {event}"
        );
    }
}

/// Nodes awaiting **one** release put one question between them, are answered
/// together, and say so while they wait.
///
/// Three promises, against the real probe: a wait still expecting its first
/// answer reads `no-answer-yet` and never `not-answered`, no node is left
/// reporting no answer once a node beside it has one, and the three of them cost
/// one probe run a poll between them — the count that says the first two are
/// structural rather than luck.
#[test]
fn nodes_awaiting_one_release_put_one_question_and_are_answered_together() {
    let world = watching("adoption-one-question");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // Three of them, because two can straddle a round by luck and three cannot:
    // asked one at a time, the third waits out both of the others.
    let waiters = ["first", "second", "third"];
    let mut nodes = vec![engine()];
    for id in waiters {
        let mut node = crate::harness::agent(id, &[ENGINE]);
        node["adoption"] = json!("published");
        nodes.push(node);
    }
    let run = start(&world, "adoption-one-question", nodes);
    world.until("every wait to carry the probe's answer", |world| {
        waiters
            .iter()
            .all(|node| answered(world, &run, node) == Some("not-released".to_owned()))
    });

    for node in waiters {
        assert!(!dispatched(&world, &run, node), "{node} was dispatched");
    }
    // What every wait said, in the order the store holds it, because both of the
    // next two promises are about a *sequence*: a node reading `no-answer-yet` is
    // right until the first answer lands and wrong from that moment on.
    let mut waited: Vec<(String, String)> = Vec::new();
    for event in world.journal(&run) {
        if event["kind"] != "release-wait" {
            continue;
        }
        let node = event["labels"]["node"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        for entry in event["payload"]["awaiting"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            waited.push((
                node.clone(),
                entry["last_answer"].as_str().unwrap_or_default().to_owned(),
            ));
        }
    }
    // Each node's first wait is raised before anything can have answered it, and
    // says so — `no-answer-yet`, and never `not-answered`, which is the word for
    // a probe that ran and could not answer. This probe answers.
    for node in waiters {
        let first = waited.iter().find(|(who, _)| who == node);
        assert_eq!(
            first.map(|(_, said)| said.as_str()),
            Some("no-answer-yet"),
            "'{node}' reported its first wait as something other than a question still out",
        );
    }
    assert!(
        !wait_surface(&world, &run, "first").is_empty()
            && world
                .events_of(&run, "planner-surface-queued")
                .iter()
                .any(|event| event["payload"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("last answer: no-answer-yet")),
        "no surface told a reader the probe was still out rather than broken"
    );
    // And no node was left reporting no answer after a node beside it had one.
    let mut answered_yet = false;
    for (node, said) in &waited {
        assert_ne!(
            said, "not-answered",
            "'{node}' reported a probe that answers as one that could not"
        );
        let carries = said == "not-released" || said == "released";
        assert!(
            carries || !answered_yet,
            "'{node}' still reports '{said}' for a release a node beside it has already been \
             answered about"
        );
        answered_yet |= carries;
    }
    assert!(
        answered_yet,
        "no wait carried the probe's answer at all, so this proved nothing"
    );

    // Counted over a window the probe's own tally marks out rather than over the
    // journey's elapsed time, because what is held is a **rate**: one probe run
    // for this release every `ONEPIPELINE_RELEASE_POLL_SECONDS`, however fast the
    // host is. One question per waiting node spends that budget three times over.
    world.until("the release to be asked about", |world| {
        world.probe_runs(ENGINE) >= 1
    });
    let before = world.probe_runs(ENGINE);
    let from = std::time::Instant::now();
    world.until(
        "the release to be asked about several times over",
        |world| world.probe_runs(ENGINE) >= before + ASKS,
    );
    let asked = world.probe_runs(ENGINE) - before;
    let over = from.elapsed().as_secs_f64();
    assert!(
        asked as f64 <= over / POLL_SECONDS as f64 + 2.0,
        "{asked} probes were run for one release in {over:.1}s, which is oftener than one \
         every {POLL_SECONDS}s — the nodes awaiting it are each buying their own copy of \
         one answer"
    );

    releases_at(&answer, "0.2.0");
    world.until("every node to settle", |world| {
        world.events_of(&run, "node-settled").len() == 4
    });
    let mut arrived = false;
    for event in world.journal(&run) {
        match event["kind"].as_str().unwrap_or_default() {
            "release-arrived" => arrived = true,
            "release-wait" => assert!(
                !arrived,
                "'{}' was still waiting on a release another node had already been handed: \
                 {event}",
                event["labels"]["node"],
            ),
            _ => {}
        }
    }
    assert!(arrived, "no release ever arrived, so this proved nothing");
    for event in world.events_of(&run, "node-settled") {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a node the wait held was failed: {event}"
        );
    }
}

/// The adoption mode resolves through **exactly four rungs**, and each of them
/// decides a node the rung beneath it would have decided differently.
///
/// Driven as behaviour rather than as a lookup, because the mode is not a value
/// anything reports: what a rung decides is whether the node is scheduled. So
/// each rung is proved by a pair — one node it holds beside one the next rung
/// down would have let go, and the other way round.
#[test]
fn the_adoption_mode_resolves_through_exactly_four_rungs() {
    let world = watching("adoption-rungs");
    world.write_graphs();
    let consumer_repo = world.repository("local-direct", &[]);
    let engine_repo = world.extra_repository(ENGINE);
    let unruled = world.extra_repository("tool");
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    // Rung 3, the global one, says `published`. Rung 2 says `fast` for the
    // `service` repository and says nothing at all for `tool`, which is what
    // leaves `tool` on rung 3.
    world.releases(&format!(
        "{}  - match: {{host: github.com, owner: owner, name: service}}\n\
         \x20   adoption: fast\n\
         default:\n\
         \x20 adoption: published\n",
        repositories(&script),
    ));
    releases_at(&answer, "0.1.0");

    // Rung 1 — the node's own field — against rung 4, the floor: two nodes with
    // no repository at all, one stating `published` and one stating nothing.
    let mut stated = crate::harness::agent("stated", &[ENGINE]);
    stated["adoption"] = json!("published");
    let floor = crate::harness::agent("floor", &[ENGINE]);
    // Rung 2 against rung 3: one node in the repository a rule names `fast`, and
    // one in a repository no rule names, which takes the global `published`.
    let mut by_repository = lifecycle("by-repository", &[ENGINE]);
    by_repository["title"] = json!("feat: ship by-repository");
    let mut by_global = lifecycle("by-global", &[ENGINE]);
    by_global["repo"] = json!("tool");
    by_global["title"] = json!("feat: ship by-global");

    let run = start(
        &world,
        "adoption-rungs",
        vec![engine(), stated, floor, by_repository, by_global],
    );
    world.until("the two waits to carry their own answer", |world| {
        answered(world, &run, "stated") == Some("not-released".to_owned())
            && answered(world, &run, "by-global") == Some("not-released".to_owned())
    });

    // The floor let a node go that the global rung above it would have held, and
    // the node's own field held one the floor would have let go.
    assert!(
        dispatched(&world, &run, "floor"),
        "rung 4 did not decide a node with no repository and no field of its own"
    );
    assert!(
        !dispatched(&world, &run, "stated"),
        "rung 1 did not win over the floor beneath it"
    );
    // The repository rung let a node go that the global rung would have held.
    assert!(
        dispatched(&world, &run, "by-repository"),
        "rung 2 did not win over rung 3"
    );
    assert!(
        !dispatched(&world, &run, "by-global"),
        "rung 3 did not decide a node no rule names"
    );

    releases_at(&answer, "0.2.0");
    world.until("every node to settle", |world| {
        world.events_of(&run, "node-settled").len() == 5
    });
    let _ = (consumer_repo, unruled);
}

/// Both release styles, side by side, through the sibling's own interface: one
/// node awaiting an automated target whose real probe answers, and one awaiting a
/// human-step target that only a person's acknowledgement can answer.
///
/// The point is what is *the same* — one hold, indefinite, neither failing nor
/// timing out — and what differs: where the readiness answer is obtained, and
/// what is reported.
#[test]
fn the_two_release_styles_take_one_scheduling_path_and_are_reported_apart() {
    let world = watching("adoption-styles");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    releases_at(&answer, "0.1.0");

    // In a repository of its own, because the two waits end at different moments
    // and two lifecycle nodes publishing from one checkout at once race each
    // other's fetch. Nothing here is about that.
    let packaging = world.extra_repository("tool");
    let mut on_the_wheel = consumer(Some("published"));
    on_the_wheel["id"] = json!("packager");
    on_the_wheel["repo"] = json!("tool");
    on_the_wheel["title"] = json!("feat: ship packager");
    on_the_wheel["consumes"] = json!({"engine": "wheel"});
    let run = start(
        &world,
        "adoption-styles",
        vec![engine(), consumer(Some("published")), on_the_wheel],
    );
    world.until("both waits to carry their own answer", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
            && answered(world, &run, "packager") == Some("awaiting-human-step".to_owned())
    });

    // The same hold: neither is dispatched, and neither is failed.
    for node in ["consumer", "packager"] {
        assert!(!dispatched(&world, &run, node), "{node} was dispatched");
    }

    // Told apart by the answer each obtained, and by what each reports.
    let automated = awaiting(&world, &run, "consumer");
    assert_eq!(automated[0]["style"], json!("automated"));
    assert!(automated[0].get("action").is_none());

    let human = awaiting(&world, &run, "packager");
    assert_eq!(human[0]["style"], json!("human-step"));
    assert_eq!(human[0]["target"], json!("wheel"));
    assert_eq!(
        human[0]["action"],
        json!(ACTION),
        "the wait does not carry the text the person needs"
    );

    let surface = wait_surface(&world, &run, "packager");
    assert!(
        surface.contains("human-step release") && surface.contains(ACTION),
        "the surface does not say a person has to act, or what they have to do:\n{surface}"
    );
    assert!(
        !wait_surface(&world, &run, "consumer").contains("human-step"),
        "an automated wait reads as one somebody has to act on"
    );

    // **No probe ran for the human-step target**, and the sibling's own
    // `release-probed` — relayed unchanged, like every other `onevcs` kind — is
    // the evidence: the publication's baseline capture probed the automated
    // target and nothing else.
    let probed = world.events_of(&run, "release-probed");
    assert!(!probed.is_empty(), "no probe was relayed at all");
    for event in &probed {
        assert_eq!(
            event["source"],
            json!("vcs"),
            "a relayed kind was rewritten"
        );
        assert_eq!(
            event["payload"]["target"],
            json!("crate"),
            "a probe was run for a target that has none: {event}"
        );
    }

    // The automated one is answered by its probe.
    releases_at(&answer, "0.2.0");
    world.until("the automated wait to end", |world| {
        dispatched(world, &run, "consumer")
    });
    assert!(
        !dispatched(&world, &run, "packager"),
        "the human-step wait ended when the automated one did"
    );

    // The human-step one is answered by the real acknowledge operation, run the
    // way the person who performed the release runs it.
    let landed = branch_of(&world, &run, "engine");
    world.on_onevcs(|| {
        onevcs::acknowledge_release(
            &landed,
            &"wheel".parse().expect("a target name"),
            "1.0.0",
            false,
        )
        .expect("the release is acknowledged")
    });
    world.until("the human-step wait to end", |world| {
        dispatched(world, &run, "packager")
    });

    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 3
    });
    for event in world.events_of(&run, "node-settled") {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a node one of the two waits held was failed: {event}"
        );
    }
    let _ = packaging;
    let arrived: Vec<Value> = world.events_of(&run, "release-arrived");
    assert!(
        arrived
            .iter()
            .any(|event| event["payload"]["style"] == json!("human-step")
                && event["payload"]["version"] == json!("1.0.0")),
        "the human-step release was not reported as one: {arrived:?}"
    );
}

/// The branch one node's work was published from, as its settlement recorded it.
///
/// The spelling `onevcs` resolves landed work by, which is what a person's own
/// `onevcs release acknowledge` is given.
fn branch_of(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == node)
        .and_then(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("nothing recorded which branch {node} published from"))
}

/// The commit one node's change reached its base at, as the run observed it.
fn landing_commit(world: &World, run: &str, node: &str) -> String {
    world
        .journal(run)
        .into_iter()
        .find(|event| {
            event["source"] == "vcs"
                && event["kind"] == "merge-completed"
                && event["labels"]["node"] == node
        })
        .and_then(|event| event["payload"]["sha"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("nothing recorded where {node}'s work landed"))
}

/// The rules this host publishes under when a journey needs a **change request**
/// to exist at all.
///
/// A draft is a state of a change request, so `local-direct` — which reaches the
/// base with git alone and opens none — has nothing to draft and `onevcs` refuses
/// a reason under it by name. The consumer therefore publishes `change-auto`,
/// which is also the policy that would arm the host's own merge on the change if
/// anything let it: "nothing merges a draft" is only worth asserting where
/// something would have.
///
/// Everything else stays `local-direct`, because the *dependency's* work has to
/// really reach its base branch for `onevcs` to resolve it as landed and answer
/// anything about a release of it at all — and a `change-*` publication here
/// lands through the `gh` stand-in, which moves no git ref.
fn consumer_opens_a_change(world: &World) {
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        "version: 3\n\
         rules:\n\
         \x20 - match: {host: github.com, owner: owner, name: service}\n\
         \x20   publication: change-auto\n\
         \x20   approvals: none\n\
         default:\n\
         \x20 publication: local-direct\n\
         \x20 approvals: none\n",
    )
    .expect("the rules file is written");
}

/// The two repositories every draft journey needs, publishing the way
/// [`consumer_opens_a_change`] describes.
fn two_repositories_opening_a_change(world: &World) -> (Repository, Repository) {
    let (engine, consumer) = two_repositories(world);
    consumer_opens_a_change(world);
    (engine, consumer)
}

/// Every `gh` invocation this world's stand-in recorded, as its argument list.
fn gh_calls(world: &World) -> Vec<Vec<String>> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "gh")
        .filter_map(|call| {
            Some(
                call["args"]
                    .as_array()?
                    .iter()
                    .filter_map(|arg| arg.as_str().map(str::to_string))
                    .collect(),
            )
        })
        .collect()
}

/// One node's newest settlement.
fn settlement_of(world: &World, run: &str, node: &str) -> Value {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("{node} has not settled"))
}

/// A fast-adoption node whose awaited release **has not happened** goes as far as
/// it can and stops: the change request opens as a draft carrying the reason, the
/// node settles `complete-but-draft`, and nothing merges it.
///
/// The failure this closes is the worst one available, because it is a success:
/// the node used to settle `done` and the host used to land its change, with the
/// temporary git pin it was launched against now permanent in a base branch and
/// no reader able to tell it had ever been temporary.
///
/// The run is the other half. It is **not finished** — its own views say so and
/// say what each node is waiting on — because a run that reported settled here
/// would be a run whose operator has no reason to look again.
#[test]
fn a_fast_node_whose_release_is_not_out_settles_complete_but_draft_and_nothing_merges_it() {
    let world = watching("adoption-draft-held");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories_opening_a_change(&world);
    // **Two** out-of-repository dependencies, in two repositories that release,
    // because one reason has to name one of them and say how many there are.
    let tool_repo = world.extra_repository("tool");
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    let (tool_script, tool_answer) = world.probe_in(&tool_repo, "tool");
    world.releases(&two_that_release(&script, "tool", &tool_script));
    // The version that was out when each dependency's work landed, and the version
    // that is out when the consumer publishes: the same one. Nothing has been
    // released since, which is the whole condition.
    releases_at(&answer, "0.1.0");
    releases_at(&tool_answer, "0.1.0");
    // The host lands whatever it is handed. Scripted so that the counterfactual
    // this journey is about is reachable: without the draft the change merges and
    // the node goes green, which is the failure — a success — this closes.
    world.script("gh.merged", "");

    let mut packager = lifecycle("packager", &[]);
    packager["repo"] = json!("tool");
    let mut waiting = consumer(Some("fast"));
    waiting["deps"] = json!([ENGINE, "packager"]);
    // A node downstream of the draft. Its dependency is complete and its work
    // cannot land, so starting it would build on a change nobody can merge.
    let follower = crate::harness::agent("follower", &["consumer"]);
    let run = start(
        &world,
        "adoption-draft-held",
        vec![engine(), packager, waiting, follower],
    );
    world.until("the three publishing nodes to settle", |world| {
        world.events_of(&run, "node-settled").len() == 3
    });

    // The node is complete and is not done: every step ran and the branch is
    // published, and the one thing left is outside this run.
    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(
        settled["payload"]["status"],
        json!("complete-but-draft"),
        "the node did not settle as a draft: {settled}"
    );
    assert_eq!(settled["payload"]["outcome"], json!("change-draft"));
    assert_eq!(settled["payload"]["landing"], json!("unlanded"));
    let detail = settled["payload"]["detail"]
        .as_str()
        .expect("a draft settlement says why");
    // The whole sentence, character for character, against the branch the engine
    // really published from: a `contains` over the two names it has to carry
    // would pass on a line no person could read, and this is the one line a
    // person reads to learn why the node stopped short of done.
    assert_eq!(
        detail,
        format!(
            "complete, and held as a draft: awaiting the crate release of \
             github.com/owner/engine, pinned to {reference} until it arrives",
            reference = branch_of(&world, &run, ENGINE),
        ),
        "the settlement does not name the dependency and the target it awaits"
    );

    // A node downstream of it does not start. `complete-but-draft` is not `done`,
    // so the frontier never reaches the follower — which is the whole reason the
    // status is not `done`.
    assert!(
        !dispatched(&world, &run, "follower"),
        "a node was started on work that cannot land"
    );

    // The draft was **requested of the host**, and the reason travelled with the
    // publication rather than being written into the change request's body.
    let calls = gh_calls(&world);
    let created: Vec<&Vec<String>> = calls
        .iter()
        .filter(|call| call.first().map(String::as_str) == Some("pr"))
        .filter(|call| call.get(1).map(String::as_str) == Some("create"))
        .collect();
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(
        created[0].iter().any(|arg| arg == "--draft"),
        "the change request was not opened as a draft: {:?}",
        created[0]
    );

    // And nothing merged it. `change-auto` is the policy that asks the host to
    // land the change once its checks pass, so a publication that did not stop
    // here would have.
    assert!(
        !calls
            .iter()
            .any(|call| call.first().map(String::as_str) == Some("pr")
                && call.get(1).map(String::as_str) == Some("merge")),
        "a change held as a draft was handed to the host to merge: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.get(1).map(String::as_str) == Some("ready")),
        "the draft was lifted while the release it waits on is still out: {calls:?}"
    );

    // The sibling's own record of the hold is in the merged store, stamped with
    // the node it belongs to and rewritten in nothing else.
    let drafted = world.events_of(&run, "change-drafted");
    assert_eq!(drafted.len(), 1, "{drafted:?}");
    assert_eq!(drafted[0]["labels"]["node"], json!("consumer"));
    assert_eq!(
        drafted[0]["payload"]["awaiting"],
        json!("github.com/owner/engine")
    );
    assert_eq!(drafted[0]["payload"]["target"], json!("crate"));
    // One reason names one dependency, and says how many this node adopted early:
    // the second is real and unreleased, and a reason that mentioned only the one
    // it names would leave a reader thinking there is one release to wait for.
    let because = drafted[0]["payload"]["because"]
        .as_str()
        .expect("the reason carries the sentence a person reads");
    assert!(
        because.contains("one of 2 release(s)"),
        "the reason does not say how many releases this node is waiting on: {because}"
    );

    // The **run** is not finished, and reads as waiting rather than as stalled.
    let results = world.run(&["results", &run]);
    results.exited(0);
    assert!(
        results.stdout.contains("waiting"),
        "a run holding a draft reported as something other than waiting:\n{}",
        results.stdout
    );
    assert!(
        results.stdout.contains("complete-but-draft")
            && results.stdout.contains("github.com/owner/engine"),
        "`results` does not say which dependency the node waits on:\n{}",
        results.stdout
    );
    let status = world.run(&["status", &run]);
    status.exited(0);
    assert!(
        status.stdout.contains("complete and held as a draft")
            && status.stdout.contains("neither stalled nor finished")
            && status.stdout.contains("github.com/owner/engine"),
        "`status` does not report the run as waiting on a named release:\n{}",
        status.stdout
    );
    assert!(
        !status.stdout.contains("SETTLED"),
        "a run whose node is held as a draft reported as settled:\n{}",
        status.stdout
    );

    // **One of the two arriving is not enough.** The engine releases and the tool
    // does not, and the node stays exactly where it is: a draft lifted on a
    // partial arrival would land a change still pinned to the release that has
    // not happened, which is the whole failure this closes with one dependency
    // and is no different with two.
    releases_at(&answer, "0.2.0");
    world.until("the engine's release to be observed", |world| {
        world
            .events_of(&run, "release-arrived")
            .iter()
            .any(|event| event["payload"]["identity"] == "github.com/owner/engine")
    });
    assert!(
        world.events_of(&run, "release-adopted").is_empty(),
        "the node was told to move off its pins while one of them is still a pin"
    );
    assert_eq!(
        settled_status(&world, &run, "consumer"),
        Some("complete-but-draft".to_owned()),
        "one release of two lifted the draft"
    );
    assert_eq!(
        tasks_of(&world, "consumer").len(),
        1,
        "the node was put back to work on a release that has only half arrived"
    );

    // The plan of record says the node is still in progress. `done` there is what
    // a person planning around this run reads as "nothing left to do about it",
    // and there is: the change cannot land until the release arrives.
    world.until_store("the store to carry the node as unfinished", |world| {
        world
            .store_tasks(&format!("plans:{}", project_id(&run)))
            .iter()
            .any(|task| {
                task["item"]["metadata"]["onepipeline.id"] == "consumer"
                    && task["item"]["status"]["category"] == "in-progress"
            })
    });

    // And the same state is readable through this crate's own API rather than
    // only off a rendered line: a host that pins this engine reads the run store
    // it writes through these, and a state only the command line could see would
    // be one such a host has no way to act on.
    let paths = onepipeline::views::RunPaths::under(&world.runs, &run);
    let view = onepipeline::views::RunView::open(&paths).expect("the run reads back");
    assert_eq!(
        view.state
            .statuses()
            .get("consumer")
            .map(|status| status.as_str()),
        Some("complete-but-draft"),
        "the API does not report the node as held"
    );
    assert!(
        onepipeline::views::results(&view).contains("github.com/owner/engine"),
        "the API's own rendering does not say what the node waits on"
    );

    world.run(&["stop", &run]).exited(0);
}

/// The release arrives, and the draft is lifted by a **new worker on the branch
/// the node already published** — not by a fresh branch beside it, and not by
/// anything a person does.
///
/// This is the other half of settling late, and the half that makes it
/// recoverable rather than merely safe. The node was left complete, so what the
/// arrival buys is one dispatch: the note names the version, the worker moves the
/// pin, and the publication that carries no reason any more is what `onevcs`
/// lifts the draft on. The node then settles `done` and the run finishes.
#[test]
fn a_release_that_arrives_puts_a_worker_back_on_the_same_branch_and_lifts_the_draft() {
    let world = watching("adoption-draft-lifted");
    world.write_graphs();
    let (engine_repo, consumer_repo) = two_repositories_opening_a_change(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");
    // What the host does once the change is no longer a draft: land it. Scripted
    // from the start, so nothing about the merge changes when the draft lifts —
    // what changes is whether a merge is asked for at all.
    world.script("gh.merged", "");

    let run = start(
        &world,
        "adoption-draft-lifted",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the consumer to be held as a draft", |world| {
        settled_status(world, &run, "consumer") == Some("complete-but-draft".to_owned())
    });
    let held_on = branch_of(&world, &run, "consumer");
    assert_eq!(
        tasks_of(&world, "consumer").len(),
        1,
        "the node was dispatched more than once before any release arrived"
    );

    // The worker's second turn writes something the first did not, so the branch
    // it is put back on visibly moves: this is the pin being moved, which is the
    // whole of what the second dispatch is for.
    world.script("consumer.work", "consumer pins the released engine 0.2.0\n");

    // The release happens. Nobody replies, nobody requeues, nothing is edited.
    releases_at(&answer, "0.2.0");
    world.until("the run to finish", |world| {
        settled_status(world, &run, "consumer") == Some("done".to_owned())
    });

    // A second dispatch really ran, and it ran **on the branch the draft is on**.
    let dispatched = tasks_of(&world, "consumer");
    assert_eq!(dispatched.len(), 2, "{dispatched:?}");
    assert!(
        dispatched[1].contains("## Planner context")
            && dispatched[1].contains("github.com/owner/engine — crate 0.2.0")
            && dispatched[1].contains("Move from the git pin to that released version"),
        "the worker that lifts the draft was not told which version arrived:\n{}",
        dispatched[1]
    );
    assert_eq!(
        branch_of(&world, &run, "consumer"),
        held_on,
        "the release cut a second branch beside the change request it was meant to lift"
    );

    // The draft was lifted at the host, once, on the change the first publication
    // opened — and only after the release, never before.
    let calls = gh_calls(&world);
    let readied: Vec<&Vec<String>> = calls
        .iter()
        .filter(|call| call.get(1).map(String::as_str) == Some("ready"))
        .collect();
    assert_eq!(readied.len(), 1, "{readied:?}");
    let opened: Vec<&Vec<String>> = calls
        .iter()
        .filter(|call| call.get(1).map(String::as_str) == Some("create"))
        .collect();
    assert_eq!(
        opened.len(),
        1,
        "the second dispatch opened a second change request: {opened:?}"
    );
    assert!(
        world.events_of(&run, "draft-lifted").len() == 1,
        "the sibling did not record the lift: {:?}",
        world.kinds(&run)
    );

    // And the node is complete and undrafted: its change landed, and what reached
    // the base is the work the *second* worker left on that branch.
    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(settled["payload"]["outcome"], json!("merged"), "{settled}");
    assert_eq!(settled["payload"]["landing"], json!("landed"), "{settled}");
    assert_eq!(
        world.events_of(&run, "node-settled").len(),
        3,
        "the draft settlement and the completion are not two records of one node"
    );
    let _ = consumer_repo;
}

/// The word one node's newest settlement carried, or nothing if it has not
/// settled at all.
///
/// Polled, so it answers rather than panicking on a node this run has not settled
/// yet — which is every node for as long as a journey is waiting for one.
fn settled_status(world: &World, run: &str, node: &str) -> Option<String> {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)?
        .get("payload")?
        .get("status")?
        .as_str()
        .map(str::to_string)
}

/// A fast-adoption node whose dependency **had already released** when it
/// launched settles `done` directly: no draft, nothing held, and a change the
/// host lands.
///
/// The other end of the same decision, and the one that says the draft is a
/// judgement rather than a mode: fast adoption did not become "always hold". Two
/// runs, because "already released when it launched" is a fact about the moment
/// the node starts — the upstream lands its work and settles in a run of its own,
/// the release goes out, and only then is the consumer launched, pinned to that
/// work by a cross-DAG reference.
#[test]
fn a_fast_node_whose_release_was_already_out_settles_done_with_no_draft() {
    let world = watching("adoption-draft-not-needed");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories_opening_a_change(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");
    world.script("gh.merged", "");

    // The upstream lands the engine's work, and the release carrying it goes out.
    let upstream = start(&world, "adoption-already-released", vec![engine()]);
    world.until("the upstream to settle", |world| {
        world.run_file(&upstream, "result.json").is_file()
    });
    releases_at(&answer, "0.2.0");
    let landed = branch_of(&world, &upstream, "engine");
    world.until("the release to be out before anything launches", |world| {
        matches!(
            world.on_onevcs(|| onevcs::release_status(&landed, None)),
            Ok(onevcs::ReleaseStatus::Released { .. })
        )
    });

    // Only now is the consumer launched, against a version that already exists.
    let mut across = consumer(Some("fast"));
    across["deps"] = json!([format!("run:{upstream}#engine")]);
    across["consumes"] = json!({format!("run:{upstream}#engine"): "crate"});
    let run = start(&world, "adoption-draft-not-needed", vec![across]);
    world.until("the consumer to settle", |world| {
        settled_status(world, &run, "consumer").is_some()
    });

    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(
        settled["payload"]["status"],
        json!("done"),
        "a node whose release was already out was held anyway: {settled}"
    );
    assert_eq!(settled["payload"]["outcome"], json!("merged"), "{settled}");
    let calls = gh_calls(&world);
    assert!(
        !calls
            .iter()
            .any(|call| call.iter().any(|arg| arg == "--draft")),
        "the host was asked to hold a change whose release is out: {calls:?}"
    );
    assert!(
        world.events_of(&run, "change-drafted").is_empty(),
        "a draft was recorded for a node that never needed one"
    );
    // And it was dispatched exactly once: there was nothing to come back for.
    assert_eq!(tasks_of(&world, "consumer").len(), 1);
}

/// A **published**-adoption node never enters the draft state: it is held until
/// its dependency's release answers, and then settles `done` like any other node.
///
/// The draft belongs to fast adoption alone, and the reason is what the two modes
/// each buy. A `published` node is not scheduled until the release is out, so it
/// launches against a version rather than a git pin and has no temporary pin for
/// a draft to hold back — drafting it would hold a change nothing was wrong with.
#[test]
fn a_published_node_is_never_held_as_a_draft_and_settles_done_on_its_release() {
    let world = watching("adoption-published-undrafted");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories_opening_a_change(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");
    world.script("gh.merged", "");

    let run = start(
        &world,
        "adoption-published-undrafted",
        vec![engine(), consumer(Some("published"))],
    );
    // Held: the release has not moved, and the node is not dispatched.
    world.until("the wait to carry its own answer", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
    });
    assert!(!dispatched(&world, &run, "consumer"));

    // The release answers, and that is the only thing that starts it.
    releases_at(&answer, "0.2.0");
    world.until("the consumer to settle", |world| {
        settled_status(world, &run, "consumer").is_some()
    });

    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(
        settled["payload"]["status"],
        json!("done"),
        "a published-adoption node reported a state that belongs to fast adoption: {settled}"
    );
    assert_eq!(settled["payload"]["outcome"], json!("merged"), "{settled}");
    // Never, at any point in the run: not in a settlement, not at the host, and
    // not in the sibling's own record of what it published.
    assert!(
        world
            .events_of(&run, "node-settled")
            .iter()
            .all(|event| event["payload"]["status"] != json!("complete-but-draft")),
        "a published-adoption node settled as a draft at some point: {:?}",
        world.events_of(&run, "node-settled")
    );
    assert!(
        world.events_of(&run, "change-drafted").is_empty(),
        "a published-adoption node's change was opened as a draft"
    );
    let calls = gh_calls(&world);
    assert!(
        !calls
            .iter()
            .any(|call| call.iter().any(|arg| arg == "--draft")),
        "the host was asked to draft a published-adoption node's change: {calls:?}"
    );
}

/// A fast node whose awaited target is a **human step** is held as a draft too.
///
/// `released` is the one answer that lets a pin go, and there are three others
/// that reach the publication. This is one: the dependency's work has landed and
/// nobody has recorded the release yet, so the pin is exactly as temporary as it
/// is against an automated target nothing has released — and landing the change
/// would make it permanent in the same way.
#[test]
fn a_fast_node_awaiting_a_human_step_is_held_as_a_draft() {
    let world = watching("adoption-draft-human-step");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories_opening_a_change(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    // The automated target *is* released, so the only thing that can hold this
    // node is the human-step target it consumes.
    releases_at(&answer, "0.1.0");
    world.script("gh.merged", "");

    let mut waiting = consumer(Some("fast"));
    waiting["consumes"] = json!({ENGINE: "wheel"});
    let run = start(&world, "adoption-draft-human-step", vec![engine(), waiting]);
    world.until("the consumer to settle", |world| {
        settled_status(world, &run, "consumer").is_some()
    });

    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(
        settled["payload"]["status"],
        json!("complete-but-draft"),
        "a node awaiting a human step landed its git pin: {settled}"
    );
    let detail = settled["payload"]["detail"]
        .as_str()
        .expect("a draft settlement says why");
    assert!(
        detail.starts_with(
            "complete, and held as a draft: awaiting the wheel release of \
             github.com/owner/engine, pinned to "
        ),
        "the settlement does not name the human-step target it awaits: {detail}"
    );

    let calls = gh_calls(&world);
    assert!(
        calls
            .iter()
            .any(|call| call.iter().any(|arg| arg == "--draft")),
        "the change request was not opened as a draft: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.first().map(String::as_str) == Some("pr")
                && call.get(1).map(String::as_str) == Some("merge")),
        "a change awaiting a human step was handed to the host to merge: {calls:?}"
    );
    world.run(&["stop", &run]).exited(0);
}

/// A fast node whose probe **could not answer** is held as a draft, and never
/// landed on the strength of a question nothing came back from.
///
/// The third answer that reaches the publication, and the one where the safe
/// direction has to be chosen deliberately: an unusable answer says nothing about
/// whether the release happened, so reading it as *released* would land the pin
/// and reading it as anything else holds the change. The draft is the second, and
/// a person is left a change request to finish rather than a permanent pin.
#[test]
fn a_fast_node_whose_probe_could_not_answer_is_held_as_a_draft() {
    let world = watching("adoption-draft-unanswered");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories_opening_a_change(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // A version at the landing, so the baseline the engine's publication captures
    // is one a later answer can be compared against — and so the probe breaking is
    // the only thing that has changed by the time the consumer publishes.
    releases_at(&answer, "0.1.0");
    world.script("gh.merged", "");
    // The consumer's turn is held open, because the question this journey is about
    // is asked at the consumer's *publication*: the probe has to break after the
    // engine has landed and before the consumer settles.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");

    let run = start(
        &world,
        "adoption-draft-unanswered",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the engine to land its work", |world| {
        settled_status(world, &run, ENGINE) == Some("done".to_owned())
    });

    // A probe that prints something that is not a version at all: answered
    // unusably, which is neither released nor not released.
    releases_at(&answer, "whatever-the-nightly-was");
    world.release("consumer.go");
    world.until("the consumer to settle", |world| {
        settled_status(world, &run, "consumer").is_some()
    });

    let settled = settlement_of(&world, &run, "consumer");
    assert_eq!(
        settled["payload"]["status"],
        json!("complete-but-draft"),
        "a probe that could not answer was read as a release, and landed the pin: {settled}"
    );
    let calls = gh_calls(&world);
    assert!(
        calls
            .iter()
            .any(|call| call.iter().any(|arg| arg == "--draft")),
        "the change request was not opened as a draft: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.first().map(String::as_str) == Some("pr")
                && call.get(1).map(String::as_str) == Some("merge")),
        "a change whose release nothing answered for was handed to the host to merge: {calls:?}"
    );

    // And the hold was the hold: an answer the host can read lifts it, which is
    // what says the draft was the unusable answer rather than the probe being
    // broken for good.
    world.script("consumer.work", "consumer pins the released engine 0.2.0\n");
    releases_at(&answer, "0.2.0");
    world.until("the readable answer to finish the node", |world| {
        settled_status(world, &run, "consumer") == Some("done".to_owned())
    });
    world.run(&["stop", &run]).exited(0);
}
