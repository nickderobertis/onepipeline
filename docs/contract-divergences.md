# Where the code and the contract diverge

The contract is committed **verbatim as approved** and is never edited to match
the code. Where it cannot be compiled exactly as written, the
code takes the nearest thing that does exist, and the divergence is recorded
here as a proposal for the planner who owns the contract. Nothing on this list is
resolved unilaterally.

Entries **1–9, 23–32 and 34** have since been **ruled on by the planner who owns
the contract**, and `docs/contract.md` was amended to carry each ruling. They stay
for the record: each states what diverged, what was ruled, and where the amended
contract now says it.

Entries **10–22, 33 and 35–40 are open**. Each states what the code does today and
the proposal it is waiting on. Most are questions for a *producer* rather than for
this crate, because `oneagentgraph` and `onevcs` are independent tools that expose
general integration hooks only and nothing in them may know about this one; the
rest — 36 to 40 — are for the planner who owns the contract, and name the sentence
in it they would change. Entry 40 is for both: its plan-schema and event-kind
halves are the contract owner's, and the two things it could not compile are
`onevcs`'s. An open entry is recorded here and never resolved from this
repository.

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
waiting for an identity's merge queue from the rest of that publication and from
the agent's. `onevcs` emits `lock-wait` **after** `queue::turn` has returned,
with the seconds it waited in the payload, and emits `lock-acquired` immediately
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
`telemetry_separates_publication_and_lock_time_from_agent_time` holds the other
half to a real held stretch — the publishing push, held at the repository's own
`pre-push` hook — asserts the elapsed is on the record, and bounds the bucket
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

**It widened at `oneagentgraph 0.2.18`, and this crate now writes into its own
process too.** A single-sided member's turn became an `oneharness_core` library
call there, so the harness oneharness spawns inherits *the hosting process's*
environment rather than one composed per member — and the map a caller hands
`run::start` reaches only the `${VAR}` expansion of the graph's own `env:` block.
The pairs `agentgraph::Launch::env` promises to export therefore reached nothing
on the library backend while the subprocess backend still set them on its child:
one launch, two answers, silently. `agentgraph::export` closes that split by
setting them on this process, which is the same process-wide write this entry is
about — so the proposal above is unchanged and would close this half with it.
What makes it tolerable meanwhile is which pairs they are: a launch carries the
run's own id and where its ledger lives, both constant for the life of a driver,
and one driver drives one run.

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

`crates/testfakes`'s `fake-claude` is the one hand-written double this
repository still needs, and it needs it for exactly one behaviour that no
upstream double has: the dag-scope graph's `orchestrator` member is an agent
whose whole job is to run this crate's own engine verbs, so a double standing in
for that turn has to *run a command*. `oneagentgraph-fake-harness`'s sentinel
table — `fake:complete-now`, `fake:hold`, `fake:park`, `fake:did-work`, and the
`FAKE_HARNESS_*` variables — steers what a turn answers and never what it
executes.

Of the other two candidate seams, one is still unreachable and the other stopped
being a cost:

* **`oneharness run --mock-harness ID`** is unreachable. onejudge 0.3.8 has no
  passthrough for it, and `oneagentgraph`'s own `src/bin/fake_harness.rs`
  records the rest of the reason at length: the `MOCK_*` contract fixes one
  response per process environment, and a member needs several from one binary.
* **`ONEHARNESS_BIN_<ID>` is now what the double is reached at.** It was
  rejected here while it meant substituting the provider below a real
  `oneharness` binary this repository had none of. `oneagentgraph 0.2.18` runs a
  single-sided member's turn through `oneharness_core` as a **library**, so there
  is no `oneharness` binary in the picture on any platform, that crate is already
  in this repository's dependency graph through the sibling, and the process
  oneharness spawns is the harness the member's chain selected — which is what
  `fake-claude` is. No second `oneharness-core` major, and no provisioning step
  on any CI leg. What the move did not give is a double that needs no writing:
  the seam names an executable, and what stands at it is still this
  repository's own.

Until an upstream one exists, the double stays and is reached at
`ONEHARNESS_BIN_CLAUDE_CODE`.

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

## 23. The retained driver a detached launch spawns is a verb the contract does not name — RESOLVED

**Ruling: name both retained-driver verbs in the driver contract as the
documented mechanism a detached launch keeps driving with. They stay
`hide = true` in the CLI, and the smoke coverage requirement stands.**

Ruled together with entry 25, which raised the same question about the second
verb. The driver contract now names `drive GRAPH` and `drive-run RUN`, says what
each retains and why, and states that both are hidden, absent from the surface
list, and reached by `scripts/smoke-published.sh` — which `tests/contract.rs`
requires of every hidden verb, because a published artifact that cannot reach
them cannot launch a detached run at all.

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

What was open was only whether the contract should *say* this: a launcher that
spawns itself is a fact about the driver contract, and it was recorded here
rather than there. It is now recorded in both.

## 24. A node's judge controls never left this crate — RESOLVED

**Ruling: forward `max_turns` as a node-scope override, delete `done_when` from
the plan schema, and refuse any control this build cannot apply. No `done_when`
field in `oneagentgraph`, and no config-merge layer here.**

Schema v7's node shapes included a judge-only `done_when`, and the contract
carried it. Nothing transmitted it. A dispatch's entire per-node override set was
one line — the persona — so `done_when` and `max_turns` were parsed from the
plan, copied into a step, and dropped. Nothing in the launch record, the journal,
or any view said so.

The cost was not the wrong default but the silence. One node was failed twice —
after three hours and after fifteen minutes — against the base config's default
criterion, with its `max_turns: 45` collapsed to the base default of 12, while
the repository's own gate was green both times. Its retry carried a corrected
`done_when` and a corrected budget and failed on the identical sentence, because
neither control was ever transmitted. The work was complete both times and had to
be recovered by hand.

`max_turns` is now `members.worker.max_turns=N` beside the persona, applied after
the run-wide `--node-set`s because a control written against one node is the more
specific of the two. `OnejudgeMember::max_turns` is the sibling's existing
mechanism, so nothing new was needed there.

`done_when` is **gone from `Node` and `Step`** rather than forwarded. onejudge
hands that field to the judge verbatim as its criterion, and the judge is given
the transcript whose first message is the task with its acceptance criteria — so
the per-node bar is already written, in the task, and a second place to write it
is a second place for it to disagree with itself. A bar broader than one node
belongs in the onejudge base config the node-scope graph's worker points at,
written once; onejudge's own default when none is supplied is already "the
original task is complete". Both alternatives — a `done_when` on
`OnejudgeMember`, and a config-merge layer in this crate — were considered and
rejected.

`#[serde(deny_unknown_fields)]` would have refused a plan still carrying the
field with a bare `unknown field` message. Every plan written before this change
carries one, so it is refused **by name** instead, with where the bar goes:

