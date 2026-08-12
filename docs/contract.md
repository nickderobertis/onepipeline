# onepipeline contract

Composes oneagentgraph + onevcs, owns the task DAG, merges the three event streams.

Plan schema v1 = ai-orchestrator tracked-graph schema v7 node shapes unchanged (`agent` direct, lifecycle with `repo`, `kind: human`, nested `steps` on one branch, `expects_no_diff`, `context`, cross-DAG `run:<id>#<node>` refs, What/Why/Acceptance-criteria task prose, judge-only `done_when`), with: `repo` resolved through onevcs; new optional per-node `executor: NAME`; new optional `agent_graph: REF` overriding the default node-scope graph config.

`resume` continues a node on the branch its previous attempt preserved: `{branch, checkpoint?, completed_steps?}`. `branch` is the preserved branch. `completed_steps` names the steps that branch already carries, and a continuation skips exactly those and re-runs the rest — an absent or empty list re-runs the whole workstream, which repeats work but never skips it. `checkpoint` must be a commit reachable on the remote; a local-only revision is not a checkpoint, because the machine that continues the node is not the machine that made it.

Cross-DAG edges: a `run:<id>#<node>` dep resolves by reading the referenced run's ledger, and only a `node-settled` of `done` satisfies it. An unknown run, a node that has not settled, and one that settled `failed` or `skipped` all leave the consumer **blocked, never failed** — the upstream may still arrive. Resolution is re-read on every reconcile pass and afresh in every later round, so an upstream that arrives after its consumer was blocked starts that consumer in the next round rather than parking the run. On first resolution the consumer records how far the upstream had got (`cross-dag-satisfied`, `{dependency, last_seq}`); if the upstream passes that point afterwards the consumer reports it once (`upstream-modified`, `{dependency, captured_last_seq, observed_last_seq}`) and is **reported, not re-run** — the work was correct when it was done, and repeating it is the planner's judgement. `last_seq` is the count of records in the upstream's merged store, because a run is written by several processes and no single stream's `seq` describes it.

Driver contract: `onepipeline start plan.json [--attach|--detach] [--round-budget 14400] [--heartbeat-interval 1800] [--set PATH=VALUE]... [--node-set PATH=VALUE]... [--acknowledge-concurrent]` launches the dag-scope agent graph (shipped default: `orchestrator` member + resettable-cron `check-in` member) via oneagentgraph. Each repeatable `--set` is forwarded opaquely and in order to that dag-scope launch; each repeatable `--node-set` is forwarded opaquely and in order to every node-scope launch. Both lists are retained in the launch record and replayed by `adopt`. The orchestrator member drives engine verbs (`onepipeline round run|next`) guarded by the run ownership lock (single writer); its judge side is `onepipeline channel serve RUN` as a command provider. Attach returns when the run settles; exit 3 = nothing is driving the run. `onepipeline adopt RUN` attaches a fresh driver to an intact ledger. Ownership: runs belong to the launching session; `runs --mine`; `stop` refuses another session's run and `--force` names the owner.

Before launch, every targeted repository is checked through `onevcs session holders REPO --json`. A live holder refuses the launch unless `--acknowledge-concurrent` is passed; that override remains visible on stderr and emits `concurrent-acknowledged` with the shared identities and runs. A stale holder is reported and does not refuse.

Channel (public contract): `onepipeline next RUN`, `reply RUN [FILE]`, `surface RUN --kind check-in --message TEXT`, `attest RUN REF`, `stop RUN`. Reply envelope: legacy verdicts plus `{"version": 1, "commands": [...]}` with ops `add | drop | reparent | retry | cancel | requeue | attest | complete | context` — required fields and validation semantics exactly as ai-orchestrator's live-edit protocol (docs/orchestration.md#live-graph-edits): applied-or-rejected-with-reason, durable command queue, reply exit 0 = applied, 1 = accepted-not-yet-reconciled, 2 = refused/malformed. `context` carries one further optional field, `deliver: auto|live|next`, defaulting to `auto`: `auto` delivers the note into the node's running turn when it has a controllable one and otherwise attaches it to the next dispatch, `live` refuses with a reason when it cannot deliver into the running turn, and `next` is the next-dispatch behaviour explicitly. Anything else is refused. Live delivery is `oneagentgraph interrupt RUN MEMBER --input`, addressed by the graph run and member the dispatch's own relayed envelopes stamp; that verb's exit 3 — no controllable turn in flight — is the `auto` fall-through and the `live` refusal and is not an error, while a delivery that was attempted and failed is refused under both. A note the running turn took is not also owed to the next dispatch, and `edit-committed` records which happened as `delivery: live | deferred` on the `context-added` operation it compiled. Surface consumption triggers `oneagentgraph reset-timer RUN check-in` — the whole pacemaker-reset contract.

Merged stream: envelope NDJSON, one store per run, interleaving the three sources `pipeline`, `agentgraph`, `vcs`. A relayed envelope keeps its producer's own `stream`, `seq`, `source`, and kind, so a sibling's kind is a wire string this library never rejects. A lifecycle node's `onevcs` session is **followed** — `onevcs events TOKEN --follow` — from the moment there is a token until the session closes, so the gate run, the push, the change request, the check polling, and the merge reach the store while they happen rather than in one batch at settlement; each session envelope is stamped with the node it belongs to, which its producer cannot know, and an enricher never rewrites a key the producer stamped. A follow that never started, or that neither ended cleanly nor relayed a record, falls back to reading the stream once. This library's **own** kinds are a closed set — the `PipelineKind` enum, which is what emits them — and exactly these: `run-started`, `round-started`, `round-finished`, `node-dispatched`, `node-settled`, `edit-committed`, `edit-rejected`, `planner-surface-queued`, `planner-surfaced`, `planner-replied`, `human-attested`, `driver-adopted`, `run-stopped`, `quiet-worker`, `round-budget-exceeded`, `boundary-retried`, `cross-dag-satisfied`, `upstream-modified`, `completion-requested`, `concurrent-acknowledged`.

