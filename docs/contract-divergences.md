# Where the code and the contract diverge

The contract is committed **verbatim as approved** and is never edited to match
the code. Where it cannot be compiled exactly as written, the
code takes the nearest thing that does exist, and the divergence is recorded
here as a proposal for the planner who owns the contract. Nothing on this list is
resolved unilaterally.

Entries **1–9** have since been **ruled on by the planner who owns the
contract**, and `docs/contract.md` was amended to carry each ruling. They stay
for the record: each states what diverged, what was ruled, and where the amended
contract now says it.

Entries **10 onward are open**. Each states what the code does today and the
proposal it is waiting on — most of them a question for a *producer* rather than
for this crate, because `oneagentgraph` and `onevcs` are independent tools that
expose general integration hooks only and nothing in them may know about this
one. An open entry is recorded here and never resolved from this repository.

## 1. `ResolvedGraphRef` is not a type `oneagentgraph` exports — RESOLVED

**Ruling: accept `ConfigRef`; the contract names it. No new type in
`oneagentgraph`.**

The contract's `DispatchRequest` declared:

```rust
pub graph: ResolvedGraphRef,   // content-addressed node-scope agent-graph config (oneagentgraph type)
```

`oneagentgraph` exports no `ResolvedGraphRef`. The type matching the comment is
[`oneagentgraph::config::ConfigRef`](https://github.com/nickderobertis/oneagentgraph/blob/main/src/config.rs)
— "a filesystem path, or an `https` URL that is fetched, checksummed, and
recorded content-addressed in the run record so replay never depends on the URL
staying stable." `ConfigRef` is what the code uses, in `DispatchRequest::graph`
and in a plan node's `agent_graph`.

The proposal asked whether a `ResolvedGraphRef` — the fetched content plus its
digest, as distinct from the *reference* a config is written as — was intended
as a second type. It was not: the contract's seam sketch now declares
`pub graph: ConfigRef`, and neither repository grows another type for it.

## 2. `SessionSpec` is not a type `onevcs` exports — RESOLVED

**Ruling: accept `SessionRequest`; the caller asks, the executing machine
opens.**

The contract's `WorkspaceSpec` declared:

```rust
pub workspace: WorkspaceSpec,  // Path(PathBuf) | VcsSession(SessionSpec: onevcs type)
```

`onevcs` exports no `SessionSpec`. Its type for *asking* for a session is
[`onevcs::SessionRequest`](https://github.com/nickderobertis/onevcs/blob/main/crates/onevcs/src/session.rs)
— repo, branch, base, execution checkout — which is exactly what
`WorkspaceSpec::VcsSession` carries, since the contract says the machine running
the dispatch is the one that opens the session. (`onevcs::Session` is the *opened*
session, which the dispatching machine never holds.) `SessionRequest` is what the
code uses, and what the amended contract names; the surrounding sentence now says
the request carries the ask and never an opened session.

## 3. `DispatchOutcome` has no specified fields — RESOLVED

**Ruling: accept the four fields and state them; keep the struct
`#[non_exhaustive]`, and keep `tests/contract.rs` gating the prose.**

The contract named `DispatchOutcome` as `DispatchHandle::wait`'s success value
but said nothing about what it carries.

It carries four fields, each one a thing a caller **cannot** recover from the
relayed event stream:

```rust
pub struct DispatchOutcome {
    pub succeeded: bool,
    pub detail: String,
    pub session: Option<String>,
    pub branch: Option<String>,
}
```

`succeeded` and `detail` are the settlement itself — a stream of turns does not
say whether the dispatch ended well, and a node has to settle `done` or
`failed`. `session` and `branch` follow from the contract's own statement that
`WorkspaceSpec::VcsSession` means *the machine running the dispatch opens the
session there*: the caller never opened it, so publication has no token unless
the outcome hands one back.

The contract's seam sketch now declares all four under `#[non_exhaustive]`, so
naming more later stays additive. `tests/contract.rs` reads the struct's
declaration and gates it against both documents, so neither can drift from what
`DispatchOutcome` actually carries.

## 4. The rules grammar spells one predicate but describes two families — RESOLVED

**Ruling: state both predicate families explicitly, and what each matches on.**

The contract called the rules "ordered predicates over capacity **and node
labels**", and its example spelled exactly one:

```yaml
rules:
  - when: {executor_has_capacity: local}
    use: local
```

`when` is a *mapping*, so `Predicate` is compiled as a struct rather than a
one-variant enum — which also makes a second condition a second field, read as
"all of these hold". The label family's key name and matching semantics were the
contract's to settle, and it has settled them: `node_label: {KEY: VALUE, ...}`
matches the node's own reserved labels (`run_id`, `round`, `node`, `persona`) by
exact string equality, never a glob or a pattern; several conditions in one
`when` conjoin; and a `when` naming neither family — or naming `step`, which no
executor choice can see, because the choice is made once per node before any step
runs — is refused at load rather than left as a rule that silently never fires. `Predicate` carries both fields,
and the shipped default rules are unchanged — the capacity family alone is what
a single-executor host needs.

## 5. `min_free_mem: 2GiB` is carried as its string — RESOLVED

**Ruling: accept `String` plus a validated unit list; the contract states the
units. Refusing an unknown unit at load is correct.**

`ExecutorEntry::min_free_mem` is a `String`, holding `2GiB` as the contract's
example writes it, and `rules::bytes_of` reads it as a byte count where the
evaluator needs one.

The field stays a `String` on the type rather than becoming a `u64`, because the
contract fixes the *wire* syntax and a parsed field would make the type unable to
round-trip what a rules file wrote. `bytes_of` accepts exactly the units the
contract now spells — `B`, `KiB`, `MiB`, `GiB`, `TiB`, and a bare byte count.

A unit outside that list is **refused when the rules file loads**, naming the
executor and the list. Read leniently it would mean *no limit at all*, so the one
file written to keep dispatches off an exhausted host would be the file that
removed the bound — and a rules file is external input, which this crate
validates at its boundary rather than at the first dispatch. The contract now
states both the unit list and the refusal, so a rules file naming `2GB` is
refused against a documented vocabulary rather than against this
implementation's reading of the one example.

## 6. `onepipeline`'s own event kinds are not enumerated — RESOLVED

**Ruling: enumerate them, and narrow the type to an enum. Cross-source kinds stay
strings.**

The contract fixed the merged stream as "envelope NDJSON" and named the three
sources it merges, but enumerated no event kinds for this library — unlike the
sibling contracts, which each enumerate their own. `EventKind` was therefore the
wire string for everything this crate wrote as well as everything it relayed.

The contract now lists this library's nineteen kinds, and
[`event::PipelineKind`](../src/event.rs) is the enum of exactly those:
`Journal::emit` takes one, so a kind this crate emits cannot be a typo, and every
reader folds through `PipelineKind::from_wire`. A **relayed** envelope keeps its
producer's kind as `EventKind`'s wire string, because an enum there would reject
a kind a newer sibling already produces.

## 7. `resume` cannot say how much of a preserved branch survives — RESOLVED

**Ruling: accept the proposal. `resume` names the completed steps a preserved
branch carries, and a checkpoint must be a commit reachable on the remote.**

The plan schema is "v7 node shapes unchanged", and `resume` is one of them. The
shape originally compiled here was the two fields a continuation cannot do
without:

```rust
pub struct Resume {
    pub branch: String,
    pub checkpoint: Option<String>,
}
```

`ai-orchestrator`'s own `Resume` carries four more — `completed_steps`, `mode`,
`base_branch`, and `pr_base` — and one of them decides real behaviour. Its
lifecycle splits the preserved branch by *what the merge path refused*
(`orchestrator/lifecycle.py`, `REJECTED_CONTENT_OUTCOMES`): a gate that rejected
the **content** clears `completed_steps`, because republishing the identical tree
would be refused identically, while a publication that failed for any other
reason keeps them and re-runs only what is left.

`Resume` now carries `completed_steps`, and a continuation skips exactly the
steps it names. An absent or empty list re-runs the whole workstream, which is
the safe direction: work is repeated, never skipped or lost. The contract also
now states what a `checkpoint` is — a commit reachable on the remote, because the
machine that continues a node is not the machine that made it. The
content/non-content split that decides *when* `completed_steps` is cleared stays
with the library that knows what its merge path refused, which is `onevcs`; this
crate carries forward what the round it folded actually finished.

## 8. Cross-DAG edges are named by the schema and nowhere else — RESOLVED

**Ruling: adopt `ai-orchestrator`'s semantics and state them, including
blocked-not-failed and reported-not-rerun.**

The contract named the feature once, in the plan-schema sentence: "cross-DAG
`run:<id>#<node>` refs". That fixed the *syntax* and nothing else. It did not
say what resolves an edge, what an unresolvable one does, or what either records
— and an edge that is only syntax is inert, because a reference nothing resolves
blocks its consumer for ever.

The semantics implemented are `ai-orchestrator`'s, which
`docs/orchestration.md` states and which this crate's task named as the source
being preserved. The contract now states all four:

- An edge resolves by reading the referenced run's ledger. Only a `node-settled`
  of `done` satisfies it; an unknown run, a node that has not settled, and one
  that settled `failed` or `skipped` all leave the consumer **blocked**, never
  failed, because the upstream may still arrive.
- Resolution is re-read on every reconcile pass and afresh in every later round,
  so an upstream that arrives after a consumer was blocked starts it in the next
  round rather than parking the run.
- On first resolution the consumer records how far the upstream had got
  (`cross-dag-satisfied`, `{dependency, last_seq}`). If the upstream passes that
  point afterwards the consumer reports it once per consumer
  (`upstream-modified`, `{dependency, captured_last_seq, observed_last_seq}`)
  and **does not re-run**: the work was correct when it was done, and repeating
  it is the planner's judgement.
- `last_seq` is the count of records in the upstream's merged store. A run is
  written by several processes, so no single stream's `seq` describes it.

Both kinds are `onepipeline`'s own, and divergence 6 now enumerates them.

## 9. The views were not the 1:1 port the contract claimed — RESOLVED

**Ruling: state what the views actually report. The buckets are the eight
`ai-orchestrator` carries, and `usage` exists.**

The contract said "Views (CLI, semantics ported 1:1) … WALL buckets that sum
exactly", and they were not ported 1:1:

- `BucketName` had four variants — `dispatching`, `awaiting-planner`,
  `awaiting-human`, `orchestrating` — where the wire and `ai-orchestrator` both
  carry eight, and the reduction was not recorded anywhere.
- `RunTelemetry` carried **no usage field at all**, while the module's own doc
  comment and the CLI's help both promised "session timing and usage".
- `status` said a node had been in flight for thirty-four minutes and nothing
  else, though `oneagentgraph` emits a bounded tool summary from both member
  kinds and this crate already relayed it into the journal unread.
- A lifecycle node's session stream was read once, immediately before `close`,
  so the whole publication — the longest wall-clock segment the node has —
  appeared in one batch when it was already over.
- There was no transcript verb, though the evidence a turn leaves behind is
  retained and named on every `member-settled`.

The contract now states each of these: the eight bucket names, the four usage
parties and their fields, the per-node activity readout, the followed session
stream, and `transcript`. `awaiting-planner` and `awaiting-human` are **not**
among the eight; the time they named lands in `scheduling`, whose definition now
says so. The telemetry document's `schema_version` went from `1` to `2`, because
a consumer filtering on `dispatching` finds no such bucket under `2`.

## 10. The merged stream does not say which side of a turn ran — OPEN

**Proposal (for `oneagentgraph`): stamp the conversation side on a turn, the way
`fallback-advanced` already carries `role: agent | judge`.**

The contract's eight buckets and four usage parties both distinguish the agent
side of a dispatch from the judge side supervising it. The *usage* split is
answerable — the onejudge report a member settles with carries
`telemetry.agent.usage` and `telemetry.judge.usage`, and this crate reads it
from the `report_path` the settlement names. The *timing* split is not: only the
agent side's tool events are streamed, and no `turn-started`, `turn-activity`, or
`turn-completed` says which side it belongs to.

So the `judge` **bucket** is served absent rather than as a zero, and the judge's
wall time is inside `agent` until a producer says otherwise.
`telemetry::of_run` already reads `payload.role` where one is stamped, so a
producer that adds it needs no change here. The `judge` **party** of `usage` is
populated today.

## 11. Nothing in this stack runs an LLM-lint pass — OPEN

**Proposal: leave `llmlint` absent until something produces it, or drop it from
the contract's party and bucket lists.**

The contract names `llmlint` as one of the four usage parties and one of the
eight buckets, because `ai-orchestrator` accounts for one. No member of the
shipped graphs runs an LLM-lint pass and no library in the stack reports one, so
both are served **absent**. That is deliberate and is the rule the contract
states — an unmeasured bucket must not read as a measured zero — but it means
two named slots are permanently empty until a producer fills them.

## 12. `turn-completed` spells its usage two ways — OPEN

**Proposal (for `oneagentgraph`): make the emitted payload and the declared type
agree.**

`oneagentgraph::event::TurnCompleted` declares `usage` as a struct spelling the
numbers `tokens_in`, `tokens_out`, `cache_read`, `cache_write`, `cost`,
`duration`. What the library actually emits is the onejudge report's own usage
object, which spells them `input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_write_tokens`, `cost_usd` — and that is what its own text renderer reads.

This crate reads **both** spellings. Picking one would leave a run whose
producer used the other reporting no usage at all, and on a host whose routine
failure mode is quota exhaustion a silently unaccounted run is worse than a
verbose reader.

For the same reason the retained report is read **structurally**, by field name,
rather than through the producing libraries' own types: it is a sibling's
artifact, and a stricter read would refuse a whole report over one field it did
not recognise and report nothing at all.

The report is also read out of **this run's own copy** rather than from the path
`report_path` names. `oneagentgraph` mints that path under a state root this
crate neither chooses nor can recompute: the constant naming the environment
variable lives in that library's `src/main.rs`, private to its binary, its
library exposes no accessor, an operator moves the root with a variable this
crate does not set, and a future executor stores the report on another machine
entirely. So there is no root to pin it to, and one pinned here would be this
crate re-declaring a sibling's config — refusing legitimate reports the moment
it was wrong, which for evidence is the wrong direction to fail in.

What there *is* instead is a moment when the path carries the producer's
authority: the envelope arriving on the stdout of a process this crate started.
The copy is taken there — refusing a name that is not the producing library's
own `REPORT_FILE`, a symlink, anything that is not a plain file, and anything
past a size bound — and every reader afterwards opens only that copy, at a name
derived from the settlement. A line forged into a journal afterwards reaches
nothing at all.

**Proposal (for `oneagentgraph`): expose the state root its binary resolves.**
A `pub fn state_dir(env) -> PathBuf` on the library, or the `STATE_DIR_ENV`
constant made public, would let this crate confine the ingest to a
producer-owned root as well — a second lock on the same door. It is a general
accessor for a location that library already computes, so nothing in it would
need to know this crate exists.

## 13. The contract names a command where the code makes a library call — OPEN

**Proposal: restate the merged-stream sentence in terms of the operation rather
than the CLI. What it promises is unchanged.**

The contract fixes how a lifecycle node's session reaches the merged stream:

> A lifecycle node's `onevcs` session is **followed** — `onevcs events TOKEN
> --follow` — from the moment there is a token until the session closes …

Every promise around that clause still holds exactly: the session is followed
from the moment there is a token until it closes, each envelope is stamped with
the node it belongs to, an enricher never rewrites a key the producer stamped,
and a follow that never started or that relayed nothing falls back to reading
the stream once. What is no longer true is the **mechanism named in the dash**:
`onevcs 0.2.1` publishes [`EventStream`], a cursor over one session's stream that
hands back typed envelopes attributed to the session that wrote them, so this
crate calls it rather than spawning `onevcs events` and parsing its stdout.

Nothing else in the contract names a `onevcs` command, and the same release made
`publish`, `session close`, and the session record library calls too — so the
four operations this crate performs are now four function calls and no
subprocess. The clause is the last place the composition is described as a
process boundary.

## 14. A second session on one repository destroys the first's workspace — OPEN

**Proposal (for `onevcs`): do not reclaim a run root whose session record is
`Open`.**

`Vcs::open_session` releases the occupancy lease it took before it returns, and
the next `open_session` on the same identity reclaims every run root whose lease
is free and whose clone holds no commit the origin lacks. A session that has been
opened and is *being worked in* — its worker has written files and committed
nothing yet — matches both, so opening a second session deletes the first's clone
and worktree outright. The first publication then fails with `cannot run git: No
such file or directory`, which is git refusing a working directory that is no
longer there.

The session record already says `Lifecycle::Open`, so the fact needed to skip it
is recorded; `reclaim` does not consult it. Nothing here can hold the lease
instead: `open_session` hands back a [`Session`], not a lease, and `adopt_session`
— the one call that claims one — commits whatever the worktree holds behind an
incomplete-step marker, which is not what a caller re-entering its own live
session means.

**What this crate does today.** A lifecycle node opens **one** session, and every
dispatch after the first runs in the worktree it handed back
(`WorkspaceSpec::Path`), so no second session is opened for one node. That is
also the only correct reading of the composition: a second session is a fresh
clone cut from the base, so a later step would see none of the earlier steps'
work and the `pr-author` dispatch — asked to *read this branch's diff* — would
read an empty one. Both were invisible while a scripted `onevcs` double answered
the same worktree for the same branch.

**Still reachable, and not fixable here:** two lifecycle nodes on one repository,
in flight at once, still reclaim each other. Their sessions are opened
independently and neither can hold a lease.

## 15. A publication's typed outcome drops the change request's identity — OPEN

**Proposal (for `onevcs`): carry the change request on `PublishOutcome::Merged`,
and its host id alongside the URL.**

`PublishOutcome` is the value this crate settles a node on, and two facts a caller
needs are not on it:

- **The host's id.** `ChangeOpen` and `Queued` carry a `Url` and nothing else,
  while the `ChangeRequest` the publication actually opened carries
  `id: ChangeId` as well — the handle every later command addresses the change
  by. It is on the session's own `change-opened` event, so this crate reads it
  from there; a caller that only holds the `Publication` cannot.
- **Where a merged change is read.** `Merged(Sha)` names the commit and not the
  change request, so a `change-auto` or `change-direct` publication that *landed*
  answers with no URL at all — the one ending where a person is most likely to
  want the link.

`Settlement::change_url` is therefore populated for `change-open` and `queued`
and empty for `merged`, and the change id is journalled by the sibling rather
than by this crate.

## 16. `lock-wait` is emitted after the wait, so its bucket measures nothing — OPEN

**Proposal (for `onevcs`): bracket the wait — emit `lock-wait` before waiting for
the identity's turn, or keep the marker and make the elapsed the reported
duration.**

The contract's eight-way breakdown separates the time a publication spends
waiting for an identity's merge queue from the time its gate runs and from the
agent's. `onevcs` emits `lock-wait` **after** `queue::turn` has returned, with
the seconds it waited in the payload, and emits `lock-acquired` immediately
after — so the interval between the two markers is the cost of writing two
records however long the wait was, and `telemetry`'s `lock_wait` bucket reads
0ms or 1ms on every real run. The wall time lands in whichever bucket precedes
it.

The `onevcs` double this repository used to carry emitted the marker and *then*
blocked, which is a shape no release of that library has produced; that is what
kept the bucket looking measured.

**Follow-up for this crate, not done here:** charge the bucket from the marker's
own `elapsed` payload rather than from the interval between two markers. It
changes how `telemetry::of_run` folds phases and the sums the checked-in golden
holds, which is a change of its own.
`telemetry_separates_gate_and_lock_time_from_agent_time` holds the gate half to a
real held stretch, asserts the elapsed is on the record, and bounds the bucket
below what it calls a measurable stretch — so it fails, naming the wait, if the
bucket ever spans one again. It does **not** assert an exact number: the two
markers carry real millisecond timestamps, so under coverage instrumentation
they reliably differ by one, and an exact assertion was measuring the host's
clock rather than this crate. The double emitted the marker and then blocked,
which is what made an exact number look like a fact.

## 17. A stream line this build cannot read loses the batch around it — OPEN

**Proposal (for `onevcs`): hand back the envelopes that parsed, and report the
line that did not, rather than refusing the read.**

`EventStream::read` returns `Err` on the first line it cannot parse, and the
cursor behind it has already advanced past every line in that batch — so the
whole envelopes read alongside a bad one are discarded with it and are never
handed back on a later read. One unreadable line therefore hides a whole
publication rather than itself.

Exercised rather than assumed, in
`a_session_stream_that_is_not_whole_is_read_for_what_it_holds`: a stream whose
last record is truncated mid-line hands back **nothing**, including the whole
record before it.

The same test settles the related worry that came in with the typed reader — that
`Reader::lines` might tear a final line written without its newline. It does not
lose one: `onevcs` writes a record as two calls, the line then its terminator, and
a reader that arrives between them reads the whole line, relays it once, and does
not hand it back when the terminator lands. That half is sound.

`tests/e2e/lifecycle.rs` holds the part that is this crate's: a line it cannot
read does not fail the node, and the loss is said out loud rather than folded
into an empty stream.

## 18. No lifecycle journey runs on Windows — OPEN

**Proposal (for `onevcs`): store, or hand git, a path git will clone from on
Windows.**

`onevcs register` records what `std::fs::canonicalize` answers, which on Windows
is the verbatim form `\\?\C:\…`, and `session open` gives that path to `git clone`
as the repository to clone from. Git reads a leading `\\?\` as a UNC URL and
refuses it, so no session opens on Windows for this crate or for anybody else
calling that library.

While `onevcs` was reached as a subprocess only two journeys here drove the real
one, and only those two were `cfg(not(windows))`. Now that every lifecycle
journey drives the real repository side, all of them are — `tests/e2e/lifecycle.rs`
whole, and the lifecycle journeys in `views.rs` and `live_edit.rs`. The Windows
leg of CI runs the rest of the suite and this crate's own units, and
`the_real_onevcs_opens_no_session_on_windows_which_is_why_the_journeys_above_are_not_run_here`
is what fails when the sibling stops doing it — which is the signal to drop every
one of those attributes.

## 19. A graph's `env:` block is written into the calling process — OPEN

**Proposal (for `oneagentgraph`): give a member's launch its own environment
rather than the process's, so two graphs running in one process are isolated.**

`run::run` calls a private `export` before anything launches, and that function
does `std::env::remove_var` / `std::env::set_var` on the **running process**:
`ONEHARNESS_HARNESSES` is cleared, and every pair of the graph's `env:` block is
set. It is deliberate upstream and load-bearing — a two-party member is a thread
there, so the `oneharness run` it spawns can only inherit what the contract
promises it if the pairs are on this process — and it was safe while one graph
run was one process.

It is no longer safe here. This crate dispatches several nodes at once, and each
is now a `run::start` in this process rather than a child, so two concurrent
runs each write the other's members' environment and either can clear
`ONEHARNESS_HARNESSES` out from under the other. `set_var` in a multithreaded
process is also the operation Rust 2024 made `unsafe`; this crate is edition
2021, so it compiles, and the race is real either way.

Observed rather than argued, in
`a_graphs_env_block_is_exported_into_this_process_and_not_into_the_run_alone`:
after a launch of a graph declaring one variable, this process is carrying it.
The shipped graphs declare no `env:` block, so nothing in this repository trips
it today — which is why the test is a characterisation rather than a failure,
and why it is the thing that will say so when upstream confines it.

What would close it: the member launch already builds a `member_env` map beside
the export. Handing that to the spawn — and to the in-process side — instead of
mutating the process would need nothing from this crate.

## 20. The sibling's environment keys have no library spelling — OPEN

**Proposal (for `oneagentgraph`): publish `STATE_DIR_ENV` and
`ONEHARNESS_BIN_ENV` from the library, beside the entry points that need them.**

`run::start`, `run::signal`, and `control::interrupt` all take their environment
as a parameter rather than reading the process's — which is right, and is what
lets a consumer hold two runs on two installs. But the *names* a caller must
resolve out of that environment — `ONEAGENTGRAPH_STATE_DIR` and
`ONEAGENTGRAPH_ONEHARNESS_BIN` — and the fallbacks the CLI applies to them are
private `const`s and private functions in the sibling's **binary**. So every
library caller restates them, as `src/agentgraph.rs` does, and a rename or a
changed fallback lands on the CLI alone while the library callers keep resolving
the old thing without a compile error.

The same shape one step along: `run::run` returns `Result<i32, Error>` and the
CLI maps that `Error` to an exit code in a private `exit_for`. This crate's
`exit_for` is a copy of it, kept so a run settles with the code the process path
carried. A `pub fn exit_for(&Error) -> i32` — or an `Error::exit_code` — would
make it one rule rather than two.

## 21. There is no upstream double for a member that must run a command — OPEN

**Proposal (for `oneagentgraph`): a sentinel in
`oneagentgraph-fake-harness` that runs a command and settles on its exit code.**

`crates/testfakes`'s `fake-oneharness` is the one hand-written double this
repository still needs, and it needs it for exactly one behaviour that no
upstream double has: the dag-scope graph's `orchestrator` member is an agent
whose whole job is to run this crate's own engine verbs, so a double standing in
for that turn has to *run a command*. `oneagentgraph-fake-harness`'s sentinel
table — `fake:complete-now`, `fake:hold`, `fake:park`, `fake:did-work`, and the
`FAKE_HARNESS_*` variables — steers what a turn answers and never what it
executes.

The other two candidate seams were rejected on evidence, not preference:

* **`oneharness run --mock-harness ID`** is unreachable. onejudge 0.3.8 has no
  passthrough for it, and `oneagentgraph`'s own `src/bin/fake_harness.rs`
  records the rest of the reason at length: the `MOCK_*` contract fixes one
  response per process environment, and a member needs several from one binary.
* **`ONEHARNESS_BIN_<ID>`** is the right seam and is what the sentinel above
  would be reached through — but it substitutes the *provider* below a real
  `oneharness`, and this repository has no `oneharness` binary in its dependency
  graph on any platform. Taking it would add a second `oneharness-core` major to
  the tree and a provisioning step to three CI legs, to arrive at a double that
  still could not run the orchestrator's command.

Until one exists, the double stays and is reached at `ONEAGENTGRAPH_ONEHARNESS_BIN`.

## 22. `onevcs-testing`'s providers cannot reach a consumer's subprocess — OPEN

**Proposal (for `onevcs`): a documented way to select a supplied `Hosting` from
outside the process — or an accepted answer that `ONEVCS_GH` is that way.**

`onevcs-testing`'s own doc names this repository's case: drive a real `onevcs`
through a real journey "without a real GitHub — and without the scripted fake
binary that substituting the whole CLI amounts to". `MemoryHost` and `FileHost`
are supplied through `Providers`, which is a **compile-time** injection, and
`src/vcs.rs` calls `Providers::real()` because anything else would put a test
implementation on a path a release binary can reach — which is the invariant
`onevcs-testing` exists as a separate crate to protect.

Every e2e journey here drives the compiled `onepipeline` as a subprocess, which
is this repository's own non-negotiable. The two rules do not meet: there is no
way for a journey to hand `FileHost` to a binary it spawns. `tests/onevcs_seam.rs`
is where those providers *can* be used, and does.

So the host side of a subprocess journey is still stood in for at `onevcs`'s own
`ONEVCS_GH` override — that library's documented seam for the `gh` executable —
by `crates/testfakes`'s `fake-gh`. `tests/smoke/` runs the same publication
against the real `gh` and is what holds it honest.

## 23. The retained driver a detached launch spawns is a verb the contract does not name — OPEN

**Proposal (for the planner who owns the contract): name `start --detach`'s
retained driver in the driver contract, or accept a hidden verb outside the
documented surface.**

The contract's driver contract declares `start ... [--attach|--detach]` and says
the launch "launches the dag-scope agent graph ... via oneagentgraph". It does
not say *how* a detached launch keeps driving after its launcher exits, and the
two answers are not equivalent.

An attached launch composes `oneagentgraph` as a **library**, in this process.
A detached one cannot: a library scheduler thread does not outlive the launcher
that is about to exit, so something has to be retained. Retaining
`oneagentgraph` **by name** is what this replaced, and it made the launcher two
parsers rather than one — it validated with the sibling this crate is compiled
against and ran with whichever the host had installed, so a graph document the
runner accepted was refused by the default attached launch, one flag apart:

```text
unknown field `task`, expected one of `oneharness_config`, `persona`, `schedule`, `deps`
```

So the retained process is now *this executable*, at `onepipeline drive GRAPH
--task ... --dir ... [--label ...] [--set ...]` — the same arguments
`oneagentgraph run` takes, composed by this build's own copy of that library. One
build decides what a graph document may contain, whichever launch mode asked.
`ONEPIPELINE_ONEAGENTGRAPH_BIN` remains the explicit, all-or-nothing override and
is now the only way an installed sibling is composed instead.

The verb is `hide = true`, so it is absent from `--help` and from every list the
contract's surface is checked against; `scripts/smoke-published.sh` reaches it
directly, and `tests/contract.rs` requires that of every hidden verb, because a
published artifact without it cannot launch a detached run at all.

What is open is only whether the contract should *say* this: a launcher that
spawns itself is a fact about the driver contract, and it is currently recorded
here rather than there.