```text
invalid: plan.json: 'contract': `done_when` is no longer a plan field. A node's
review bar is the `## Acceptance criteria` section of its own task, which the
judge is handed verbatim; a bar broader than one node belongs in the onejudge
base config the node-scope graph's worker already points at, under
`user.done_when`
```

The same refusal answers a reply envelope's `add` and a `requeue`'s amendment,
which is where a planner writes a corrected bar.

What keeps the next control honest is structural rather than remembered.
`DispatchRequest` carries a `controls: NodeControls`, and `src/controls.rs` is
the one place a control becomes an override: it destructures `NodeControls` with
no `..` rest pattern, so a field added there does not compile until it is given
an override — and one whose honest answer is "nothing this build can apply" is
written `set: None` and refuses the plan at validation and the launch at
composition. Silently defaulting is no longer reachable.

The dispatch's own view of a control is valid by construction. `NodeControls`
holds `max_turns` as a `NonZeroU32`, so a budget of zero — which no dispatch can
run under, and which `oneagentgraph` refuses when it validates the graph — cannot
be built, carried, or launched with. The plan schema keeps `Option<u32>`, because
that is the shape a v7 plan file is written in and a live edit merges submitted
JSON into; `NodeControls::of_node` and `of_step` are the checked conversion
between the two, and they run at the trust boundary every plan and every edit
crosses. A graph that reached a dispatch without crossing it — one folded from a
journal a stale build wrote — settles the node `invalid-node` with the same
sentence rather than launching.

The plan schema was **version 2** for it, and the amended contract listed
`max_turns` among the node shapes and no `done_when` at all, stated what a
control is and what becomes of one that cannot be applied, and named
`pub controls: NodeControls` in its seam sketch.

**Superseded on the version, not on the controls.** That change refused a
version-1 plan deliberately; the schema is at 3 now and this build reads 1, 2,
and 3, because what each version added is keyed to the version the *document*
declares and a plan already written on a host is a document this engine can
execute. What survives unchanged is the field: a plan carrying `done_when` is
answered with that field's own refusal at every version, because the review bar
its author wrote is the thing they have to move.

## 25. The retained driver of a detached launch is now a *second* hidden verb — RESOLVED

**Ruling: as entry 23 — name both retained-driver verbs in the driver contract.
Both stay `hide = true`, and both stay under the smoke coverage requirement.**

Entry 23 records `onepipeline drive`, the hidden verb a detached launch retains
to compose *this build's* `oneagentgraph`. Roundless execution adds a second one
for the same structural reason and at a different layer. The engine is no longer
an agent running `round run|next`: `start` runs the reconcile loop itself, and an
attached launch runs it in-process. A **detached** launch cannot — the loop is a
thread, and a thread does not outlive the launcher that is about to return — so
it retains this executable at `onepipeline drive-run RUN`, which takes the
ownership lock, launches the observer graph if the run has one, and runs the same
loop the attached launch would have run in this process.

Both verbs are `hide = true` and both are reached directly by
`scripts/smoke-published.sh`, which `tests/contract.rs` requires of every hidden
verb. `drive-run` is also what makes `stop` whole: the observer is launched *by*
the retained driver, so it is inside the process tree a stop reaps rather than a
sibling process reparented to init.

The driver contract now names both, and says of each what it retains and why.

## 26. `attach` returns when the loop concludes, not the moment a surface blocks — RESOLVED

**Ruling: confirmed. `awaiting-planner` means an outstanding decision **and**
nothing else able to move; an attached launch waits out every decision it can
make progress beside and returns only when the run cannot advance without
something arriving over the channel.**

The contract says attach "returns when the run settles ... a blocking surface
waits". Under rounds that was unambiguous: the driver was a separate process, so
the attaching launcher could return the moment a blocking surface appeared and
leave the run advancing behind it.

It cannot now, and the two halves of the delta pull against each other. A
decision point "pauses only its dependent subtree; independent branches proceed",
so returning the moment one appears would abandon branches that are still
running — in-process, returning ends the loop. And clearing a decision
"auto-resumes the paused subtree within the running loop", which requires the
loop to still be running when the clear arrives.

So the loop waits out every decision it can still make progress beside, and
returns only when nothing can move without something arriving over the channel.
`settlement_of` then reads `awaiting-planner` off that state: an outstanding
`kind: human` action, or an unanswered blocking surface. A decision cleared while
other work is in flight resumes inside the loop, exactly as the delta says; one
cleared after the launch returned is picked up by `adopt`. That is what the
ruling confirmed, and the driver contract now states the conjunction outright.

## 27. `adopt` now ends the parked driver it is taking the run over from — RESOLVED

**Ruling: confirmed. `adopt` may politely end a driver the liveness verdict has
already called PARKED or otherwise undriven, wait for it to go, and then take the
lock. The stderr disclosure stands, and there is still no `--force`.**

The contract makes `PARKED` — a live pid that has written nothing for a whole
interval — an *undriven* verdict, and `adopt` the way back from it. Under rounds
that cost nothing: the ownership lock was taken and released per engine verb, so
a parked driver was not holding it.

The loop now holds that lock for as long as it drives, and a reclaim only happens
for a holder this host can prove is gone. An adoption that started its loop
beside a parked driver would lose the race and refuse — closing the one documented
way back from `PARKED`. So `adopt`, having already refused a run that is genuinely
being driven, ends the parked driver politely, waits for it to go, and then takes
the lock. It says so on stderr, and it still has no `--force`: what it may end is
only a driver the liveness verdict has already called undriven. The driver
contract now says all of that.

## 28. A retried dispatch is journalled as a dispatch, not as its own kind — RESOLVED

**Ruling: confirmed. `node-dispatched` carrying an `attempt` is the record for a
re-asked dispatch, and only a dispatch that produced nothing and failed is
re-asked. The contract's event section states the `attempt` payload.**

The approved event delta removes `boundary-retried`, whose name was the round
boundary's. The behaviour it reported is not round-shaped and is retained: a
dispatch that produced *nothing* and failed is asked again, because that failure
carries no work to lose, and only that one — an attempt that answered has already
answered.

With the kind gone the retry had nowhere to be recorded, and an unreported retry
is a run whose evidence says one dispatch where three happened. So each attempt
emits `node-dispatched`, which is what it is, carrying `attempt`, `attempts`, and
the bounded reason the last attempt gave. A reader counts dispatches per node to
see a retry; the settlement still distinguishes `no-agent-progress` from
`task-failed`.

The alternative — silence — was rejected as removing evidence the delta did not
ask to remove.

## 29. Continuing preserved work moved from the round transition into `retry` — RESOLVED

**Ruling: confirmed, both halves. A `retry` naming neither `branch` nor `resume`
inherits both from the node it supersedes — an explicitly named one wins — and
`retry` removes the superseded node in the same edit, emitting `node-dropped`.
The run's record keeps that node's settlement and the edit that replaced it.**

Two behaviours lived in the round transition, and rounds are gone.

**Continuing a preserved branch.** A node that ran, committed, and stopped —
failed, cancelled, parked — leaves work on a branch. The transition pinned the
carried node to it, so the next round's attempt continued that branch rather than
cutting a fresh one beside committed work nothing points at. Roundless, nothing
carries a node forward on its own: what re-runs the work is the planner's
`retry`. So the pin is folded onto the node when its settlement records the
branch (`projection::pin_preserved_branch`), and a replacement that names neither
`branch` nor `resume` inherits both from the node it supersedes. A planner who
named either is answered with what they named, which is the agreement
`validate_retry_pin` already holds.

**Removing the superseded node.** The contract's own words were that the
superseded node "stays in the executed graph, cancelled, so the round's own
record still names it, and the transition then removes it exactly as a `drop`
would". There is no transition to do the removing, and a cancelled node left in
the graph holds the whole run in `waiting` forever — a graph that can never
settle because something was retried. So `retry` now removes it in the same
edit, emitting the `node-dropped` operation the transition would have. What
became of it is still in the run's record: its own `node-settled`, and the
`edit-committed` that replaced it.

## 30. There was no launch config for the `filters:` block to live in — RESOLVED

**Ruling: ship the launch config. `start --launch-config FILE` reads a versioned
document whose `filters:` block is the approved one, and the flags stay, as
overrides of the part of it they name.**

The approved contract names a launch-level `filters:` block "in the launch config
and equivalent `start` flags". This crate had no launch *config*: `start` took a
plan and flags, and the only launch-level document was `launch.json`, which the
launcher **writes** and `adopt` replays — nothing an operator hands to `start`.
The block reached a run as three flags only.

Both halves now exist and they are one block.
[`filter::LaunchConfig`](../src/filter.rs) is the document — a `schema_version`,
an optional `filters:`, and what a later version added beside it, read as YAML or
JSON by `LaunchConfig::load` — and `cli::StartArgs::launch_config` is the
`--launch-config FILE` that names it. `driver::declared_filters` reads that
config as the **base** and applies each flag over it: `--filter-agentgraph` and
`--filter-vcs` replace their source filter wholesale, and each
`--filter-profile NAME=SPEC` replaces one profile *by name*, leaving the rest of
a config's profiles standing. A launch naming no config is the same code path
with an empty base, which is what keeps the two surfaces one block rather than
two that have to agree.

The config is external input and is refused at its own boundary — unknown key by
name, `schema_version` by its number, and a filter by the grammar's own rules —
before a run is minted. It is versioned and pinned: `LAUNCH_CONFIG_SCHEMA_VERSION`
beside a checked-in golden **per version**, gated the way the run result and the
telemetry document already are, so the shape cannot move without someone deciding
to move it.

**Version 2 is `pr_author_graph`**, the launch's second decision, and the bump was
made deliberately with the golden that pins it —
`tests/golden/launch-config-v2.json`, beside the version-1 one that stays checked
in for what a single golden cannot pin: that a config written before the key is
still a document this build reads. `LAUNCH_CONFIG_SCHEMA_VERSIONS_READ` is that
set, and naming the key at version 1 is refused by the key's own name rather than
by the number — the same rule the plan schema reads `body` by.

## 31. `next` had no event view for a profile to shape, so it grew one — RESOLVED

**Ruling: confirmed. `next` answers `{status, surface, events}`, with `events`
the whole merged store shaped by the profile — the same view `monitor` renders —
and no cursor. A profile is a view over events; the channel is never filtered.**

The approved contract says a read-time profile "shapes the event view" of both
`next` and `monitor`, and adds that a profile "must not change which surfaces
exist or the unread-surface accounting" and that "blocking surfaces are always
delivered". `monitor` renders the merged store, so it has an event view. `next`
did not: it answered `{status, surface}` and nothing else, so `--filter` on it
would have had nothing to act on unless it filtered the *channel* — which is the
one thing the same sentence forbids.

So `next` now answers `{status, surface, events}`, where `events` is the run's
merged store shaped by the profile, exactly as `monitor` shapes what it renders.
The three clauses above then become one guarantee, held by `tests/e2e/filter.rs`:
a profile is a view over **events**, and the channel — which surfaces exist, which
one `next` claims, and the unread accounting over them — is never filtered.

The corner this leaves is cost rather than correctness: the view is the whole
store each time, as `monitor`'s already is, so a planner polling `next` re-reads
the pipeline spine on every call. A cursor — "since the `planner-surfaced` this
reader last recorded" — is derivable from the store with no new state, and was
put to the planner as the alternative. It was **not** taken: the approved text
says a profile shapes a view and says nothing about a cursor, and a `next` that
answered with a window while `monitor` answered with the whole store would make
the two verbs disagree about what "the event view" means.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] this entry
*describes* the duplication rather than introducing it: `docs/contract.md` fixes the
filter grammar — like the envelope beside it — as duplicated per repository by design,
with the committed grammar text as the one source and each producer's own contract test as
the drift gate. `tests/contract.rs` is this repository's, and it fails `just check` the
moment `src/filter.rs` stops matching the document. Building a cross-repository generation
step or drift gate instead is a change to three independently-released tools and to the
approved contract, which is a proposal to the planner who owns it — which is what this
entry is. -->

## 32. Corners of the shared filter grammar, resolved as the other two producers resolve them — RESOLVED

**Ruling: confirmed, all three, as `oneagentgraph`'s `docs/event-filter-notes.md`
records them. The glob dialect is now stated in the contract itself, so it is no
longer a corner; the other two are implementation agreements it names.**

**The grammar has one source and a drift gate, and this entry is not about
that.** The source is the committed grammar text, which the approved contract
fixes as authoritative for all three producers; the gate is `tests/contract.rs`,
which drives that text's own example through `src/filter.rs`'s types and fails
this repository's `just check` the moment its copy stops matching. `oneagentgraph`
and `onevcs` each carry the same text and the same gate. There is deliberately no
shared crate — the same decision the envelope beside it is under, and for the same
reason: a shared crate would make three independently-released tools co-version.

What this entry is about is narrower. Three corners of that text do not settle a question
an implementation has to answer anyway, so each producer answers it — and two
producers answering it differently is a spec that filters differently depending
on who read it, which no contract test can catch because each copy would still
match the text. They are resolved here to match what `oneagentgraph`'s
`docs/event-filter-notes.md` records, and listed for the planner because the
agreement is between repositories rather than inside one.

**The glob dialect is `*` and nothing else.** `*` stands for any run of
characters including none; every other character is itself, so `?` and `[a-z]`
are literals. Stated in the contract rather than left to each implementation.

**A label matcher reads the key as the envelope carries it.** This crate's
`event::Labels` has typed slots for `run_id`, `node`, `step`, and `persona`, and
flattens everything else into `extra` — so `member`, which the grammar reserves
and this crate never stamps itself, arrives among the extras on a relayed
sibling envelope. `filter::stamped` consults the typed slot *and* the extras;
consulting only the typed slot would refuse to see a label the same consumer can
plainly read.

**`round` is not matchable.** It is a reserved label the approved matcher list
does not name, and this crate no longer stamps it at all — so it is refused by
the matcher reader like any other non-field, rather than quietly accepted as a
key nothing can satisfy.

<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

## 33. Nothing on `onevcs`'s surface says whether a published change has landed *since* — OPEN

**Proposal (for `onevcs`): a read that answers "has this branch reached its
base?" for a branch that was pushed — the change request's state, or the
comparison against the base that `Vcs::recoverable` already makes internally.**

A node's `landing` is an observation made at settlement: `onevcs publish`
answers `ChangeOpen` or `Queued`, this crate records `unlanded`, and the run
neither blocks nor polls for a merge somebody else owns. That is deliberate. What
is not deliberate is that the snapshot is all any later reader has — a node that
settled `done (queued)` was still rendering `NOT landed` hours after its change
had merged and released, and `just runs` counted it against the run.

Re-reading it needs one of two answers, and this crate can reach neither:

* **The host's.** `RemoteHost::find_changes(head, base)` is exactly the read —
  but a `RemoteHost` comes from `Hosting::for_repo(slug)`, and the `owner/name`
  slug is derived from an identity key by `onevcs`'s own private `gh::slug`.
  Deriving it here would be a second copy of a sibling's rule, which
  `src/AGENTS.md` forbids — and it would fail in the direction that matters: a
  copy that drifted, or an identity on a host that is not GitHub, would address
  *some other repository* and answer confidently about it.
* **Git's.** `Vcs::recoverable` makes precisely the comparison —
  `git diff --quiet <base> <branch>`, "whether the base already carries this
  branch's content" — but only for branches `unpublished_branches` returns
  first, which is those with commits on no `origin` remote-tracking ref. A change
  request's branch was pushed to open it, so it is excluded whether it merged or
  not, and its absence from that list says nothing. The comparison is there; it
  is not reachable for the branches that need it.

Until one exists, **no view claims to know where a change is now.** Every line
that carries an unlanded node dates its answer to the settlement, says nothing
has re-read it since, and names the change to open —
`views::landed_phrase`, `RunView::summary`, and the `status` line, all held by
`a_change_that_merged_after_settlement_is_reported_as_of_settlement_not_as_now`.
That is the honest half of what the change asked for: the stale fact is no longer
asserted, and it still cannot be corrected from here.

## 34. A drafting dispatch that produced no body was reported nowhere — RESOLVED

**Ruling: a new event kind, `body-not-drafted`, carrying which of the three
endings it was — plus the same words on the node's own settlement. A
succeeded/failed boolean, and folding the outcome into `published`'s payload,
were both put up and both refused.**

The approved contract already said what a drafting dispatch that ends badly does
to the publication: nothing. "A drafting dispatch that does not start, fails, is
cancelled, or answers with nothing the schema accepted leaves the publication
untouched." What it did not say is that the run should be able to *tell*, and the
code took that literally: `lifecycle::drafted` answered `Option<String>` and the
caller published `body.as_deref()`, so four situations produced an identical
bodyless change request — the planner supplied no body, the launch named no
pr-author graph, the dispatch failed, and the schema refused the answer. No kind
in this library named drafting at all, and `published` was the only publication
event, so the first remote lifecycle change request a newly wired drafter opened
was its own only evidence — and a bodyless one cannot say whether the drafter ran
and failed or was never wired.

The three failure endings are kept apart because they take three different fixes:
a graph that will not start or will not finish, a graph whose answers the schema
refuses, and a graph that answers inside the schema with nothing in it. Collapsing
them costs the diagnosis as well as the signal, which is why the boolean was
refused. Folding them into `published` was refused for a second reason: drafting
is not on the publication path, and a field on the publication's own record is
exactly the reading — "the publication carries what the drafter did" — that
"never on the publication path" exists to deny.

Two endings are deliberately **not** emitted, and the contract now says so: a
launch that named no pr-author graph, and a node that carried its own `body`.
Neither spends a dispatch and neither is a failure, and a kind that fired for
them would report the shipped default as a fault.

The contract carries the ruling in its shipped-content paragraph, and
`body-not-drafted` is in the closed set of this library's own kinds beside it.

## 35. A pinned branch cannot be compared against its base before a dispatch is spent — OPEN

**Proposal (for `onevcs`): a read that answers "does this branch carry anything
its base does not?" for a branch named *before* a session exists — the comparison
`Vcs::open_session` and `publish` already make internally, answering for a branch
that does not exist as well as for one that does.**

A branch-pinned lifecycle node whose content has already landed costs a whole
dispatch — provider time, a worktree, a gate run — to discover it at publication,
where `onevcs` answers `PublishOutcome::NothingToPublish` and this crate settles
`no-changes`. Twice in one run a planner had to notice that by hand and park the
node, once after a full dispatch had run to completion. The comparison that would
have answered before anything started is a single `git diff`, and this crate can
reach none of the four places it is already made:

* **The repository the comparison runs in is not nameable here.** A node's `repo`
  is an identity key, a registered alias, an origin URL, or a path, and its
  `execution_checkout` is a *registered alias*; both resolve to a checkout path
  through the registry document, which `onevcs` reads behind its own private
  `store::load` and `home::registry_path`. `resolve_identity` is the only public
  resolution and it answers an `Identity` — origin, workflow, repo type, gate —
  with no path in it. Deriving the path here would be a second copy of a
  sibling's rule, which `src/AGENTS.md` forbids.
* **Inside a session it is a different question.** `open_session` cuts a pinned
  branch nothing carries yet **fresh from the base** — `git worktree add -b
  <branch> <path> origin/<base>` — so in the session the branch and the base are
  identical by construction, which is every node this comparison is wanted for: a
  node whose content already landed is one whose branch is the base's own tree.
  A comparison made after opening answers "no diff" for all of them, which is the
  wrong answer given confidently.
* **`Vcs::recoverable` makes the comparison and cannot be asked this.** It runs
  `trees_differ` against the base *now*, which is exactly the judgement wanted —
  but only over the branches `git::unpublished_branches` returns first, which is
  those carrying commits on no `origin` remote-tracking ref. A branch whose
  content landed, one that was pushed to open a change request, and one that
  never existed are all equally absent from that list, so its silence cannot
  separate "already landed" from "no such branch" — and the second must still
  dispatch. Divergence 33 records the same limit met from the other side.
* **`open_session` looks the branch up and answers nothing about it.** It reads a
  pinned branch across every checkout of the identity and against origin's copy,
  at exactly the moment wanted, and — from `onevcs` 0.8.0 — continues the branch
  it finds instead of refusing it. Every pin now passes silently, the one whose
  content already landed included, so what used to be one silent case is the only
  case. `Session` reports the branch and its base and no comparison of the two.

**What this crate does today.** Nothing new: a branch-pinned node dispatches as it
always has, and a branch whose base already carries its content is discovered at
publication and settles `no-changes`. Nothing here runs git — no path of this
crate ever has — and a comparison written against a checkout it had to guess at
would fail in the direction that matters, settling a node as already-landed
because it looked at the wrong repository.

## 36. `attest` now also takes a node that settled `failed` — OPEN

**Proposal (for the planner who owns the contract): state that `attest RUN REF`
accepts two references — a ready `kind: human` node's action, and a node that
settled `failed` whose work a person is vouching has landed — and that the
second releases every node the failure had skipped.**

The contract names `attest` in one shape only: a decision point is "a ready
`kind: human` node's attestation", and clearing it "auto-resumes the paused
subtree". That leaves a failed node's dependents with no way back at all. A skip
is *derived* — re-computed from the dependency's status on every reconcile pass —
so a node whose dependency failed is skipped for the whole life of the run, and
nothing in the edit vocabulary says the thing that would release it. `retry`
re-runs work that is already done, `drop` detaches the dependents from the
dependency they actually had, and a `context` note reaches a node that will never
be dispatched again. Measured: a 27-node run permanently skipped a node over a
dependency whose change was already merged on `main` as a pull request, and the
run had no answer to type.

**What this crate does today is the block below, and the block is the source.**
`tests/e2e/channel.rs` parses it out of this file and answers it with a run
holding a node in every settlement it can reach — each one this list names is
attested, and each one it does not is asserted refused. So the list and the
build fail `just check` the moment either gains a settlement without the other.
A divergence nothing gates is one that quietly stops being true, and while this
is open there is nowhere else the second reference may be written down.

```json
{
  "op": "attest",
  "settlements": ["waiting", "failed"]
}
```

They are node settlements, spelled as `results` prints them, and the refusal
every other settlement gets names both of these.

The proposal is whether this belongs on `attest` at all, or whether the second
statement — "this failure's work is in the base; stop gating on it" — deserves an
op of its own. This crate is built against `attest` and will move if the ruling
says otherwise.

## 37. A plan cannot say "continue the work already on this branch" — OPEN

**Proposal (for the planner who owns the contract): state what a plan node writes
to continue a branch a previous attempt preserved — either that a node's `resume`
pins the session's branch as a `retry`'s does, or that `branch` alone is the way
and `resume` is only ever written by an edit.**

The contract says `resume` "continues a node on the branch its previous attempt
preserved: `{branch, checkpoint?, completed_steps?}`". In this build that is true
of a `retry` edit and not of a plan file: `edits::pin_retry_branch` turns a
replacement's `resume` into its branch pin, and nothing does the same for a node
read out of a plan. A plan node stating `resume` therefore skips the steps the
branch already carries — the `completed_steps` half — and then opens a session on
a **fresh** branch, so the steps that were skipped are missing from the branch
that gets published.

What a planner has left is `branch`, and `onevcs` 0.8.0 is where that became
enough. Below it a pin onto a branch carrying commits its base does not was
honoured in one case only — where an open session already held that branch, the
reuse `onevcs` 0.4.2 added — and a preserved branch nobody still held a session
for was refused with `already carries N commit(s) that main does not`. The *only*
spelling that got past that was `base_branch` equal to `branch`, because the
comparison the refusal is made on is then empty by construction; that spelling was
folklore, it was undocumented, and it cost the run its report: the publication
compared the branch against itself, answered `PublishOutcome::NothingToPublish`,
and the node settled `no-changes` with its integration target never told about any
of it. Measured, before the adoption: four nodes across three runs, every one of
them carrying the work it reported it had not written. From 0.8.0 a pinned branch
that already exists is **continued** from its own tip whatever left it there — a
session still open, a session its owner closed, or a branch somebody landed by
hand — and `base_branch` equal to `branch` is refused when the session opens,
naming the pin in its place.

That answers the capability the proposal was blocked on. It does not answer the
proposal, which is about what a *plan node writes*: `resume` still means one thing
in a `retry` edit and another in a plan file, and which of the two spellings the
contract intends is the planner's to say.

**What this crate does today.** `src/plan.rs` documents both fields — `branch` is
where the work goes and a name that already exists is continued from its own tip,
`base_branch` is the integration target, and setting the second equal to the first
is not a supported way to continue a branch and is refused by `onevcs` before the
dispatch is spent, so the node settles `infrastructure-failure` carrying the
sibling's own sentence rather than a `no-changes` nobody can act on. The statement
is held to the code from both sides: a drift test over the fields' own
documentation in `src/plan.rs`, a journey in `tests/e2e/lifecycle.rs` that drives
the refused spelling through the real repository side, and one in
`tests/e2e/session_reuse.rs` that drives the spelling it points a planner at.
Nothing about the plan schema is widened here — what a node may say is the
contract's, and the semantics the pin runs into are `onevcs`'s.

## 38. `surface` can only be handed its message on the command line — OPEN

**Proposal (for the planner who owns the contract): state the surface verb as
`surface RUN --kind check-in [FILE]`, with the message read from `FILE`, or from
stdin when none is named — the shape `reply RUN [FILE]` already has one line
earlier in the same paragraph — and keep `--message TEXT` as the inline form.**

The contract spells one way in: `surface RUN --kind check-in --message TEXT`.
That puts arbitrary agent-authored prose in a command-line argument, and a
command line is read by a shell before this process sees a byte of it. An agent
composing such a call with its message in double quotes had its own prose
executed — backticks inside double quotes are command substitution, so bash ran
the quoted command and spliced its stdout into the message. It cost about 25
minutes of CPU on a shared host for work nobody asked for, and left no audit
trail at all: the only trace was the message text mutating into command output.

Quoting discipline is not the fix. A rule an agent has to apply to prose it wrote
itself fails silently the first time it is broken, and the failure is invisible
in exactly the record that would show it. The class goes away when the body
arrives as bytes, which is what the sibling verb in the same file already does:
`reply RUN [FILE]`, "omitted, it is read from stdin".

**What this crate does today.** `surface` takes its message from whichever of the
three carried it — `--message TEXT`, a `FILE` positional, or stdin when neither
names one — and `--message` and `FILE` are refused together, so nobody has to
guess which was used. The body is trimmed at its ends exactly as `reply` trims
the envelope it reads, and everything inside it, metacharacters included, reaches
the queued surface unchanged. An empty body is refused, naming where the command
looked, so the argument that used to be required is still impossible to forget by
accident. `tests/e2e/channel.rs` drives a message full of backticks, `$(...)`,
semicolons and pipes through the stdin and file forms and asserts the queue holds
it byte for byte.

## 39. A watcher has no op for saying something, and no kind to say it under — OPEN

**Proposal (for the planner who owns the contract): add `finding` to the reply
envelope's op list and to the `monitor` allowlist, and add `finding` to the
surface kinds `surface --kind` accepts.**

The contract gives an observing member a structured envelope for *edits* and
nothing for *observations*, so a monitor emits what it saw as raw turn text — and
every turn it takes therefore produces a surface, including a turn that only
states an intent to look. Measured: one run queued **28 planner surfaces, of
which 24 were content-free preambles**, fourteen of them variants of a single
sentence. Buried among them was a blocking question from a worker that had
finished every measurement it could and needed three contract rulings; it sat
unread for 15 minutes with the whole frontier stopped behind it. The degradation
is not only the reading: "N planner update(s) waiting" is the one line this
host's operating rules say may never be filtered, and it carries no information
when 86% of N is throat-clearing.

A member-level response schema would fix the shape and cost the streaming — and
the monitor is the one member with no per-turn deadline, because it watches for
the life of the run. So the fix is an op in the envelope the member already
writes: raising a surface becomes a deliberate act, and a turn with nothing to
report emits nothing at all.

The op goes **inside `commands`**, beside the existing operations, and not beside
them as a new top-level field. The reader on the other side of this seam accepts
exactly `version`, `author`, `completion`, `message`, `reason`, and `commands`
and is closed to unknown fields, so a findings operation spelled as a tenth
top-level key would be refused whole — taking the verdict and the graph edits in
the same envelope down with it.

**What this crate does today is the block below, and the block is the source.**
`tests/contract.rs` parses it out of this file and holds it against the types:
every op named here must be one this build accepts and the contract's own list
does not, must round-trip as written, and must be allowed for exactly the authors
named; every surface kind named here must be one `SurfaceKind` carries and the
contract does not. Both directions, so a build that grows an op or a kind this
entry does not name fails as loudly as one that drops one it does.

```json
{
  "ops": [
    {
      "op": "finding",
      "message": "`build` is verifying against a base that moved under it",
      "blocking": true,
      "id": "build"
    }
  ],
  "monitor_may_issue": ["finding"],
  "surface_kinds": ["finding"]
}
```

`message` is required and an empty one is refused; `blocking` defaults to `false`,
so an observation holds nothing back unless it says otherwise; `id` is optional
and, given, must name a node the graph has — a blocking finding about work nobody
is doing would hold no subtree while still reading, in every planner view, as a
decision the run is waiting on. An accepted `finding` compiles to no graph
mutation, and the surface it raises is the only thing the planner sees for it:
the "monitor applied an edit" surface every other monitor op additionally raises
is suppressed for this one, because reporting it twice is the multiplication the
op exists to end.

Adding to `SurfaceKind` is the schema change it looks like. The set is
`#[non_exhaustive]`, so no consumer matches it exhaustively and the addition
cannot break one at compile time; what a consumer *can* pin is a release, so the
kind arrives with a **minor** version bump, cut by `release-plz` from the `feat`
commit that introduces it. Nothing in this repository writes a version by hand.