Executor seam:

```rust
pub trait Executor {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;      // { vcs_sessions: bool, ... }
    fn capacity(&self) -> CapacityReport;        // { slots_free, load1, mem_free_bytes }
    fn dispatch(&self, req: DispatchRequest) -> Result<Box<dyn DispatchHandle>>;
}
pub struct DispatchRequest {
    pub graph: ConfigRef,                        // content-addressed node-scope agent-graph config (oneagentgraph type)
    pub task: String,
    pub labels: Labels,                          // reserved: run_id, round, node, step, persona
    pub workspace: WorkspaceSpec,                // Path(PathBuf) | VcsSession(SessionRequest: onevcs type)
    pub cancel: CancellationToken,
}
pub trait DispatchHandle {
    fn events(&mut self) -> EventStream;         // envelope NDJSON relayed from wherever it runs
    fn wait(&mut self) -> Result<DispatchOutcome>;
    fn cancel(&self, mode: CancelMode);          // Cooperative | Kill
}
#[non_exhaustive]
pub struct DispatchOutcome {
    pub succeeded: bool,                         // the settlement itself: a stream of turns does not carry it
    pub detail: String,
    pub session: Option<String>,                 // the executing machine opened it, so it hands the token back
    pub branch: Option<String>,
}
```

`WorkspaceSpec::VcsSession` means the machine running the dispatch opens the onevcs session there — so the request carries `onevcs::SessionRequest`, the *ask*, and never an opened `onevcs::Session`; v1 ships `LocalExecutor` only (supports both variants), the trait + rules grammar are shaped for WS dispatch-server and k8s executors. `DispatchOutcome` is `#[non_exhaustive]`: naming a further field later is additive.

Executor rules (YAML, ordered predicates over capacity + node labels):

```yaml
executors:
  - {name: local, type: local, max_load1: 8.0, min_free_mem: 2GiB}
rules:
  - when: {executor_has_capacity: local}
    use: local
  - use: local
```

`min_free_mem` is carried as the string a rules file wrote it as, so the file round-trips; the units are exactly `B`, `KiB`, `MiB`, `GiB`, `TiB`, and a bare byte count. Any other unit is refused when the rules file loads, naming the executor and the list — read leniently an unreadable limit means *no limit at all*, so the one file written to keep dispatches off an exhausted host would be the file that removed the bound.

`when` is a mapping, and there are exactly two predicate families in it. `executor_has_capacity: NAME` matches on **capacity**: the named executor's `CapacityReport` against the limits its `executors` entry declares. `node_label: {KEY: VALUE, ...}` matches on the **node's labels** by exact string equality, never a glob or a pattern. The keys it may name are the reserved ones that exist when the choice is made — `run_id`, `round`, `node`, `persona` — because an executor is chosen once per node, before any of its steps run; `step` and a free-form extra are refused at load rather than left as a condition nothing can satisfy. Several conditions in one `when` conjoin: all of them hold or the rule does not fire. A `when` naming neither family is refused at load rather than read as an always-true rule. A rule with no `when` at all is the fallback, and the first rule that holds decides.

Views (CLI): `runs`, `status`, `host`, `monitor`, `results`, `goals`, `transcript RUN [NODE]`, `telemetry [--breakdown]` — unread-surface accounting, driver liveness (DRIVER DEAD vs PARKED vs UNDRIVEN), provider-health block sourced from `oneagentgraph health`. `status` reports, per in-flight node, **what its dispatch is doing now**, how many events it has recorded, and how long since the last one, read from the `turn-activity` summaries `oneagentgraph` emits from both member kinds. `transcript` renders a dispatched turn's tools from those same summaries and its words from the onejudge report a `member-settled` retained at `report_path`. That report is **copied into the run's own storage as the settlement is ingested** — from a process this library started, refusing anything that is not the producing library's own plain file, of its own name, within a bounded size — and every reader afterwards opens only that copy, at a path derived from the settlement rather than taken from it. A settlement whose copy the run does not hold is named as unretained, and the path it claimed is printed and never opened. `telemetry` carries per-party `usage` — `agent`, `judge`, `llmlint`, `total`, each with `input`, `output`, `cache_read`, `cache_write`, and `cost_usd` — and eight WALL buckets that sum exactly: `agent`, `judge`, `llmlint`, `gate`, `publication_wait`, `lock_wait`, `setup`, `scheduling`. Where two nodes are doing different things across one millisecond it is named by the more specific of the two, which is what keeps gate time and lock waiting separable from agent time. A bucket or a party nothing in the stack measures is served **absent**, never as a zero that reads as measured.

Shipped content: personas `orchestrator`, `check-in`, `pr-author`; the dag-scope agent-graph config; a default node-scope config (worker+judge); example plans. pr-author composition: one post-verification dispatch drafting the ChangeSpec title/body from the diff; drafting failure falls back deterministic and never blocks publication.
