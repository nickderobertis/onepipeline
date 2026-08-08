# onepipeline contract

Composes oneagentgraph + onevcs, owns the task DAG, merges the three event streams.

Plan schema v1 = ai-orchestrator tracked-graph schema v7 node shapes unchanged (`agent` direct, lifecycle with `repo`, `kind: human`, nested `steps` on one branch, `expects_no_diff`, `context`, cross-DAG `run:<id>#<node>` refs, What/Why/Acceptance-criteria task prose, judge-only `done_when`), with: `repo` resolved through onevcs; new optional per-node `executor: NAME`; new optional `agent_graph: REF` overriding the default node-scope graph config.

Driver contract: `onepipeline start plan.json [--attach|--detach] [--round-budget 14400] [--heartbeat-interval 1800]` launches the dag-scope agent graph (shipped default: `orchestrator` member + resettable-cron `check-in` member) via oneagentgraph. The orchestrator member drives engine verbs (`onepipeline round run|next`) guarded by the run ownership lock (single writer); its judge side is `onepipeline channel serve RUN` as a command provider. Attach returns when the run settles; exit 3 = nothing is driving the run. `onepipeline adopt RUN` attaches a fresh driver to an intact ledger. Ownership: runs belong to the launching session; `runs --mine`; `stop` refuses another session's run and `--force` names the owner.

Channel (public contract): `onepipeline next RUN`, `reply RUN [FILE]`, `surface RUN --kind check-in --message TEXT`, `attest RUN REF`, `stop RUN`. Reply envelope: legacy verdicts plus `{"version": 1, "commands": [...]}` with ops `add | drop | reparent | retry | cancel | requeue | attest | complete | context` — required fields and validation semantics exactly as ai-orchestrator's live-edit protocol (docs/orchestration.md#live-graph-edits): applied-or-rejected-with-reason, durable command queue, reply exit 0 = applied, 1 = accepted-not-yet-reconciled, 2 = refused/malformed. Surface consumption triggers `oneagentgraph reset-timer RUN check-in` — the whole pacemaker-reset contract.

Executor seam:

```rust
pub trait Executor {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;      // { vcs_sessions: bool, ... }
    fn capacity(&self) -> CapacityReport;        // { slots_free, load1, mem_free_bytes }
    fn dispatch(&self, req: DispatchRequest) -> Result<Box<dyn DispatchHandle>>;
}
pub struct DispatchRequest {
    pub graph: ResolvedGraphRef,                 // content-addressed node-scope agent-graph config (oneagentgraph type)
    pub task: String,
    pub labels: Labels,                          // reserved: run_id, round, node, step, persona
    pub workspace: WorkspaceSpec,                // Path(PathBuf) | VcsSession(SessionSpec: onevcs type)
    pub cancel: CancellationToken,
}
pub trait DispatchHandle {
    fn events(&mut self) -> EventStream;         // envelope NDJSON relayed from wherever it runs
    fn wait(&mut self) -> Result<DispatchOutcome>;
    fn cancel(&self, mode: CancelMode);          // Cooperative | Kill
}
```

`WorkspaceSpec::VcsSession` means the machine running the dispatch opens the onevcs session there; v1 ships `LocalExecutor` only (supports both variants), the trait + rules grammar are shaped for WS dispatch-server and k8s executors.

Executor rules (YAML, ordered predicates over capacity + node labels):

```yaml
executors:
  - {name: local, type: local, max_load1: 8.0, min_free_mem: 2GiB}
rules:
  - when: {executor_has_capacity: local}
    use: local
  - use: local
```

Views (CLI, semantics ported 1:1): `runs`, `status`, `host`, `monitor`, `results`, `goals`, `telemetry [--breakdown]` — unread-surface accounting, driver liveness (DRIVER DEAD vs PARKED vs UNDRIVEN), provider-health block sourced from `oneagentgraph health`, WALL buckets that sum exactly.

Shipped content: personas `orchestrator`, `check-in`, `pr-author`; the dag-scope agent-graph config; a default node-scope config (worker+judge); example plans. pr-author composition: one post-verification dispatch drafting the ChangeSpec title/body from the diff; drafting failure falls back deterministic and never blocks publication.