**Beside it, and not a divergence: the order surfaces are read in.** The contract
fixes which surfaces exist and that a blocking one is delivered under every
profile; it says nothing about the order a reader takes them in, and this build
now hands out **a blocking surface first**, arrival order within each class. A
blocking surface holds back the subtree that depends on it and produces no other
signal until somebody reads it, and nothing else in the queue does either of
those things — so narration loses nothing by being read second, while a question
behind it is a stopped frontier. Reading narration while a blocking surface is
pending no longer clears that pending state either: a report is not an answer,
and only a reply releases the subtree a decision is holding.

## 40. A node cannot say it depends on the *release* rather than on the work — OPEN

**Proposal (for the planner who owns the contract): add two optional node fields
to plan schema 3 — `adoption` and `consumes` — and three kinds to this library's
closed set: `release-wait`, `release-arrived`, `release-adopted`.**

The contract's plan schema says when a node launches relative to its
dependencies' **branches** and has no way to say "this node needs the *released*
thing" as distinct from "this node needs the *work*". So a plan spanning several
repositories has that sequencing done by a person: a node is held back by hand
until somebody has watched a release go out, or it launches early and the worker
is corrected mid-run once the thing it pinned against has been published. Both
are manual supervision of something the run already has the events to know, now
that `onevcs` 0.13.0 answers what a repository releases and whether the release
carrying one landed change has happened yet.

