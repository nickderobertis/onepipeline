// The next boundary for every crate a change to which still selects more than half the suites,
// and the part of it that stays a real shared input once that boundary is drawn. The target-layout
// and engine re-homing reports both render from here, and each refuses a broad crate missing a row.

/** The further split that narrows each broad crate's reach. */
export const FURTHER_SPLITS = {
  "run-views":
    "Separate snapshot assembly, activity/history queries, liveness refresh and upstream/publication refresh; each command runner retains only the refreshes it actually invokes. Full snapshot integrations retain all facets.",
  "edit-reconciler":
    "Separate envelope validation, staged operation application and orphan queue recovery; inline submitters still compose their actual apply/recovery effects.",
  "readjust-state":
    "Separate detection evidence, lineage placement and readjust progress records; consumers retain only the state facets they read.",
  readjustment:
    "Separate host detection from retarget/restack execution and post-edit refresh; only operations invoking each effect retain it.",
  "review-replies":
    "Separate review idempotence keys, decline validation, drafting and host posting; the editor needs validation and actual triggered effects.",
  "review-discovery":
    "Separate review discovery/query from scheduled pass effects and reply triggering.",
  "event-emission":
    "Separate payload encoding, bounded formatting and channel delivery; each emitter retains its actual writer implementation.",
  "channel-admission":
    "Separate ownership handover, queue admission and reply wait policy; preserve takeover recovery ordering.",
  "run-control":
    "Separate lifecycle start/adoption from stop/shutdown and continuation; retain real ownership and process operations for each entrypoint.",
  publication:
    "Separate landing validation, publish execution, restack and retarget operations; preserve atomic publication boundaries.",
  "release-watch":
    "Separate stated-release evidence/acknowledgement from upstream polling and scheduler wakeup.",
  "task-store":
    "Separate graph edits, dispatch facts and publication state commits while retaining transactional consistency.",
  "plan-edits":
    "Separate operation schema and validation from operation-specific application; retain full envelope validation integrations.",
  "agent-session":
    "Separate session identity/lookup from author graph creation and process attachment.",
  "planner-notes": "Separate note envelope validation, delivery and reply receipt handling.",
  "run-state":
    "Separate node status and frontier records from activity, refusal and driver-claim records; status readers and activity views then link only their facet. The derived-status cache stays with the status facet.",
  "plan-validation":
    "Separate the graph value types run state records (NodeStatus, NodeRef, Landing, Settled) from graph and destination validation; run-state would then link the value types only, and validation reach the suites that validate.",
  "plan-criteria":
    "Already the task-document grammar alone; plan-model renders tasks through it, so a grammar change legitimately reaches every plan reader. Splitting it further returns nothing until rendering moves out of plan-model.",
  "plan-compat":
    "Reaches the plan-validation suites because validation gates on schema versions; separating the read-side version gate from the write-side retired-field refusal would leave writers alone linking the refusal.",
  "plan-preservation":
    "Reaches exactly plan-validation's suites, its only consumer; it narrows further only when plan-validation's own reach does (see that row).",
  "render-cost":
    "Separate read-byte accounting, elapsed timers and display formatting; only consumers of the changed counter or formatter depend on it.",
  processes:
    "Separate host identity, process liveness, subprocess launch and handle detachment; replace generic sys imports with those capability crates.",
  "event-vocabulary":
    "Separate scheduler, channel, publication and agent kind registries; assemble complete validation only in the journal registry integration.",
  diagnostics:
    "Move domain-specific refusal constructors/formatters to the owning subsystem; retain only the stable error envelope shared by all commands.",
  "plan-model":
    "Separate node/edge schema, executor configuration, publication criteria and plan-loading IO; field-specific readers use the corresponding schema facet. Common strict schema validation remains shared.",
  "event-payloads":
    "Separate scheduler, edit, publication and channel payload schemas from the generic bus envelope. Full history validators still compose all registries; a registry grammar change legitimately reaches them.",
  "run-record":
    "Separate launch identity, agent configuration and invocation provenance; retain a facade LaunchRecord with identical serialization. Whole-record strict decoding remains a shared grammar input.",
  "run-projection":
    "Separate scheduler frontier, branch/publication, channel debt and activity folds; views request their required facets. Full-state checkpoint compatibility retains a full-fold integration.",
  "planner-channel":
    "Separate envelope schema/allowlist, transport queue, delivery acknowledgements and document-layout validation. Reply transport does not compile scheduler-op validation.",
  "run-files":
    "Separate atomic replacement/durability, append-tail healing, JSON codec and range reads; read-only facets cease importing append/lock implementations. Atomic replacement fixes still run all writers.",
  "branch-template":
    "Separate branch-template parsing from expansion; commands validating plans need parsing, launch composition needs expansion.",
  "yaml-codec":
    "Keep generic strict codec stable; move plan, channel-layout and executor document adapters into their owning crates. A generic strict-decoder fix remains a real shared input.",
  "run-paths":
    "Separate run-id validation/root selection from checkpoint/channel/report/dispatch path builders, colocating each builder with its subsystem. Common root semantics remain shared.",
  "run-labels":
    "Separate label record codec/query from update/atomic commit; display commands depend only on the query and schema.",
  "journal-reader":
    "Separate envelope decoding/version checks, merge ordering, loss accounting and byte-range iteration. Full-history readers still compose the same strict checks.",
  "publication-query":
    "Separate landing evidence reread, branch status and debt classification; status needs landing evidence, debt commands need the corresponding debt facet.",
  "run-result":
    "Separate terminal settlement wire facts from per-node publication details and presentation; full results retains all facts, liveness readers need only settlement.",
  checkpoint:
    "Separate coverage/seal validation, bounded replay and derived-cache publication. Cache-writing readers still test the real cache writer.",
  "run-summary":
    "Separate summary record/query from append-driven Maintainer; host/runs listing need query, writers compose maintenance.",
  "append-stamp":
    "Separate stamp reading/comparison from publication; a changed shared stamp protocol remains an actual input of every stamp consumer.",
  "run-report":
    "Separate terminal result record reading from report atomic publication and report presentation; preserve the existing result wire shape.",
  "cross-run-query":
    "Separate reference parsing, upstream status snapshot IO and resolution-cache tracking; only the views consuming upstream state retain those facets.",
  "member-liveness":
    "Separate heartbeat wire decoding, freshness classification and heartbeat publication; observer views do not import publisher implementation.",
  "event-filter":
    "Separate selector AST, matcher and YAML document adapter; inline selectors need no file parser.",
  "publication-boundary":
    "Separate boundary classification (which edits cross a published boundary) from its enforcement at publish time; edit validation links classification only.",
  "publication-host":
    "Separate host capability probing from host writes (push, change-request open and update); readers link probing only.",
  "agent-execution":
    "Separate dispatch request composition, the oneagentgraph invocation and outcome classification; observers link outcome classification only.",
  "run-locks":
    "Separate the lock-file record from acquisition policy (wait, stale takeover, refusal); views inspecting a holder link the record only.",
  journal:
    "Separate the append writer from the on-disk segment layout; readers reach the layout through journal-reader alone.",
  "branch-stack":
    "Separate stack lineage (parent order) from restack execution; views showing a stack link lineage only.",
  "dispatch-controls":
    "Separate control-record parsing from its enforcement at dispatch; views showing controls link parsing only.",
  "fake-oneagentgraph":
    "Separate graph sessions from member scripting and from each typed sibling protocol, one binary each, so a test starting only sessions links no member scripting.",
  "fake-gh":
    "Separate issue and change-request state, review threads and branch pushes into one fake per surface behind onevcs's ONEVCS_GH seam.",
  "test-fake-member":
    "Separate scheduled-member scripting, observe and supervise helpers and ask-manager replies, one module per member behaviour.",
  "test-fake-codec":
    "None left to make: the per-fake split already reduced it to the stable primitive codec.",
  "test-containment": "None left to make: it is one concept, ending a test's whole process tree.",
  "test-profile":
    "Separate the profile::process wrapper from the profile::fs wrapper and from cost reporting.",
};