`adoption` is one of `fast` and `published`, and it is the **first rung of four**:
the node's own field, then the repository rung and the global rung — which are
`onevcs`'s and which [`onevcs::adoption_for`] answers together — then `fast`.
There is deliberately no plan-level tier and no run-only override.

`consumes` is keyed by **dependency node id** and not by repository, because the
dependency is a node: two nodes in one repository can legitimately want different
targets, and a repository key could not tell them apart. A dependency it names
none for takes the target that dependency's repository declaration marks as its
default.

Under `fast` the node launches on its dependencies' branch readiness alone — the
readiness the contract already describes, unchanged — and a dependency inside the
node's own repository still produces the stacked or merged-stacked branch it
produces today. A dependency **outside** it gains a row in a trailing block on
the node's rendered task, under `onepipeline::plan::CROSS_REPO_REFERENCES_HEADING`
and appended by the same rendering that appends `## Planner context`, so the
worker pins against git rather than against a version that does not exist yet.
When those releases arrive the still-running node is sent one `context` note at
`deliver: auto` naming the versions, framed as observed state and adding no
acceptance criteria.

Under `published` the node is **not scheduled at all** until every one of those
dependencies answers released. It holds beside the existing `paused_by` gate
rather than in place of it, and the hold is absolute: no timeout, no deadline, no
retry budget, and no automatic degrade to fast adoption. "Not answered" never
releases it and is never recorded as "not released". The run raises a
non-blocking planner surface naming what it awaits and for how long, repeated on
its own interval, so the decision to keep waiting, to flip the node to fast
adoption, or to stop the run stays a person's and is an informed one.

The scheduler is **identical for both release styles** — one hold, indefinite,
never failing — and what differs is only where the readiness answer comes from
and what is reported. An automated target's answer is its probe, which is a
subprocess: it is asked off the reconcile loop's own thread and paced on its own
interval, `ONEPIPELINE_RELEASE_POLL_SECONDS` and 120 seconds by default. A
human-step target's answer is the acknowledgement record, for which this crate
runs no probe because there is none to run. `awaiting-human-step` is carried as
its own answer through the scheduler, the surface, and the payload and is never
folded into either neighbour. This crate never performs a human release step,
never prompts for one, and never acknowledges one on somebody's behalf.

**What this crate does today is the block below, and the block is the source.**
`tests/contract.rs` parses it out of this file and holds it against the types:
the node named here parses at schema 3, round-trips exactly as written, and
carries the two fields with the values written; every event kind named here must
be one `PipelineKind` carries and the contract's own list does not; and the
heading named here must be the constant this crate publishes. The kind set is
held both directions, so a build that grows a kind neither document names fails
as loudly as one that drops one.

```json
{
  "node": {
    "id": "consumer",
    "persona": "engineer",
    "task": "## What\nbuild against the released engine",
    "deps": ["engine"],
    "adoption": "published",
    "consumes": {"engine": "crate"}
  },
  "event_kinds": ["release-wait", "release-arrived", "release-adopted"],
  "heading": "## Cross-repository references"
}
```

Both fields are **optional and omitted when empty**, so a plan naming neither
round-trips as the file wrote it and produces exactly the run it produces today:
no reference block, no hold, and a rendered task byte-identical to the one it
renders now. That is why the addition is at schema 3 rather than at a schema 4 —
there is no document a version-3 reader would refuse and no field whose absence
means something new. `release-wait`, `release-arrived` and `release-adopted`
arrive with a **minor** version bump, cut by `release-plz` from the `feat` commit
that introduces them, exactly as entry 39's `SurfaceKind` addition did.

**Two rules the journeys settled, and both are load-bearing.**

*A repository that declares **no release targets** releases nothing*, so a
dependency landing there earns no row and no hold whatever the consuming node's
adoption mode says. That is every repository on a host that has configured none —
which is every host there was before `onevcs` had a release-targets document at
all — and it is what makes "a plan naming neither field produces exactly the run
it produces today" exact rather than nearly so. The alternative reading, holding a
`published` node for ever against a repository nobody has configured, is a wait no
answer can end.

*The reference `onevcs` resolves landed work by is the **branch***. That library
knows a change request's URL, a session token, a branch a registered checkout or
run clone holds, and a commit one of *those branches* carries — and a landing
commit sitting on the base alone is none of them. So the branch is what is asked
about and the landing commit is what the reference block *shows*, which is the
cell a worker actually wants.

**One thing that is held by a fold test rather than by a journey, and why.** A
fresh driver takes up what its predecessor already said, out of the journal,
before it starts watching — so a node it finds still running is not told its
releases arrived a second time. A journey for that has to kill a driver
mid-dispatch, adopt the run, and get a *second* node told before it can assert
about the first. One was written. It is green on its own in about fourteen
seconds, and it timed out against the suite's 120-second deadline on three of four
runs of the instrumented suite, while holding e2e concurrency slots the rest of it
needs; the causes ruled out along the way were an undrained child descriptor and
two lifecycle nodes contending for one repository's session lease, and neither was
the whole of it. A test whose verdict depends on how loaded the host was is worse
than none, so it is not in the suite. What the seeding *is*, is a fold of a
durable record, and `src/release.rs`'s
`a_fresh_driver_takes_up_what_its_predecessor_already_said` drives exactly that in
both directions — a node the predecessor told is not told again, and one it never
told still is. Both deliveries either side of it are driven end to end.

**Two things this crate could not compile as the workstream described them.**

*The global adoption rung is not reachable for a node with no `repo`.* The chain
is node → repository → global → `fast`, and a node with no repository is meant to
fall from the first rung straight to the third. `onevcs` answers the second and
third rungs together, through `adoption_for(repo)`, and publishes no way to read
the global rung without naming a repository — `releases::ReleasesFile` is a public
type with no public loader. So such a node falls to the floor instead, which is
what the global rung itself answers on every host that has not set
`default.adoption: published`, and differs only on one that has. **Proposal for
`onevcs`: publish the global rung on its own** — a `global_adoption()`, or a
loader for the release-targets document — and this crate reads rung three where
it now reads rung four.

*A deferred arrival note is owed to the next dispatch the engine starts, which
within one run is limited.* Where the running turn has no controllable lever the
note is recorded `delivery: next` and folded back onto the node exactly as a
planner's own deferred `context` note is — same mechanism, same rendering, same
consumption by the dispatch that takes it. What is limited is not this addition
but the mechanism it reuses: a node that has already settled is not re-derived
ready by any user-facing verb, so its owed note waits for whatever does dispatch
it next. That is the `context` op's own reachability and is not changed here.

**What became of the blocker this entry used to carry.**