/** What stays a real shared input of each broad crate after its further split, and who reads it. */
export const STILL_SHARED = {
  publication: "the landing record every publish, restack and retarget writes",
  "edit-reconciler": "the edit envelope grammar every submitter and the reconciler validate",
  "run-control": "the run-ownership protocol every entrypoint takes",
  "run-projection": "the full-state checkpoint shape the full-fold integration reads",
  "channel-admission": "the ownership handover record both outgoing and incoming owners read",
  "run-result": "the terminal settlement facts results and every liveness reader read",
  "run-views": "the snapshot shape the full snapshot integrations assemble",
  "run-state": "the node status record and derived-status cache every status reader reads",
  "event-emission": "the bounded payload encoding every emitter writes",
  "plan-edits": "the operation envelope schema every edit submitter validates against",
  "plan-model": "the common strict schema validation every plan loader runs",
  "plan-compat": "the schema-version gate every plan reader passes",
  "plan-criteria": "the task-document grammar, which every plan reader renders through plan-model",
  "plan-preservation": "nothing of its own: it reaches only plan-validation's consumers",
  "publication-boundary": "the classification rules edit validation and publication both apply",
  "event-payloads": "the generic bus envelope, and every registry for the full-history validators",
  "review-replies": "the review idempotence key that drafting and posting share",
  "agent-execution": "the oneagentgraph invocation protocol every dispatching command speaks",
  "plan-validation": "graph and destination validation every command validating a plan runs",
  "event-vocabulary": "the complete kind registry the journal registry integration assembles",
  "readjust-state": "the readjust progress record detection and placement both read",
  readjustment: "host detection, which every retarget and restack runs first",
  "review-discovery": "the host review query scheduled passes and reply triggering share",
  "planner-channel": "the envelope schema and allowlist every channel writer and reader checks",
  "publication-host": "the onevcs host contract every publishing command is written against",
  "release-watch": "the stated-release evidence record acknowledgement and polling share",
  "publication-query": "landing evidence reread, which status and the debt commands share",
  "run-files": "atomic replacement, which every writer uses",
  "run-report": "the result wire shape",
  "task-store": "the transaction spanning graph, dispatch and publication commits",
  "agent-session": "session identity, which attachment and graph creation share",
  "member-liveness": "the heartbeat wire format publisher and observers share",
  "planner-notes": "the note envelope delivery and receipt share",
  processes: "process liveness, which every command checking a run's owner reads",
  "run-paths": "run-id validation and common root semantics",
  diagnostics: "the stable error envelope every command prints",
  "run-locks": "the lock-file record every writer and holder inspection reads",
  "run-record": "whole-record strict decoding of LaunchRecord",
  "journal-reader": "envelope decoding and version checks every history reader runs",
  "branch-template": "the branch-template grammar parsing and expansion share",
  "append-stamp": "the stamp protocol every stamp consumer compares",
  checkpoint: "coverage and seal validation every bounded replay runs",
  "cross-run-query": "reference parsing every upstream view runs",
  "event-filter": "the selector AST matcher and document adapter share",
  journal: "the segment layout writer and journal-reader share",
  "render-cost": "the counter record each consumer of a changed counter reads",
  "run-labels": "the label record schema",
  "run-summary": "the summary record",
  "yaml-codec": "the generic strict decoder",
  "branch-stack": "the lineage record restack and retarget read",
  "dispatch-controls": "the control record",
  "fake-oneagentgraph":
    "the oneagentgraph CLI protocol it doubles, read by every test that dispatches an agent",
  "fake-gh": "onevcs's remote-double protocol it serves, read by every publishing test",
  "test-fake-member":
    "the member environment (run id, CLI binary, member name) every scripted member reads",
  "test-fake-codec":
    "the codec itself (script directory, recorded invocations, argv flags, rendezvous, YAML helper), read by every fake and through the harness by every suite",
  "test-containment": "the containment the harness places every test in, so every suite",
  "test-profile": "the process and filesystem wrappers every e2e test runs through, so every suite",
};