*All three of the sibling's release kinds reach this run's store, and the join
that carries the other two is `onevcs`'s own.* `release-probed` is emitted on the
**session's** own stream when a publication captures its baselines, so the
session follow that already exists relays it into the merged store — unchanged
and unrewritten, like every other `onevcs` kind. `tests/e2e/adoption.rs`'s
`the_siblings_release_probed_is_relayed_exactly_as_its_producer_wrote_it` holds
that field by field against the sibling's own copy, read back through `onevcs`'s
own reader out of the stream the producer wrote it on: `v`, `ts`, `stream`,
`seq`, `source`, `kind`, `phase`, `payload`, and `artifacts` are identical, and
every label the producer stamped stands, with `node` — which the producer cannot
know — added beside them.

`release-observed` and `release-acknowledged` are recorded on the **identity's**
own release record instead. A release happens long after the dispatch that
produced the work has ended, outside every session, which is why
`release-observed` carries the landing commit as the only thing that could
correlate it. This entry used to record that as a **terminal blocker**: the
reader for that record was published and its *name* was not, so the only way to
open it was to restate a naming scheme `onevcs` owns — a SHA-256 recomputed to
spell a filename — which this crate refused to do, because the day that scheme
changed a consumer that had guessed it would relay nothing, silently, and no test
that guessed the same way would catch it.

**`onevcs` 0.14.0 resolves it, and resolves it at the right end.** That library
now stamps a `phase` on every envelope and **joins the identity's release record
to the session whose landing commit it names**, so
`EventStream::open_filtered` — handed the session token this crate already
holds — returns that session's releases beside the session's own records. The
address of the second stream is never handed out, named in a refusal, or
derivable: the consumer asks about the session it knows and the sibling answers
about the work. `0.14.0` is the floor, and `Cargo.toml` required exactly it when
this was written — pre-1.0 the minor is the breaking position, so `^0.13`
excluded it and the requirement rather than the lock was what permitted it. The
requirement now reads `0.15` and `Cargo.lock` resolves `0.15.0`, from
`registry+https://github.com/rust-lang/crates.io-index`, which is above the floor
and carries all of it. The
proposal this entry made — *publish the name* — is **withdrawn**: the scheme
stays private, which is the outcome it was asking for.

What this crate does with it is one paced read and no new vocabulary. On the
reconcile loop's own passes, on the release watch's interval — `ONEPIPELINE_RELEASE_POLL_SECONDS`,
120 seconds by default, the same bound the probe is asked under and not a second
one — every node that has **settled** and whose repository declares release
targets has its session read through that reader, under the launch's own
`filters.vcs`: the same value the follow was opened with, crossing the same seam.
Everything in the **Release** phase past the mark its own stream already stands
at in this run's store is relayed, enriched with the run and the node the producer
could not know and with nothing rewritten. A node still running is skipped,
because its own follow is reading that session and two readers deciding
separately what the store already holds is how a record arrives twice. There is
no launch key, no flag, no plan field, no run-only adoption override, and **no
second wait control**: the hold is still the one this entry describes — four
rungs, one hold, indefinite, never failing, released only by an answer of
released.

Four things follow, and each is held by a journey in `tests/e2e/adoption.rs`:

- `the_siblings_other_two_release_kinds_reach_this_run_through_the_public_session_reader`
  emits both kinds for real — the sibling's own `release status` and its own
  `acknowledge` — and holds what reaches the store field for field against what
  the same public reader hands back, with `release-probed` arriving exactly once
  and no `(stream, seq)` in the store twice. A **stranger's** landing is put on
  the same identity's record first, by a run of its own, and is absent from this
  run entirely: what correlates a release is the landing, not the repository.
- `a_release_of_retried_work_is_attributed_through_the_newest_session_of_its_branch`
  is the dangerous half of that. A branch two sessions have worked on — a run that
  landed on it and a later run pinned to the same name, which `onevcs` continues by
  cutting a second session onto its tip and recording the first as superseded — has
  **two** landings, and only one is the work. The release that reaches the run
  carries the second, and the superseded session's own record resolves along its
  retry chain to that same landing when it is read through the public reader. A
  reader that stopped at the superseded copy would answer that the branch had not
  landed, which is the answer that invites re-running work that already merged.
- `a_launch_that_excludes_the_release_kinds_relays_none_of_them` states
  `filters.vcs` as `exclude: [{kind: "release-*"}]` and gets a store with none of
  the three in it and the same session's other records all present — narrowed
  rather than silenced, through the control an operator already has.
- `a_plan_naming_neither_field_runs_exactly_as_it_did` is the other end: a host
  with no release-targets document has no repository with a release phase, so
  nothing is recorded about a release and nothing is read, both sessions close,
  and the run ends exactly where it ended before there was a release record at
  all.

**Three shared-contract departures this change makes, and none of them is made in
`docs/contract.md`.**

*The merged envelope carries a `phase`.* `docs/contract.md` declares the envelope
and does not name the field. It is carried because it is the producer's own
classification of its own event and dropping it in the relay would lose what only
the producer knew — `onevcs` puts a push of the session's branch and a push of the
base it landed on in different phases, and no reader downstream can recover which
one it was. It is **optional and omitted when absent**, inside `v: 1`, exactly as
`onevcs` added it to its copy: a store written before this field round-trips as
its writer wrote it, and everything this crate emits and everything
`oneagentgraph` produces carries none. **Proposal for the planner who owns the
contract: add `phase` to the envelope that document declares, and to the matcher
list beside it.** Until that is ruled on, this crate carries the *field* and not
the *matcher* — the approved matcher list is `source`, `kind`, and the reserved
labels, and a spec naming `phase` is refused by name exactly as the grammar says
it should be, so a launch that wants a phase kept out names the kinds in it.
`tests/contract.rs`'s
`the_envelopes_phase_is_the_siblings_own_vocabulary_and_all_of_it` holds this copy
to the sibling's, exhaustively and in both directions, so a phase either side
grows alone fails `just check`.

*A settled node's session is read again after its follow has ended.* The contract
says a lifecycle node's session is followed "from the moment there is a token
until the session closes", with a read-once fallback for a follow that never
started or did not end cleanly. A release happens after all of that, by
construction, so the record of one cannot reach the store through either. What
the paced read adds is bounded in the two ways that matter: it reads only nodes
whose dispatch has settled, and it relays only the Release phase — every other
record of that session was relayed by the follow that watched it, and the marks it
reads past are the store's own. **Proposal: extend that sentence to say that a
settled node's session is re-read for the releases that carried its work, on the
release watch's interval, until the run ends.**

*Per-stream `seq` on the identity's release record is not contiguous for one run,
and cannot be.* The contract says a consumer detects loss through per-stream `seq`
gaps. That record is the **repository's**, shared by every session in it, and a
run is handed only the events correlated to its own landings — so a gap in that
stream's series is another session's release rather than a record this run lost.
Every other stream in the store is unaffected, and the accounting is kept **per
stream** rather than per read for exactly this reason: one mark over a session's
own records and its repository's releases together would let the higher series
hide the lower one's next record, which is a relayed record lost in silence.
**Proposal: say in the contract that a relayed stream a producer shares across
runs is contiguous per producer and not per run.**

## 41. A manager ruling cannot bind a node's judge, and a live-edited node is checked by nothing — OPEN

**Proposal (for the planner who owns the contract): add an `amend` op to the
reply envelope — and **not** to the `monitor` allowlist — with one optional node
field, `amendment`, at plan schema 3; and add an optional launch-level node
validator, named by `--node-validator`, `ONEPIPELINE_NODE_VALIDATOR`, and a
`node_validator` key at launch-config schema 3.**

Two halves of one seam, and the second is where the first is checked.

**A ruling that reaches the worker reaches nobody else.** A node's judge reviews
against `base ⊕ persona ⊕ --task`, and the contract's `context` is the only thing
a manager can send mid-dispatch. It reaches that task declaring itself
non-binding — "This reports observed state and adds no acceptance criteria" — and
a `deliver: live` note reaches it not at all: it is an interrupt against the
agent's control socket, persisted nowhere, so it reaches neither the judge nor a
later dispatch of the same node. Measured: a manager ruled a change out of scope,
the worker complied and re-ran its complete gate green, and seven minutes later
that node's own judge instructed it to **restore** the change — reviewing against
a task that never mentioned the ruling. The worker then held two contradictory
instructions with identical authority, and resolving it took a `retry` with an
amended `task`: killing a live, gate-green dispatch in order to change its bar.

`amend` is that lever. It names a node the graph holds that can still be
dispatched — a node that has settled `done` is refused for the reason `context`
refuses one, since nothing will read the amendment — and carries non-blank text,
a blank one being refused rather than recorded. The text becomes part of the
node's **effective task** permanently: the worker and its judge read the same
sentence on the dispatch that follows the amendment and on every later dispatch
of that node. A turn already in flight is not reached — its task was composed
before the ruling existed, and so was the one its judge reads — which is the
asymmetry with `context`, whose whole point is the turn running now. Repeated
amendments **replace**, because a bar that can only grow cannot be corrected — a
ruling issued and then thought better of would go on binding the judge beside its
own correction, which is the same two-instructions-one-authority failure the op
exists to end. Because replacing loses a ruling where appending would not, a
node's current amendment is readable from the run's `status` view and from its
per-node `results` view before anything replaces it. It is journalled as an
operation of its own, so replaying a run's journal reconstructs the amended task
without re-judging it.

It is rendered under a heading of its own, immediately **above** the task's
`## Additional info` section where the task has one and at the end otherwise, and
opens with the sentence that states its authority: *where this section and the
operational notes below disagree, this section wins*. That placement and that
sentence are the convention that resolved this in practice — both supervisory
conflicts in the session this comes from traced to an instruction's authority
being written down nowhere.

It is **not** on the monitor allowlist. What a node is judged against is a
decomposition decision the monitor's own persona already reserves to the planner,
and an observer that could move a bar could resolve an ambiguity by editing
rather than escalating.

The important half is that the **distinction is instructed** rather than merely
available: `amend` changes what the node is judged against, is permanent, is
visible to every party, and survives a re-dispatch; `context` steers the worker
only, says so, is transient, and does not alter the bar. The persona and the
README state that pairing as the reason both exist.

**And a node introduced by a live edit is validated by nothing.** A plan file is
checked before launch by a host validator that refuses a node whose acceptance
criteria name a procedure instead of a property, are silent about a demand its
resolved review bar makes, or rest on work the dispatch cannot perform. A node
introduced by `add` or `retry` is dispatched identically and reaches no such
check: nothing on the reply path invokes one. Every node introduced by live edit
in one session — three of them — went in unchecked.

Running that validation *inside* this crate is not implementable and that is why
this is a hook. The validator is hundreds of lines of one operator's
host-specific rules, reading that host's dispatch appendix, its shared review
contract, its personas directory, and the personas compiled into the
`oneagentgraph` this crate links — none of which this crate has or should have.
So `onepipeline start` gains an optional **command**: the node crosses as JSON on
its stdin, exit 0 accepts the edit, and a non-zero exit refuses it with the
command's own stderr as the reason, in the same shape a refused op already gets.
Four ops are offered to it, and they are exactly the ones that put unchecked task
prose in front of a dispatch: `add`, `retry`, a `requeue` whose amendment touches
`task`, and `amend`. A launch that names no validator behaves exactly as it does
today.

An accepted edit is offered to it **twice** — once by the submission check and
once by the reconciler — because those two run one validator between them, which
is what makes "applied or rejected with a reason" true: an envelope reaching the
loop may have been written by a build or a caller that did not check. A node
validator is a read-only check of one node, so asking it twice asks the same
question; a refused edit is asked once, because the submission check turns it
away before anything is queued.

The value is the executable itself, invoked with no arguments of its own: this
crate names an external program the way it already names `oneagentgraph`, and a
host needing arguments wraps them in a script — which a host carrying that many
rules has anyway. A validator that cannot be *started* refuses the edit rather
than letting it through, because accepting one would be this crate deciding that
an unenforced rule is no rule, silently, on the path a manager reaches for under
pressure.

It is nameable three ways, like this crate's other launch-level configuration,
and the order between them composes the two rules this crate already states
rather than inventing a third: the contract says of the launch config that *"the
config is the base and each flag overrides the part of it that it names"*, and
every `ONEPIPELINE_*` setting is read as an override of a shipped default. So,
highest first: the flag, then the environment variable, then the launch config
field, then the shipped default of no validator at all. It is resolved **once**,
at the launch, and retained in the launch record beside the graphs — so an
`adopt` replays the validator its launch resolved rather than re-reading an
environment that has since moved.

`node_validator` is a launch-config key versions 1 and 2 never had, so the
version this build writes moves to **3**, the versions it reads keep 2 and 1, and
a document declaring 1 or 2 while carrying it is refused **by that field's name**
— exactly as a version-1 document carrying `pr_author_graph` is. Each key is
refused by *its own* arrival version rather than by the schema's current number,
so a version-2 config naming the drafting graph version 2 introduced is still a
document this build reads.

`amendment` is at plan schema **3** rather than at a schema 4, by entry 40's own
argument: it is optional and omitted when absent, so a plan naming it round-trips
as the file wrote it, there is no document a version-3 reader would refuse, and
there is no field whose absence means something new. The op and the flag arrive
with a **minor** version bump, cut by `release-plz` from the `feat` commit that
introduces them, exactly as entries 39 and 40 did.

**What this crate does today is the block below, and the block is the source.**
`tests/contract.rs` parses it out of this file and holds it against the types:
the op named here must be one this build accepts and the contract's own list does
not, must round-trip as written, and must be allowed for exactly the authors
named; the node must parse at schema 3 and round-trip exactly as written; the
heading must be the constant this crate publishes; and the launch config must
parse at the version stated and carry the key. `tests/e2e/node_validator.rs`
reads the three spellings out of the same block and drives their precedence
against a real validator command, so the order is proven rather than asserted in
prose.

```json
{
  "ops": [
    {
      "op": "amend",
      "id": "build",
      "text": "The redundant comment lines are out of scope for this node: leave them."
    }
  ],
  "monitor_may_issue": [],
  "node": {
    "id": "build",
    "persona": "engineer",
    "task": "## What\nship it",
    "amendment": "The redundant comment lines are out of scope for this node: leave them."
  },
  "heading": "## Amendment",
  "validator": {
    "flag": "--node-validator",
    "environment": "ONEPIPELINE_NODE_VALIDATOR",
    "config_key": "node_validator",
    "config_schema_version": 3,
    "precedence": ["flag", "environment", "config_key"],
    "ops_offered": ["add", "retry", "requeue", "amend"]
  }
}
```

## 42. `adopt` takes no flags, so a run can only be recovered attached — OPEN

**Proposal (for the planner who owns the contract): state the adoption verb as
`onepipeline adopt RUN [--attach|--detach]`, the same pair `start` carries one
paragraph earlier, with the same default and the same meaning — and say of the
detached form that the driver it retains is `onepipeline drive-run RUN --adopt`,
which is where the adoption is recorded because it is the process that takes the
lock.**

The contract spells one way in: *"`onepipeline adopt RUN` attaches a fresh
driver to an intact ledger"*. `start` has both forms in the same paragraph, and
adoption is the documented way back from a driver that died — so a manager
supervising several runs at once could recover one of them only by blocking the
session watching the others. The way out an operator reaches for is
backgrounding the launcher by hand, which is the pattern this host's own
operating doctrine spends a paragraph forbidding: a missing flag that pushes a
person into a defect is not merely an inconvenience.

Adoption has an ordering the launch verb does not, and it is what decides where
the work goes. `adopt` checks ownership, refuses a run something is genuinely
driving, ends a driver holding a run nothing is driving, and **takes the
ownership lock before anything is written** — because an adoption that recorded
itself and then lost that race would leave the record naming a process that is
not driving the run. A detaching launcher cannot hold that lock on the driver's
behalf: it is about to exit, and the lock is released when it does. So the split
is by what each process can be the single writer of. The launcher makes every
check — they are refusals an operator has to see, and a detached one would
otherwise answer through a log file nobody is reading — and ends the parked
driver, saying so on its own stderr. The **retained driver** takes the lock and,
under it, counts the adoption, moves the dead driver's record aside, journals
`driver-adopted`, launches the observer, and writes the record naming itself.
One writer of the lock and one writer of the pid, and between the launcher's
last check and the driver being up the run reads exactly as those checks left
it: undriven, and adoptable again.

The launcher still waits, exactly as `start --detach` waits: it returns when the
record names the process it retained, and a driver that died on its way up is
the refusal it is, carrying the tail of what that driver said out of a log the
launcher is about to walk away from.

**What this crate does today.** `adopt` takes `--attach` and `--detach`, which
refuse each other and default to attaching, so an adoption naming neither is
exactly the adoption this verb has always performed. Detached, it prints the
same launch record a detached `start` prints — the run, the driver's pid, and
the two verbs that reach it. The hidden `drive-run` verb entry 25 records grows
one hidden flag, `--adopt`, which is the adoption's own bookkeeping done where
the lock is; it stays `hide = true` and stays reached by
`scripts/smoke-published.sh` under the same requirement. `tests/e2e/driver.rs`
drives all four outcomes against the real binary over a real run root: the
detached adoption whose driver holds every claim the run records, the one whose
driver dies on its way up, and the pair refusing itself.

## 43. One thing Contract F could not be compiled exactly as written — OPEN

The launch contract that replaced the plan file is implemented as it is written,
with one exception, where its environment-variable spelling collides with the
child product's configuration namespace.

*`ONETASKGRAPH_BIN` is a name that product's own environment layer already
claims.* The contract names the variable, and it is the one this build reads. But
`onetaskgraph` reads its **whole configuration** from `ONETASKGRAPH_`-prefixed
variables, where the suffix is a dotted setting path — so a child spawned with
`ONETASKGRAPH_BIN` still set is a child told to configure a setting called `bin`,
and it refuses the read by name. This build therefore removes the variable from
the child's environment before spawning it. **Proposal: either keep the name and
say in the contract that it is not passed through, or move it to a name outside
that product's namespace — `ONEPIPELINE_ONETASKGRAPH_BIN` is what this crate
already calls the equivalent for `oneagentgraph`.**

## 44. The minimum `onetaskgraph` this build needs is not a released version — OPEN

`src/taskgraph.rs` declares `CHECKED_MINIMUM = "0.1.0"`, which is the version the
binary carrying the surface this mapping reads **reports**, and the launch refuses
anything below it. But the reserved metadata map every field of the mapping rides
on landed in that repository *after* its 0.1.0 release and before the next one, so
the published 0.1.0 does not carry it: a host that installed from crates.io today
passes the version check and then answers every project read with a task carrying
no metadata at all, which this build refuses as a task with no `onepipeline.id`.
The refusal is correct and names the key, but it names the wrong cause.

This repository's own checks do not depend on that gap: `justfile`'s
`_ensure-onetaskgraph` installs the revision pinned there, which is the one
carrying the surface. **Proposal: when onetaskgraph cuts the release that carries
custom metadata, raise `CHECKED_MINIMUM` to it and turn the justfile's revision
pin into a version.** Until then the floor is the honest one — the version the
binary this build is checked against reports — rather than a number invented for a
release nobody has published.

## 45. A reply envelope is checked one command at a time, so nothing reviews the edit — OPEN

**Proposal (for the planner who owns the contract): add a second, launch-level
hook — an **envelope reviewer**, named by `--envelope-reviewer`,
`ONEPIPELINE_ENVELOPE_REVIEWER`, and an `envelope_reviewer` key at launch-config
schema 4 — invoked once per accepted reply envelope, after every command in it
has passed this crate's own validation and the per-node validator of entry 41,
and before any of its operations is committed.**

Entry 41's validator closed the hole it was written for and cannot close this
one. It is invoked **per command**, inside the compile step, and is handed one
node serialized on its own: no goal, no siblings, no dependency edges, and no
plan. So a reply carrying several related ops is seen as several unrelated
nodes, and nothing checks two added nodes that duplicate each other, a contract
seam *between* two nodes of one edit, the dependency edges the edit introduces,
or whether the edited graph still delivers the run's goal. Those are the checks
a plan-quality reviewer makes over a whole plan, and no per-command prompt can
carry them.

Measured: a manager under quota pressure took over a failed planner's work and
wrote a node's acceptance criteria directly, with no research into the
repository the node targeted. The criterion it wrote — that no dependency
requirement in any manifest may change and the diff must touch the lockfile
alone — contradicted a rule that repository states in its own test suite beside
the assertion enforcing it. The dispatch correctly refused to weaken the check,
stalled, and reported the conflict, costing a dispatch and a relaunch. A
deterministic check cannot see that class of failure — the criterion is
well-formed, and only the target repository knows it is wrong — and a per-node
prompt cannot see the half of it that lives between nodes.

One document crosses the reviewer's stdin: the run's **goal**, every node the
envelope introduces or changes as a `changes` list carrying the **op** that
produced each, and the **plan** they are being edited into, as the envelope
leaves it — so the reviewer sees the graph the run would converge on rather than
one it has to assemble, and which nodes are the edit rather than its context is
the list rather than a diff it works out. The goal is hoisted out of the plan it
is also part of, because it is what the whole envelope is judged against. The
ops that put a node in `changes` are the four entry 41's validator is offered —
`add`, `retry`, `amend`, a `requeue` carrying an amendment — plus `reparent`,
which changes the edges a whole-plan review is about; a `requeue` is here on any
amendment rather than on one touching `task`, because a changed turn budget is
part of the node the reviewer reads. `drop`, `cancel`, `context`, `attest`,
`complete`, and `finding` add no node to the list, and the plan is where a
review sees what they did.

Exit 0 accepts the envelope. A non-zero exit refuses it **whole** — no command
of it half-applies — carrying the reviewer's own stderr, bounded and
control-stripped exactly as entry 41's refusals are, and naming every op and
node it was reviewing, because an envelope is no longer one command and a reason
nobody can locate is a reason nobody can act on. Its stdout goes nowhere,
because this runs inside `reply`, whose own stdout is a parsed verdict. It
**fails closed**: a reviewer that cannot be started refuses the envelope, for the
reason entry 41's validator does. A launch that configures none behaves exactly
as it did before this hook.

**The refusal names the node the reviewer objected to**, which is not the set it
was reviewing: the envelope is no longer one command, so a reader given only the
list it looked at still cannot tell which node to go and change. Only the
reviewer knows which one, so it **declares** it, on a line of its stderr reading
`objection: cover` — the prefix matched case-insensitively, one line per node,
anywhere in what it says, and repeatable for an objection about the seam between
two of them. A prefix on the stream the hook already has, rather than a second
channel: its stdout is a parsed verdict's and unavailable, and a JSON answer
would make every host's reviewer a serializer to say one node's name. The
declarations are lifted out of the stderr before the rest is quoted, so a
refusal does not read the same name back in front of the reviewer's own
sentence.

A reviewer that declares nothing is not refused for it — the shell scripts
written against this hook before the line existed declare none — and it is not
reported as objecting to everything either. The refusal states which of three
answers it got, because a reader acts differently on each: a node the envelope
changes is one to go and fix, a name the envelope does not carry is the reviewer
pointing somewhere else (at a node already in the plan, or at nothing), and no
declaration at all is a refusal whose target is simply unstated. Reporting the
third as the first, by listing every node the envelope carried, is the failure
the declaration exists to end.

**An accepted envelope is offered once**, which is the one property that does not
follow entry 41 — that validator is offered an accepted edit twice, at the
submission check and again by the reconciler. Three reasons, pointing the same
way. Asking twice is free for a read-only script; this hook exists for a review
no deterministic check can make, so the host answering it is plausibly an agent,
and a second offer is a second bill for one question. The submission check is
also the only place a refusal can still be **whole**: the reconciler applies an
envelope's commands one at a time and stops at the first refusal, so a reviewer
consulted there would be answering about edits already committed. And it is the
one door — every envelope carrying commands reaches the durable queue through
that check — so once there is once per envelope rather than once per path. The
code records this reasoning where it makes the choice.

It is nameable three ways, in the order entry 41 states and for the same reason:
the flag, then the environment variable, then the launch config field, then the
shipped default of no reviewer at all. It is resolved **once**, at the launch,
and retained in the launch record beside the validator — so a `reply` typed in
another shell, and a driver a fresh `adopt` starts, use the reviewer the run was
launched under.

`envelope_reviewer` is a launch-config key versions 1 to 3 never had, so the
version this build writes moves to **4**, the versions it reads keep 3, 2, and 1,
and a document declaring an earlier one while carrying the key is refused **by
that field's name** — exactly as a version-2 document carrying `node_validator`
is. A key present and blank is refused by name too, as `node_validator` is: it
arrives with this version, so no config on disk carries one. The hook arrives
with a **minor** version bump, cut by `release-plz` from the `feat` commit that
introduces it, exactly as entry 41's did.

**What this crate does today is the block below, and the block is the source.**
`tests/contract.rs` parses it out of this file and holds it against the types:
the flag must be one `start` takes, the config key must parse at the version
stated, and the document must be built out of this crate's own published shapes —
each `changes[].node` a plan `Node` and `plan` a `Plan`, both round-tripping as
written. `tests/e2e/envelope_reviewer.rs` reads the three spellings, the
`ops_listed_as_changes` list and the `objection_prefix` out of the same block and
drives them against a real reviewer program — every op the protocol has, through
the real CLI, held against that list in both directions, and a declaration
composed from that prefix held against the node the refusal then names — so what
a reply actually hands a host, and what it reads back from one, is proven rather
than asserted in prose.

```json
{
  "reviewer": {
    "flag": "--envelope-reviewer",
    "environment": "ONEPIPELINE_ENVELOPE_REVIEWER",
    "config_key": "envelope_reviewer",
    "config_schema_version": 4,
    "precedence": ["flag", "environment", "config_key"],
    "offers_per_accepted_envelope": 1,
    "objection_prefix": "objection:",
    "ops_listed_as_changes": ["add", "amend", "reparent", "requeue", "retry"],
    "document": {
      "goal": "close the coverage gap",
      "changes": [
        {
          "op": "add",
          "node": {
            "id": "cover",
            "persona": "engineer",
            "task": "## What\nadd the missing tests",
            "deps": ["build"]
          }
        }
      ],
      "plan": {
        "schema_version": 3,
        "goal": {"text": "close the coverage gap"},
        "name": "coverage",
        "concurrency": 4,
        "tasks": [
          {"id": "build", "persona": "engineer", "task": "## What\nbuild it"},
          {
            "id": "cover",
            "persona": "engineer",
            "task": "## What\nadd the missing tests",
            "deps": ["build"]
          }
        ]
      }
    }
  }
}
```
