# onetaskgraph

![A terminal showing five task rows — three from a folder of plans and two from the folder the team's tickets live in, interleaved in configured-name order, each row a qualified id, a status category and a title](docs/screenshots/task-list.svg)

One interface over the ticketing systems your work actually lives in.

Every image in this README is a real capture of this CLI's output, produced by running the
binary against a small fixture and refused by CI the moment its bytes stop matching what
the binary prints — so the pictures cannot drift away from the tool.

Tasks, projects, labels and the dependencies between them are spread across Linear, GitHub
Projects and a folder of Markdown files, and every tool that wants to reach them ends up
reimplementing all three. `onetaskgraph` implements them once, behind a single query
surface: a command-line tool, the Rust engine crate's library API, and SDKs for Python and
TypeScript. Every consumer reaches the same engine, so their query semantics cannot drift.

Two properties make it different from a lowest-common-denominator wrapper:

- **A rich source is not reduced to a poor one's floor.** Each source declares what it can
  do natively. The engine pushes those predicates down and compensates in memory for the
  rest — and every response carries the plan it ran, so `--explain` shows you which source
  filtered server-side and which one the engine narrowed for.
- **Nothing of your work is kept outside the system that owns it.** No cache, no index,
  no local mirror. The engine holds at most one source page at a time and writes nothing
  down — enforced by a supply-chain gate that refuses every embedded store and cache
  crate, a sandboxed journey that fails if any file written during a run contains your
  data, and an assertion that the same query asked twice reaches the source twice.

  `copy` is not an exception to that, and the difference is worth stating plainly. A
  destination write is at your explicit request, names its destination, goes through that
  source's own write interface into that source's own store, and is never read back to
  answer a query. A cache is a write nobody asked for that the engine reads back. Copying
  a task into a folder of Markdown puts it in the plugin that now owns it; nothing is
  kept anywhere else, and the sandboxed journey above drives `copy` and fails, naming the
  path, if any file outside that destination's own store changes.

> **Status.** The plugin contract, the workspace, the gate, the configuration layer and the
> query engine are in place, and the binary answers every verb below. The three sources
> this product ships for your work — `local-md`, `linear` and `github-projects` — can all
> be read and written; none is read-only. A copy can still refuse a configured destination
> with no write side, such as a subprocess source whose capability handshake declares no
> writes, but that is a property of that configured source rather than of these plugins.

## Using it

Each source says what it can do, and `sources list` is where you read that back: the
predicates it applies itself, how far it walks dependencies, and the largest page it will
serve.

![A terminal showing two sources, one in-memory and one local-md, each with the predicates it applies natively, the directions it walks task and project dependencies, and its maximum page size](docs/screenshots/sources-list.svg)

```bash
onetaskgraph sources list
onetaskgraph sources fields <SOURCE> [--apply] [--json]
# Without --apply this only reports what a GitHub Projects board lacks of the fields its
# source's configuration names: the Status options status_mapping resolves to, and the
# Priority field and its options when priority_mapping is set. --apply adds the missing
# options and creates a missing Priority field, sending every existing option back with its
# id, then verifies every pre-existing option and every item's values of both fields and
# prints recovery data if GitHub drifted; a status name is reported missing for each item kind
# that names it. For a Linear source it reports every name its status_mapping gives a task
# against the team's workflow states, and every name it gives a project against the
# workspace's project statuses, each present with its type or missing; --apply creates each
# missing one first, of the type its category derives, and renames, retypes or deletes nothing.
# A GitHub Projects source's metadata_fields are reported too, each text field there or
# missing; --apply creates a missing one as a text field and reports, without changing, a
# same-named field of another type; a Linear source's are reported as carried in each item's
# collapsed Fields section, with nothing to create. See "Status mapping" below, and
# docs/metadata.md for metadata_fields.
onetaskgraph sources status-options <SOURCE> [--apply] [--json]
# The Status-only form of `sources fields`, which supersedes it; kept as it was.
onetaskgraph sources route <SOURCE> [--repository R]... [--classification public|private] [--json]
# Where an item with these repositories and this classification, written to SOURCE, would land
# by its routes — read from configuration alone, never from a source. See "Routing" below.

onetaskgraph task list [--source S]... [--label L]... [--not-label L]...
                       [--status S]... [--priority none|urgent|high|medium|low]...
                       [--project P [--members] | --no-project] [--commented-since RFC3339]
                       [--metadata KEY[/SEGMENT...]=VALUE]... [--origin SOURCE:ID]
                       [--search TEXT] [--in title|content|both]
                       [--limit N] [--page TOKEN] [--all] [--explain] [--allow-partial] [--json]
                       # --all: every page at once; a repeated cursor is refused as repeated-page
onetaskgraph task show <ID> [--no-comments] [--pin VERSION] [--with-deps]
                       # --with-deps: the task's forward edges beside it, as `depends_on`
onetaskgraph task show-many <ID>... [--no-comments] [--pin ID=VERSION]...
onetaskgraph task deps <ID> [--direction depends-on|depended-on-by] [--pin VERSION] [--complete]
onetaskgraph task copy <ID>... --to <SOURCE> [--match-by KEY] [--recreate] [--create] [--dry-run]
                       [--status CATEGORY]... [--expect VERSION]  # --expect: one task, caching destination
                       [--if-version [ID=]VERSION]...  # ID=VERSION: that task's counterpart
                       # answered failed as version-moved when it moved, the others copied
onetaskgraph task copy --to <SOURCE> [--source S]... [--commented-since RFC3339]
                       [--metadata KEY[/SEGMENT...]=VALUE]... [--status CATEGORY]...  # by query
onetaskgraph task locate <ID>... --to <SOURCE> [--no-counterpart | --target-native NATIVE-ID]
                         # where that copy would land each task; writes nothing — see
                         # "Where a copy would land: task locate"
onetaskgraph task comment add    <ID> [--body-file PATH] [--author NAME] [--expect VERSION]
                                      [--if-version VERSION]
onetaskgraph task comment list   <ID> [--pin VERSION]
onetaskgraph task comments <SOURCE> --since RFC3339 [--metadata KEY[/SEGMENT...]=VALUE]...
                           [--status S]... [--item ID]...
                           # every comment made or edited since then, and the next watermark
onetaskgraph task comment add-many <SOURCE> --input FILE|-
                           # many comments in one call, each answered on its own
onetaskgraph task find <SOURCE> --queries FILE|-
                           # many narrowed searches in one call, each answered under its key
onetaskgraph task update-many <SOURCE> --input FILE|-
                           # many held multi-field updates in one call, each answered on its own
onetaskgraph task comment edit   <ID> <COMMENT-ID> [--body-file PATH] [--expect VERSION]
                                      [--if-version VERSION]
onetaskgraph task comment delete <ID> <COMMENT-ID> [--expect VERSION]
onetaskgraph task status set <ID> draft|backlog|todo|queued|in-progress|done|cancelled|unknown
                             [--expect VERSION] [--if-version VERSION]
onetaskgraph task status reconcile --source <SOURCE> [--dry-run]
onetaskgraph task status complete --issue <OWNER/NAME#N>... [--source <SOURCE>]... [--dry-run]
onetaskgraph task priority set <ID> none|urgent|high|medium|low [--expect VERSION]
                               [--if-version VERSION] [--when-priority PRIORITY]...
                               # --when-priority: write only over these; else `outcome: held`
onetaskgraph task assignee set <ID> [<PERSON>] [--if-version VERSION]
                               # replaces every assignee; no PERSON unassigns — see "Assignees"
onetaskgraph task content set <ID> --file PATH [--expect VERSION] [--if-version VERSION]
onetaskgraph task metadata set <ID> <KEY> <VALUE> [--expect VERSION] [--if-version VERSION]
onetaskgraph task update <ID> [--title TITLE] [--body-file PATH]
                         [--status CATEGORY [--status-name NAME]] [--priority PRIORITY]
                         [--metadata KEY=JSON]... [--remove-metadata KEY]...
                         [--delivers ID... | --no-delivers] [--depends-on ID... | --no-depends-on]
                         [--repository ORIGIN... | --no-repositories]
                         [--when-priority PRIORITY]...  # every field, only over these
                         [--expect VERSION] [--if-version VERSION]
                         [--term-scope REPOSITORY... | --term-scope-empty]
onetaskgraph task create <SOURCE> --project P --title TITLE
                         [--template FILE [--search-path DIR]... | --template-loader FILE
                          | --body-file PATH]      # none of the three: the body on stdin
                         [--answers FILE] [--var NAME=VALUE]... [--status CATEGORY]
                         [--label L]... [--repository R]... [--depends-on ID]...
                         [--delivers ID]... [--metadata KEY=JSON]... [--asset PATH]...
                         [--classification public|private] [--assignee PERSON]
                         [--term-scope REPOSITORY... | --term-scope-empty]
onetaskgraph task render <ID> [--template FILE | --template-loader FILE] [--search-path DIR]...
                         [--answers FILE] [--var NAME=VALUE]... [--unset NAME]... [--dry-run]
                         [--asset PATH]... [--term-scope REPOSITORY... | --term-scope-empty]
# Every verb that writes — every copy, create, render, update, `set` and comment add or
# edit — takes --term-scope REPOSITORY (repeatable) or --term-scope-empty: see
# "The public boundary" below.
onetaskgraph task answers <ID>
onetaskgraph task assets <SOURCE:ID> --output-dir DIR [--json]
                                                 # write the task's image assets into DIR

onetaskgraph project list / show / deps          # the same flags, minus the project filter
onetaskgraph project list [--source S]... --origin SOURCE:ID [--json]
                                                 # the projects copied from that item
onetaskgraph project graph <ID> [--format mermaid|json] [--direction auto|td|lr]
                           [--group-by KEY] [--complete]
                                                 # see "Drawing a project's dependency graph"
onetaskgraph project copy <ID> --to <SOURCE> [--no-tasks | --member TASK-ID...]
                                                 [--documents]
                                                 [--match-by KEY] [--recreate] [--dry-run]
onetaskgraph project copy <ID> --to <SOURCE> --member TASK-ID... --members-only
                                                 # into the project already there, untouched
onetaskgraph project metadata set <ID> <KEY> <VALUE>
onetaskgraph project status set <ID> draft|backlog|todo|queued|in-progress|done|cancelled|unknown
onetaskgraph project create <SOURCE> --id NATIVE-ID --title TITLE [--status CATEGORY]
                            ...                  # the body, label, repository, metadata,
                                                 # classification and term-scope flags of
                                                 # `task create`
onetaskgraph project render <ID> ...             # the flags of `task render` but --asset
onetaskgraph project answers <ID>

onetaskgraph document list / show                # the same flags, minus --status
onetaskgraph document copy <ID>... --to <SOURCE> [--match-by KEY] [--recreate] [--dry-run]
onetaskgraph document metadata set <ID> <KEY> <VALUE> [--expect VERSION]
onetaskgraph document create <SOURCE> --project P --title TITLE [--id DOC]
                             ...                 # the body, label, repository, metadata,
                                                 # asset, classification and term-scope
                                                 # flags of `task create`
onetaskgraph document render <ID> ...            # the flags of `task render`
onetaskgraph document answers <ID>

onetaskgraph label list [--source S]...
onetaskgraph search <TEXT> [--in ...] [--kind task|project|both]

onetaskgraph template variables <FILE> [--search-path DIR]... | --template-loader FILE
onetaskgraph template render <FILE> [--search-path DIR]... | --template-loader FILE
                             [--answers FILE] [--var NAME=VALUE]...

onetaskgraph cache clear [SOURCE]                # empty one source's local cache, or all
onetaskgraph config show                         # every setting and the layer it came from
onetaskgraph schema                              # the JSON Schema bundle both SDKs use
```

An `<ID>` is qualified: `work:ENG-142`. Repeating `--label` narrows — a second one is a
second requirement — and `--not-label` excludes. `--project` takes a qualified id, which
narrows the query to that project's own source, or a bare native id, which is asked of
every selected source.

A **document** is what lives in a project and is not work — a design note, a runbook, a
page somebody has to read. It carries no status and takes part in no dependency graph, so
`document` has no `--status` filter and no `deps` verb. What it does carry is a
**location**: where a reader can actually open it, as a link (`url …`) or as an absolute
path on the machine its source runs on (`path …`). `task show`, `project show` and
`document show` all print it, and `--json` carries the contract type's own shape —
`{"url": …}` or `{"path": …}` — so a program branches on which key is present. Not every
source has documents; one that says it has none is reported as holding none rather than as
having failed, and a copy naming it is refused before anything is read.

A **comment** is added to a task after the task exists, without rewriting it — evidence
appended to an issue somebody else opened. `task comment add` and `edit` read the body from
`--body-file`, or from standard input when that flag is absent, and never from a word of the
command line, so a body quoting a command never passes through a shell; it is stored and
returned byte for byte, and an empty one is refused. `--author` is for a source that records
what it is given, such as a folder of Markdown; GitHub and Linear record the signed-in
account themselves and refuse it rather than drop it. `task show` prints a task's comments
after its body, and its `--json` carries them as a top-level `comments` list for a source
whose tasks have comments — absent, rather than empty, for one whose tasks have none.
Without `--comments`, a `task copy`, `project copy` or `document copy` never writes a comment
at either end, and reads one only for a `task copy` by query narrowed by `--commented-since`,
which reads each matched task's comments once to confirm the match; with it, a task or project copy keeps the destination's copies of each
task's comments in step with the source's — see
[Carrying comments, and copying only what changed](#carrying-comments-and-copying-only-what-changed).

`task show-many <ID>...` shows several tasks at once, whatever sources they are in, and its
`--json` is `{"details": [...]}`: one entry per id, in the order given, each exactly what
`task show <ID> --json` prints. An id that cannot be read — no such task, no such source, a
GitHub draft's comments — carries its failure in that entry's own `errors` and does not refuse
the others; the command exits non-zero exactly when some entry carries one. Each source is
asked for its ids together, so GitHub Projects reads 24 items, with their comments, per
request.

`task list --commented-since <INSTANT>` keeps the tasks **one of whose comments was created,
or last edited, at or after** the instant — an RFC 3339 time with its offset, such as
`2026-09-28T12:00:00Z`; one without an offset is refused, naming the flag. A task with no
comments never matches, and a comment deleted before the query is not a match. It combines
with every other filter, and the answer is exactly their intersection. A folder of Markdown,
an in-memory source and a GitHub Projects board apply it themselves. A board asks GitHub's
issue search, scoped by the board alone, for the issues updated since the instant, then reads
those candidates' comments and no others' — exact for new and for edited comments in every
repository the board's items live in, because GitHub moves an issue's `updatedAt` when one of
its comments is added or edited, which the board's credentialed journey re-takes on every
run. That search is an index that lags a write by a second or two, so when you ask again from
the time you last asked, overlap the two instants by more than that. Linear asks for the
issues with a comment created or last edited at or after the instant, by the same two times a
comment read reports. A source that does not apply it itself is narrowed by the engine, which
**reads that source's comments task by task** for every task the other filters kept: correct,
and as costly as that sounds on a large workspace.

`task list --metadata <KEY>[/<SEGMENT>…]=<VALUE>` keeps the tasks whose caller-defined
metadata holds the string `VALUE` at that location. `/` splits the top-level key — which may
itself contain dots, such as `orchestrator.follow-up` — from the nested object keys under it,
and the first `=` splits the location from the value, so
`--metadata orchestrator.follow-up/root_cause=stale-cache` keeps the tasks whose
`orchestrator.follow-up` object holds `root_cause: "stale-cache"`. The value there must be a
string equal to `VALUE`, case-sensitively. Repeat the flag and a task is kept only when it
holds every one. `task list --origin <SOURCE>:<ID>` keeps the tasks copied from that item:
those whose `onetaskgraph.origin` is exactly that qualified id — never one it only begins or
ends with. A folder of Markdown and an in-memory source apply both themselves; Linear asks for
the issues whose description holds each value as its metadata slot spells it and confirms every
one over the parsed slot; a source that applies neither is narrowed by the engine over each
task's own metadata.

A GitHub Projects board answers `--search`, `--metadata` and `--origin` **without listing the
board**, which is what keeps a large board affordable:

- `--search` is one board-scoped issue search for the text as a phrase, in the title, the body
  or both as `--in` says, and every candidate is confirmed by the same case-insensitive
  substring rule a folder of Markdown uses — so every task returned contains the text. GitHub
  matches whole words, so a task holding the text only inside a longer word (`ship` inside
  `Shipment`) is **not** returned: that is this source's declared behaviour, not a defect.
- `--metadata` is the same search with each value as a phrase in the body — GitHub indexes the
  metadata comment a board keeps at the end of each issue body — and every candidate is
  confirmed against that parsed comment.
- `--origin` is the board's own field filter on its `onetaskgraph.origin` field, which finds
  every carrier whichever release wrote it, together with the issue search for the id in the
  body, where a write mirrors it so a copy another process made seconds ago is found too.
  Every candidate is confirmed against the origin field, which is the one place a board's
  origin is read from.

Every answer is completed with what the same command has just written, so a task written a
moment ago is found. A task **another** process wrote moments ago may be missing until GitHub
indexes it — usually a second or two, and for a task whose origin is in the field alone, as
long as the board's own item list is behind on it, which can be minutes. A board **draft** is
not an issue and is never returned by any of the three. Labels, status, priority and project
are confirmed over the candidates, and a query carrying none of the three reads the board as
before.

`project list --search`, and `document list --search` without `--project P` — `--no-project`
included — are the same board-scoped search, each candidate confirmed by its kind and by the
substring rule, and narrow the same way: whole words only, so `ship` does not find a project
titled `Shipment plan`, and no board draft — a draft titled as a design document included — is
ever returned. A text with no letter or digit, such as `--`, is refused there before anything
is sent. `document list --project P --search` sends no search: it reads that project's
sub-issues and matches the text inside words too, as before.

`project list --origin <SOURCE>:<ID>` lists the projects whose `onetaskgraph.origin` is exactly
that qualified id — every copy of one plan, zero, one or several, in the rows and JSON
`project list` prints — and none other; which of several a caller wants is the caller's
question. A GitHub Projects board answers it with the same origin lookup `task list --origin`
sends, so what it costs is the copies it finds and never a page of the board; a source with no
narrower answer is listed whole and narrowed. `project list` without `--origin` is unchanged.

![A terminal showing one task from `task show`: an aligned block of id, title, status, project, labels and a path location, then the task's body, then its one comment with that comment's id, author and created and updated times](docs/screenshots/task-show.svg)

A task's **status** is set on its own with `task status set`, which writes that one field —
title, body, labels, metadata, dependencies and comments stay exactly as they are — and
answers with the status as the source reads it back. `queued` is the category for work
claimed by something that will do it and not yet started, between `todo` (ready, and nothing
has claimed it) and `in-progress`. A folder of Markdown reads the word `queued`; a GitHub
Projects board sends it to its `Queued` column by default, `status_mapping.queued` naming
another; Linear writes it as the name its source's `status_mapping.queued` gives the item's
kind, and refuses it by name when that source gives none. See "Status mapping" below.

<!-- llmlint: ignore[contracts_have_one_source_or_a_drift_gate] This user-facing summary is required to describe the GitHub projection; the loopback plugin tests and shared live journey drive the resolved mapping, mutations, and observed read-back together. -->
On GitHub Projects, terminal writes keep both GitHub representations aligned: `done`
selects the mapped `Done` option and closes the issue as completed, while `cancelled`
selects the mapped `Cancelled` option and closes it as not planned. Those are the shipped
option names. An open-category write reopens a closed issue before selecting its mapped
option; a draft item has no issue state to reopen. If any mapped option is absent, the
write is refused by that option's name before either representation changes —
`sources fields` (and its Status-only form, `sources status-options`) counts a terminal
category's mapped option as configured, so it names a missing `Done` or `Cancelled` and
`--apply` adds it. Reads keep
GitHub's issue decision authoritative for closed issues—completed reads `done` and not
planned reads `cancelled`, regardless of the displayed option—while the Status option
decides an open issue's category.

A task's **priority** is one of `none`, `urgent`, `high`, `medium` and `low`, and every task
carries one — `none` when none is set, and for every task of a source that cannot hold one.
`task list --priority` keeps the tasks at any of the priorities named, and `task priority set`
writes that one field and answers with the priority as the source reads it back; `none`
clears it. A copy carries a task's priority like its status. A folder of Markdown holds it as
a `priority:` key, Linear as its own issue priority, and a GitHub Projects board as the
option of a single-select `Priority` field that the source's `priority_mapping` names —
`Urgent`, `High`, `Medium` and `Low` unless it says otherwise. A board source configured
without `priority_mapping` holds no priority, and a copy or a `task priority set` that would
write one to it is refused by name before the board is asked anything. `sources fields
<SOURCE> --apply` creates the board's `Priority` field, or adds the options it lacks, without
disturbing an option or an item's value that is already there.

A task's **content** is replaced on its own with `task content set <ID> --file PATH`: the
file's bytes become the task's body, and status, priority, metadata, labels, repositories and
dependencies stay exactly as they are — on a GitHub board, whose metadata lives in the issue
body beside the content, that block is kept as it was. There is no compare-and-set: what the
file holds replaces whatever the task held. The answer — `TaskContentSet` under `--json` —
carries the `content` the task then holds: on a source that rewrites the Markdown it is written,
as Linear does, that is the content Linear stored, read off the write's own answer with no read
after it. Linear may normalize a content write's spelling — `- ` bullets as `* `, a table's
delimiter row as `| -- |`, a blank line before a list — while keeping its rendered meaning; on
every other source it is exactly the bytes sent.

One **metadata** key of a task, a project or a document is set on its own with `task`,
`project` or `document metadata set <ID> <KEY> <VALUE>`, which adds the key or replaces what
it holds and changes nothing else about the record — every other key, and every other field,
stays exactly as it was, and a key already holding the value is not written at all. `KEY` is
your own dotted `<namespace>.<name>`: two or more non-empty segments, never in the
`onetaskgraph.` namespace this product keeps in step itself. `VALUE` is exactly one JSON
value, parsed as JSON and never as YAML, so a bare `yes` or `2026-01-01` is refused rather
than given a type — quote a string as `'"text"'`. An unqualified id, a key or a value of
those shapes is refused before any source is asked. The answer — `MetadataSet` under
`--json` — carries the id, the key, the value as the source reads it back after the write,
and the record's location. Metadata is not status, so no task it delivers is re-evaluated.
A folder of Markdown edits the one entry of its `metadata:` block and no other byte, and
refuses by name a block it cannot edit that narrowly; a GitHub Projects board sends one
update of the issue body that changes only its metadata slot; the in-memory source holds the
value for the life of its process; and Linear sends one update of the issue's or the
project's description, or the document's content, that changes only the metadata slot at its
end. A source with no
write side, a record the source does not hold, and a stdio plugin whose handshake does not
declare the write are each refused by name.

Several fields of one task are **updated** together with `task update <ID>`, which writes
every field it names and nothing else: `--title`, the content from `--body-file`, `--status`
(and `--status-name`, the status's own word where the source keeps one), `--priority`, any
number of `--metadata KEY=JSON` to set and `--remove-metadata KEY` to remove, and
`--delivers` or `--depends-on` lists that replace the task's own, their `--no-…` forms
replacing them with none. A field that already holds the value named is not written, and an
update in which nothing differs writes nothing at all; the answer — `TaskUpdated` under
`--json` — says which fields were written, carries the task as its source reads it back, and
under `spent` what the source's own meter says the call cost. Labels, repositories and the
project a task is filed under are not among the fields: an existing item is never moved. On a
GitHub Projects board that is at most one read of the issue, one `updateIssue` carrying the
title, the body — visible content and metadata block together — and any state change, one
field write each for `Status` and `Priority`, and only the `blockedBy` edges that differ;
Linear sends one `issueUpdate` of what differs, and a folder of Markdown replaces the file
once, every byte it was not asked to change kept. Naming a status or a `delivers` list keeps
every delivered task in step exactly as `task status set` does, and exits `4` when one could
not be; naming neither re-evaluates nothing. A key both set and removed is refused before
anything is written, and so is an update naming no field. With `--when-priority PRIORITY`
(repeatable, `none` for a task with none) the whole update is written only while the task holds
one of them, in one write held to the version that read found; at any other priority nothing of
it is written and its `guard` answers `held` with the priority the task holds.

### Many items in one call

Three commands do one thing to many tasks of one source in one call, each answering every item
on its own; their documents are `onetaskgraph schema`'s `FindRequest` and `FoundTasks`,
`CommentBatch` and `CommentsAdded`, and `UpdateBatch` and `TasksUpdated`, read from a file or
from standard input with `-`.

- `task find <SOURCE> --queries FILE` runs many searches, `{"schema_version": 1, "queries":
  [{"key", "text", "in", "metadata", "origin", "status"}]}`, each narrowed by at least one of
  `text` (in the `title`, the `content` or `both`), `metadata` (`{"<KEY>[/<SEGMENT>…]":
  "<value>"}`) and `origin` — one narrowed by none is refused naming its key, and nothing is
  read. Each answer holds every match across every page, as `task list` reports a task; one
  whose page the source refused is answered `complete: false` with a `reason` naming the page,
  every other search goes on, and the command exits `4`.
- `task comment add-many <SOURCE> --input FILE` adds `{"schema_version": 1, "comments":
  [{"item", "body", "if_version"}]}`, answering each `written` with the comment's id, URL and
  version, `version-moved` with the version its task is at, or `error` with a reason naming it.
- `task update-many <SOURCE> --input FILE` applies `{"schema_version": 1, "updates": [{"item",
  "if_version", "title", "body", "status", "priority", "when_priority", "metadata",
  "remove_metadata", "repositories"}]}` — every member but `item` optional — each update one
  held write of every field it names: `written` with the version the task holds after it, where
  the write's own answers — or, on a GitHub board, the read right after a write no answer shows
  whole — determine one, `held` when its priority guard declined, `version-moved` with the
  version its task is at, or `error`. A GitHub board's version waits a minute for the ticket to
  settle: a write held to it before then is refused, and one held to it after goes ahead.

No item's failure writes or refuses another, and a batch exits `4` when any item is
`version-moved` or `error`. A source asks its backend for as few requests as it can: Linear
reads the tasks a batch writes in one aliased request and sends the updates, or the comments,
in one more, and runs the searches of one round of pages as one aliased request. Where Linear
refuses one alias of such a request it applies none of them, so the item it refused is sent
or answered on its own and the rest are sent together again — nothing of the refused request
landed, so no write, a comment included, is ever sent twice. A folder of Markdown and an
in-memory source answer each item with what one call for it would.

A task can name the tasks it **delivers**: finishing it finishes them. In a folder of
Markdown that is a `delivers:` list in the front matter — a bare id names a task of the same
folder and `<source>:<id>` a task anywhere — and on a GitHub board it is the
`onetaskgraph.delivers` key of the issue's metadata block. The delivered task's
`delivered_by` is onetaskgraph's to keep. Whenever it writes a task that delivers anything,
or delivered something before — a copy, `task status set` or `task update` — each delivered
task gains the deliverer's qualified id, a task it dropped loses it, and the delivered task's
status follows its deliverers while it is at `todo`, `queued` or `in-progress`:
`in-progress` while any runs, `queued` while any is queued, `done` once every one is done or
cancelled and at least one is done, and `todo` when they release it. A delivered task at `draft`, `backlog`, `unknown`,
`done` or `cancelled` is left alone, a deliverer its source no longer holds is dropped from
`delivered_by`, and a delivered task that cannot be read or written is reported failed while
the deliverer's own write stands.
A copy rewrites a `delivers` entry naming another item it copies to that item's new id,
counts those under `delivers_rewritten`, and never carries `delivered_by`. Every verb that
writes a deliverer reports each delivered task it evaluated under `delivered`. A `--dry-run`
copy writes nothing and so keeps no delivered task in step: its `delivered` list is empty,
which says nothing about whether a delivered task would have moved.

How a source *spells* a document is its own business. A GitHub Projects board has no
document type, so `github-projects` reads one as an ordinary issue whose title begins
`DESIGN: ` — the title you see has that prefix taken off, and writing a document puts it
back, so a design note copied out of a board and back returns the title it started with.
`docs/metadata.md` records the whole of that rule.

### Relations: a parent, and typed links

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys that
     drive each statement through the binary: `crates/onetaskgraph-e2e/tests/e2e/relations.rs`
     over folders of Markdown and an in-memory source, and the `relations` journeys of
     `crates/onetaskgraph-github-projects-e2e` and `crates/onetaskgraph-linear-e2e` against the
     loopback board and workspace; the shapes are `onetaskgraph schema`'s roots. -->
Two relations between tasks sit beside `depends_on`, which keeps its meaning: a task's
**parent**, many-to-one and as many levels deep as the destination allows, and **typed links**,
many-to-many, each a type name of your own over a directed pair of tasks. A source that writes
neither holds neither, and reads, writes and copies exactly as it did.

- `task parent set-many --input FILE` sets or removes parents, `{"schema_version": 1, "edges":
  [{"child", "parent", "if_version"}]}` with `parent` `null` to remove one, answering `{"schema_version":
  1, "results": [{"child", "parent", "outcome", "version", "reason"}]}` — `written`, `unchanged`,
  `version-moved`, `refused` or `error`. A parent is a task of the child's own source; one in
  another source is refused naming both, and so is a cycle, each for its own edge alone.
- `task link add-many --input FILE` and `task link remove-many --input FILE` take
  `{"schema_version": 1, "links": [{"from", "to", "type"}]}`, `from` and `to` tasks of any
  configured sources, and answer `{"schema_version": 1, "results": [{"from", "to", "type",
  "outcome", "from_version", "to_version", "reason"}]}` — `written`, `exists` for an add both ends
  already hold, `absent` for a removal neither holds, `partial` naming the one end that landed, or
  `error`. A link is its type and its ordered ends, so a repeated add writes nothing; repeating a
  call that answered `partial` completes the other end.
- `task relations <ID>... [--complete]` answers `{"schema_version": 1, "items": [{"item",
  "parent", "children", "children_complete", "links": [{"type", "direction", "other",
  "other_end"}], "links_complete"}]}`. A link's `other_end` is `present` or `missing` when the
  other task is among the ids asked and `unread` otherwise, so the comparison costs no request;
  `--complete` refuses an answer whose children were not read whole.

Each item is answered on its own, and the parent and link batches exit `4` when any is not
settled. Every update of a link's end is checked against the version this call read it at just
before it is written; on GitHub and Linear an edit landing between that read and the write can
still be overwritten, as with every held write there.

**Where a parent lives.** On a folder of Markdown it is the task's `parent:` front-matter entry,
on an in-memory source its configuration's `parents` map, on Linear the issue's own parent — a
parent in another team of the same source is allowed — and on a GitHub Projects source the
issue's sub-issue link, only when that source's `sub_issues` is `parent` (below). A re-parent on
GitHub is `addSubIssue` with `replaceParent`, and a parent in another repository of the source is
allowed. GitHub allows 8 levels — a root and 7 layers of sub-issues — and 100 sub-issues per
parent; an edge past either is refused alone with GitHub's own words and the rest of the call goes
on. Linear accepted at least 10 levels and 105 children when measured and states no limit, so the
store states none of its own and reports Linear's refusal if one comes.

**Where a link lives.** On every plugin the authority is the store's own metadata on both ends,
the reserved `onetaskgraph.links`, a list of `{"type", "direction": "out"|"in", "other":
"<source>:<id>"}`, beside a `## Links` section the store renders at the end of each end's
content, one line per link naming its type and linking the other task. Every read strips that
section, so content round-trips; a `## Links` heading followed by anything but bullets is a
person's and is left alone, and Linear's `*` bullets and changed blank lines are still the
section. Every write of a linked task's content — `task content set`, `task update`, a render,
a copy over it — re-renders the section from the links the task holds, keeping each line it
already showed; a copy over a Linear issue, which has no stored text at hand to keep a line
from, writes each line from the link itself. All of one task's links in a call go in one
write. A copy carries neither a parent nor
links, and never the `onetaskgraph.links` key.

**A native mirror is opt-in per type.** A source's `link_types` maps a type to the destination's
own relation, so the link also shows in that tracker's relationship panel:

```yaml
sources:
  issues:
    plugin: linear
    config:
      team: TES2
      link_types:
        caused-by: {native: related}
        duplicate-of: {}          # metadata and the `## Links` section only
```

`relates-to` on GitHub (`addRelatesTo`) and `related` on Linear (`issueRelationCreate`) are the
only words. Neither destination stores a type name and both relations are symmetric, so the type
and the direction live only in the metadata, and several mirrored links between one pair share
one native relation, removed only with the last of them. On Linear that relation is the same
kind a `related` dependency is stored as, so a task's own mirrored link is left out of its
`task deps` and never removed by a `depends_on` rewrite. `blocks`, `blocked-by` and every other
word are refused when the configuration loads, naming the source, the type and the word, because
`depends_on` already means blocking. **A duplicate link has no native mirror**: `duplicate` and
`similar` are refused too, because a native duplicate closes the issue on GitHub and moves it to
`Duplicate` on Linear. A folder of Markdown and an in-memory source have no relation of their
own, so any `native` word there is refused. A link whose ends are in two sources is metadata on
both ends and is mirrored by neither.

**What a person sees of a link with no mirror** is the `## Links` section alone: nothing appears
in GitHub's Relationships panel or Linear's related issues. A mirrored link shows there too, as
`Relates to` on GitHub and a related issue on Linear, with the type name in the section.

**A whole-body rewrite loses that end's links.** On GitHub and Linear a task's metadata rides in
its body, so a person replacing the body whole removes that end's links while the other end still
holds them. A read writes nothing about it: `task relations` of both ends answers the survivor's
link `other_end: missing`. Repeating `task link add-many` for it restores the lost end and answers
`written`; `task link remove-many` removes the end that remains.

On GitHub one relation mutation document carries at most 30 aliases — a 60-alias `addRelatesTo`
was measured answering HTTP 504 after landing 58 of them — and after such an answer the store
reads the document's tasks back and answers each from what it finds rather than sending it again.
Every aliased relation write is charged to the source's pacing as one content creation, because
GitHub's secondary limit, not its point budget, is what sustained relation writes reach.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### Assignees: one person, or none

A source holds assignees only once its configuration opts in, on every plugin:

```yaml
sources:
  board:
    plugin: github-projects
    config:
      assignees: true   # default false
```

Off — the default — nothing changes: no read asks the destination for assignees, no item carries
an `assignees` key, and every write naming an assignee is refused before anything is read or
written, naming the source and the setting (`no-assignees`).

**Reading.** An opted-in source's tasks carry `assignees: [<person>, ...]` in every shape the
store answers — `task show`, `task show-many`, `task list`, `task find` — holding **every**
person the item is assigned to, `[]` when nobody. A person is named as the destination names
one:

| Destination | A person is | Notes |
|---|---|---|
| `github-projects` | the user's **login** | An issue may hold several, and all of them are read. |
| `linear` | the user's **display name** (their @handle) | Unique within a workspace; resolved to the user once per process, in one request for every name a call needs. |
| `local-md` | the string as written, in the task's `assignees:` front-matter list | A file a person edited to hold several reads back with all of them. |
| `in-memory` | the string as given | |

A task's change token covers its assignees, so a cached read pinned to a token never serves
assignees older than it.

**Writing: one person or none.** Every write names at most one person and **replaces** whatever
the item holds — on GitHub every earlier login is removed, on Linear the assignee is set or
cleared:

```bash
onetaskgraph task create board --project P --title "Fix it" --assignee octocat   # at most once
onetaskgraph task assignee set board:I_kw... hubot                  # replaces every assignee
onetaskgraph task assignee set board:I_kw...                        # naming none: unassign
onetaskgraph task assignee set board:I_kw... hubot --if-version V   # held to the version read
```

A person the destination will not assign is refused for that item, naming them, while the rest of
the call goes on. GitHub assigns only people who can be assigned to issues in the issue's
repository, and answers a write that names anyone else by leaving them off; so each write's answer
is compared with what it asked for rather than trusted. On Linear an unknown display name is
refused.

**Guards: a person's own assignment stands.** A guard names a *set*, because a read can hold
several people, and a write lands only while the item holds exactly the people named:

- `task update-many`: an update's `"assignee": "<person>"` assigns, `"assignee": null`
  unassigns, and leaving it out leaves the assignees as they are. `"when_assignees":
  [<person>...]` writes the update only while the task holds exactly those people (`[]`: only
  while it is unassigned); otherwise nothing of it is written and it answers `held`. An item a
  person gave a second assignee therefore holds a write guarded on the first alone.
- `task copy ... --when-destination-assignees <PERSON|none>` (repeat for each person; `none`
  alone for nobody): a copy between two opted-in sources gives a **created** item the source
  item's assignee, and writes it over an **existing** item only beside this flag and only while
  that item holds exactly the set named — keeping the item's own otherwise. Each updated item's
  outcome carries `assignee_condition: {"held": [<person>...], "met": <bool>}`. Decided on the
  copy's own read, so it adds no request, as `--when-destination-priority` is. A source item
  holding more than one person is refused for that item, naming it and how many it holds, the
  call's other items going on. A copy from or to a source that has not opted in carries no
  assignee and is not refused for it.

An assignee write rides the documents the call already sends — `assigneeId` in Linear's
`issueCreate` or `issueUpdate` input, `assigneeIds` in GitHub's `createIssue` or
`updateIssue` — and costs at most one person-lookup request per call, for every person the call
names that the process has not resolved yet (a GitHub login to its node id, a Linear display name
to its user id), and none for a person already resolved or for unassigning. A call is split into
more than one lookup only where one request would exceed the destination's own documented limit:

| Destination | One lookup names at most | Why |
|---|---|---|
| `github-projects` | 500,000 logins | GitHub's limit of 500,000 nodes a request; each login is one `user(login:)` node |
| `linear` | 8,250 display names | 33 `users` connections of 250 (Linear's connection maximum), each at most 302 of the 10,000 complexity points Linear allows a query |

Within one call a person its lookup resolved is not looked up again, even after a write naming
them failed or was answered with other people than it asked for; a later call looks such a person
up afresh, so no write sends an id for anybody but the person it was resolved for.

A create naming no assignee asks the destination to assign nobody; a Linear team whose template
or triage rule assigns a default still does, and the task reads back with that person.

### Seeing which plan you got

`--explain` renders the plan the query ran, per source. Here one `--label` query reaches
two sources of differing capability — a folder of Markdown, and a source that declares it
cannot filter by label at all:

![A terminal showing two task rows above a plan block: the source that declares it cannot filter by label is listed with `applied locally: label`, and the folder of Markdown with `pushed down: label`, each with the number of pages it served](docs/screenshots/task-list-explain.svg)

The folder filtered the rows itself; the other source returned the wider set and the engine
narrowed it. One query, two plans, and the same correct answer either way — which is what
the capability declaration above buys you and why it is worth reading. `--json` carries the
same plan as a field, so a script does not have to parse the prose.

### Drawing a project's dependency graph

`project graph <ID>` prints one project's tasks and the dependency edges between them as
Mermaid flowchart text, for whatever renders Mermaid — it draws no picture itself — and
`--format json` prints the same graph keyed by task id, for a program. A project of six
tasks, one of which waits on a task of another project, prints:

```text
flowchart TD
  n1["Lay the foundation"]
  n2["Plumb"]
  n3["Raise walls"]
  n4["Wire #quot;main#quot; #35;2 #lt;panel#gt; and #gt;sockets"]
  n5["Roof"]
  n6["Paint"]
  x1["Get the permit (plans:permit)"]:::external
  n1 --> n2
  n1 --> n3
  n1 --> n4
  n2 --> n6
  n3 --> n5
  n4 --> n5
  n5 --> n6
  x1 --> n5
  classDef external stroke-dasharray: 5 5
```

The bytes are a contract, stated once, in the verb's own `--help`, and reproduced here
exactly as `onetaskgraph project graph --help` prints it. The journeys in
`crates/onetaskgraph-e2e/tests/e2e/graph.rs` hold the binary to it and this copy to that
text, so neither can drift from the other:

```text
The graph:
  Every task of the project is drawn, across every page its source answers, with each
  dependency a project task has on another task. A task's rank is 0 when it depends on no
  task of the project, and otherwise one more than the highest rank among its prerequisites
  in the project; tasks outside the project have no rank. The number of ranks is one more
  than the highest rank, or 0 for a project with no tasks, and the widest rank is the most
  project tasks sharing one rank.

Direction:
  auto (the default) lays the graph out left to right (lr) when the widest rank holds more
  tasks than there are ranks, and top-down (td) otherwise; td and lr are taken as given.

Mermaid form (the default), UTF-8, every line ending in a newline:
  1. `flowchart TD` or `flowchart LR`, after auto is resolved.
  2. One line `  n<k>["<label>"]` per task of the project, in topological order: each
     after every task of the project it depends on, ties broken by title and then by
     qualified id, each compared byte by byte. <k> counts from 1 in that order.
  3. One line `  x<k>["<label> (<qualified id>)"]:::external` per task outside the
     project that a project task depends on, ordered by title and then qualified id, <k>
     counting from 1. The qualified id there is escaped as a label is.
  4. One line `  <prerequisite> --> <dependent>` per dependency, so an arrow points from the
     task depended on to the task that depends on it, ordered by the prerequisite's position
     and then the dependent's, every n node before every x node.
  5. Last, only when an x node was printed: `  classDef external stroke-dasharray: 5 5`.
  A project with no tasks prints the first line alone, which auto makes `flowchart TD`.

Grouped (--group-by <KEY>):
  A project task's group is the value its metadata holds under KEY when that is a non-empty
  JSON string. A task without the key, or holding null or "" under it, has no group; any
  other value is refused, naming the task's qualified id and the key, with nothing on
  standard output. A task outside the project never has a group. Groups are ordered by the
  position of their first member and numbered g<j> from 1 in that order. Grouping changes
  no rank, no direction, no node number and no edge order. The Mermaid lines are
  rearranged:
  1. The first line, as above.
  2. For each group in order: `  subgraph g<j>["<label of the group's value>"]`; each
     member's node line indented by four spaces, in <k> order; each edge whose two ends are
     both members of this group, indented by four spaces, in edge order; then `  end`.
  3. The node line of each project task with no group, in <k> order.
  4. The x node lines.
  5. Every remaining edge, in edge order.
  6. The classDef line, as above.

Labels:
  A task's label is its title with each line break (\r\n, \n or \r) written as one space;
  then, in this order, # is written #35;, " is written #quot;, < is written #lt; and > is
  written #gt;. Nothing else is altered. A group's label is its value escaped the same way,
  and the qualified id in an x line is escaped the same way too.

JSON form (--format json): one document and a newline, describing exactly the graph the
Mermaid form draws, and the form to read when a program has to know which task a node is:
  {
    "schema_version": 1,
    "project": "<qualified project id>",
    "direction": "td" | "lr",
    "group_by": "<the --group-by key>" | null,
    "nodes": [
      {"key": "n1", "id": "<qualified task id>", "title": "<the title, unescaped>",
       "external": false, "group": "<the task's group>" | null}
    ],
    "edges": [
      {"from": "<qualified id of the prerequisite>", "to": "<qualified id of the dependent>"}
    ],
    "unanswered": [{"source": "<source name>", "error": {...}}]
  }
  direction is the resolved direction, never auto. nodes holds every n node and then every
  x node, in the Mermaid form's order; key is the Mermaid node id, external is true exactly
  for the x nodes, and group is null for a task with no group, for every external task, and
  for every task without --group-by. edges follows the Mermaid form's edge order.
  unanswered is present only when the graph is incomplete, as below; each entry has the
  shape of an `errors` entry of any other --json answer.

Determinism:
  The output is a function of the project's task titles, their dependency edges, the
  qualified ids of the tasks outside it, the direction and, grouped, the key and each
  task's value under it. Two calls over an unchanged project print the same bytes.

Failure:
  An id naming no project, a source that cannot answer a read, a dependency naming a task
  its source does not hold, and tasks that depend on each other in a cycle each exit
  non-zero with nothing on standard output and the problem on standard error; a graph is
  never printed in part. A source declared private that could not be asked what it records
  of the project's tasks - its credential absent or refused, or it unreachable or
  rate-limited - is not a failed read, because none of the project's tasks lives there: the
  graph is drawn from every source that holds them, that source and why are named in
  unanswered and as a notice on standard error, and the exit is 0. --complete counts it as a
  failed read instead. Anything else such a source answers with - a record that does not
  say what it names, a refusal, a configuration it reports wrong - is a failed read either
  way.
```

### Writing tasks: Markdown in, ticket out

Authoring against a ticketing API is not something a person or an agent does well, and a
folder of Markdown files is. So that is how work is created here: write the files, read
them back through the CLI to be sure they parse, then copy them where your team works.

With `notes` configured as a `local-md` source rooted at `./notes`, create its task folder
and write `notes/tasks/rate-limit.md`:

```markdown
---
title: Rate-limit the sync loop
status: Todo
labels: [{id: local-reliability, name: reliability}]
metadata: {onepipeline.turn_budget: 12}
repositories: [github.com/acme/sync]
---
Back off when the API asks us to slow down.
```

Read it through the CLI before writing to the permanent `linear` destination, then copy
the qualified id the read returned:

```console
$ onetaskgraph task list --source notes
notes:rate-limit  todo  Rate-limit the sync loop
$ onetaskgraph task copy notes:rate-limit --to linear
```

The read is intentional: a malformed front matter block caught in a local file is cheaper
to fix than a parse failure first discovered while writing to somebody's ticketing system.
Projects use the same flow under `notes/projects/`, followed by `project list` and
`project copy`.

Editing is the same road in reverse — copy out, edit, copy back:

```console
$ onetaskgraph task copy work:T-1 --to notes
$ grep -A2 '^metadata:' notes/tasks/T-1.md
metadata:
  onetaskgraph.origin: work:T-1
$ $EDITOR notes/tasks/T-1.md
$ onetaskgraph task copy notes:T-1 --to work
```

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys
     in `crates/onetaskgraph-e2e/tests/e2e/copy_link.rs`: one fails unless the link example below is
     the link a copy of `T-1` into `notes` really records, and another fails unless the rule for
     a link naming nothing names the failure kind the binary really reports. -->
The copy back **updates** rather than duplicating because the copied file carries the id
it came from, under the reserved metadata key `onetaskgraph.origin`. The item it was copied
*from* records where it landed too, under `onetaskgraph.copies` — an object naming the
counterpart at each destination it was copied into, such as
`{"notes": "notes:T-1"}`. Nothing anywhere holds a mapping: the correspondence lives on the
two items, inside the plugins that own them.

These rules find the counterpart, in this order:

0. **The explicit task.** For `task copy` with one source id, `--target-task <QUALIFIED-ID>`
   selects an existing task in the destination whose origin names the source task. A missing
   task, a task in another source, or a different origin is refused without writing. The
   report says `via: target-task`. This option is refused beside `--create`, `--match-by`
   or `--recreate`, or with several source ids, before any store request.
1. **The link.** If the item's `onetaskgraph.copies` names an item at the destination, that
   item is read by id, and when its own origin names the item being copied it is the
   counterpart and the copy updates it — one read, and no search of the destination. If it
   names an item the destination no longer holds, the copy refuses with the failure kind
   `stale-link`, naming both ids, because creating there would duplicate work somebody
   deleted; `--recreate` goes on to the rules below instead, and then creates. If it names an
   item whose origin somebody re-pointed at something else, the link is ignored.
2. **The origin.** If the item's origin names the destination, that origin *is* the
   destination item and the copy updates it.
3. **The search.** Otherwise the destination is asked for the task whose origin is the id
   being copied — the same question `task list --origin` asks, which a source that can
   answers without listing everything it holds; found, it is updated, and not found, one is
   created carrying that origin.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

Once the whole copy has landed, every item found by `--target-task`, the link, the search or `--match-by`, or
created, has its link for that destination recorded or refreshed — one metadata write at its
own source, undone with everything else if the copy cannot finish. An item found by its own
origin records nothing: that correspondence is already written down, on the destination
item. A source that cannot hold the link — it has no write side, or will not make the narrow
metadata write — is copied from exactly as before, and the report says the link was not
recorded. So after one copy the pair is found from either side by one read by id: forward
by the link, back by the origin.

Which rule found it decides what the copy records there. A copy that got its counterpart
from the first rule is a copy **back**: the destination is the original, and the item being
copied is the one that came out of it — so the destination keeps the origin it already
holds, holding none included. Stamping it with the id of its own copy would destroy the
original's provenance, and the next ordinary copy from the store it was authored in would
match nothing and create a second item beside it. Every other copy records the id it was
copied from, which is what makes the next copy of it an update.

| Flag | What it is for |
| --- | --- |
| `--dry-run` | Every read, no write, and the action each item would have got — beside, for a destination that can say, the size of the body each item would store there (`body_size`). |
| `--recreate` | An origin or a link naming an item the destination no longer holds refuses by default, because creating there would duplicate work somebody deleted. This says create instead. |
| `--create` | `task copy` only: you assert the destination holds no counterpart of any task named — typically because you have just asked it — so each is created without the correspondence lookup, and a GitHub board is sent no origin query. Sound because that query reads an index that lags a fresh write by the same amount whoever asks, so repeating yours could find nothing yours did not. Refused beside `--match-by` or `--recreate`, and for a task whose own link or origin already names an item at the destination, naming that item. |
| `--match-by KEY` | Delete or corrupt the origin key and neither rule can find the counterpart, so the next copy back creates a new item. This re-establishes the lost correspondence by matching on `title`, or on a metadata key of your choosing, without hand-editing ids. |
| `--status-policy source\|preserve-destination` | Task and project copies default to `source`, carrying the source status. With `preserve-destination`, existing destination tasks and project items keep their status and open/closed state while other changed fields are copied. New items take the source status under either policy. Documents have no status. |
| `--when-destination-status CATEGORY` | `task copy` only, repeatable: write over an existing destination task only while it is at one of these categories. Decided on the copy's own read of the destination task — from the cache under `--expect` — so it adds no request; a create is unaffected. A task at another category is handled by `--otherwise`. Each updated task's outcome carries `status_condition: {"held": <category>, "met": <bool>}`. Refused before anything is read beside `--status-policy preserve-destination`, which writes no status to guard. |
| `--otherwise refuse\|preserve-destination` | With `--when-destination-status`, and refused without it: what a task at another category gets. `refuse`, the default, writes nothing for it and answers it `{"action": "failed", "error": {"kind": "destination-status", …}, "held": …}` — naming the item, its category and the set — while every other item lands; the copy then exits `4`, with the report on standard output, as for any item left unwritten. `preserve-destination` copies it as `--status-policy preserve-destination` would, keeping its status. |
| `--when-destination-priority PRIORITY` | `task copy` only, repeatable: write the source's priority over an existing destination task only while that task is at one of these priorities (`none`, `urgent`, `high`, `medium`, `low`); at any other it keeps its own — a priority a person set — and every other field is written as without the flag. Nothing is refused for it. Taken beside either `--status-policy` and beside `--when-destination-status`; a task a status condition refuses is written nothing. Decided on the same read, so it adds no request; a create takes the source's priority. Each updated task's outcome carries `priority_condition: {"held": <priority>, "met": <bool>}`. A later copy of the unchanged source without the flag writes the source's priority. |
| `--when-destination-assignees PERSON\|none` | `task copy` only, repeatable: write the source's assignee over an existing destination task only while that task is assigned to exactly these people (`none` alone: to nobody); holding any other set it keeps its own, and every other field is written as without the flag. Without it a copy never writes an assignee over an existing task. Both sources must opt in with `config.assignees: true`; one that has not carries no assignee. Decided on the same read, so it adds no request; a create takes the source's assignee, and a source task holding several people is refused for that task. Each updated task's outcome carries `assignee_condition: {"held": [<person>...], "met": <bool>}`. See "Assignees: one person, or none". |
| `--no-tasks` | Copy a project on its own. By default `project copy` copies the project and every task in it — a home's members' tasks included — matching each task independently. |
| `--documents` | `project copy` only: copy the documents filed under the project too, in the same copy — read, put to the write policy and sized together with the project and its tasks before anything is written, and landed after them, filed under the project's copy, each reference to a task of the plan pointed at that task's copy. Without it `project copy` reads and writes no document, exactly as before, and `document copy` carries them. Refused beside `--member`. |
| `--comments` | `task copy` and `project copy`: keep the destination's copies of each task's comments in step with the source's, as [below](#carrying-comments-and-copying-only-what-changed). Without it a copy reads and writes no comment. |
| `--no-link` | Write nothing at all to the source — no `onetaskgraph.copies` link — and report every link `skipped`. The next copy finds each counterpart by the destination's own `onetaskgraph.origin`. |
| `--source S`, `--commented-since RFC3339`, `--metadata KEY=VALUE` | `task copy` only, in place of ids: copy what `task list` with the same narrowing answers. Refused beside ids. |
| `--status CATEGORY` | `task copy` only, repeatable: keep only the tasks at these categories, as `task list --status` keeps them. In a query a task at another category is not read for its comments, not copied and not named in the answer — and on a GitHub board, a status filter naming only open categories (any but `done`, `cancelled` and `unknown`, which a closed issue reads as when closed for a reason that implies neither) narrows the board-scoped issue search with `is:open`, each candidate's category still confirmed from its own item. Beside ids, an id whose task is at another category is read only as far as its status and answered `{"source": …, "action": "skipped", "reason": "status <category> is not one of <categories>"}`, neither copied nor read for its comments, without changing the exit code. The comment snapshot a host keeps is this shape: `task copy --source board --commented-since <instant> --status backlog --to snapshot`, into a folder configured with the board's own status names. |
| `--member TASK-ID` | Copy the project and exactly the tasks named, repeating the flag for each, when you know which of them changed. A task not named is not read at the destination, not written and not reported, so a one-task change costs what one task costs rather than a read of the whole project. A named task the destination does not hold yet is still created. An edge to a task not named is written to the destination id that task records at `onetaskgraph.origin`, and a copy whose edge names a task recording none is refused before anything is written, naming that task. A copy naming members was not told about the rest, so it reports nothing `orphaned`. |
| `--members-only` | With `--member`, and refused without one: copy the named tasks into the project the destination already holds, and leave that project exactly as it is. It is found by the `onetaskgraph.copies` link the source project records, then by the origin it records, then by asking the destination for the project recording it as its origin — never read to be compared, never written, and not reported, so adding one task to a board's project costs that task alone and cannot overwrite the project's title, body, labels or fields. A project the destination does not hold yet is refused before anything is written, naming the `project copy` that creates it. Everything else is `--member`'s: the same edges, the same `delivered` and `via`, and nothing `orphaned`. |

`--dry-run` is how you read that table's first row before trusting it: every source is
read, nothing is written, and each item is reported with the action it would have got.

**An item too large for its destination is found before anything is written.** A copy asks the
destination's plugin how large a body each item it plans would store there — its content
composed as the write composes it: the metadata slot the destination keeps beside the content,
the record the copy leaves, and every image reference rewritten to where the destination serves
it — after the write policy has passed them and before the first write, an intent at the source
included. A GitHub board holds at most 65,536 characters in one issue body; a Linear source
states no limit. A copy holding any item over its destination's limit — a project's tasks and
documents among them — writes nothing at all and fails as `oversize`, its failure document
listing every such item under `oversize` as `{source, destination, body_size, limit}`. A dry run
reports `body_size` on every item it sized and answers an item over the limit as
`{"action": "oversize", "limit": …}`, exiting `4`. A destination that cannot say — a stdio
plugin, a folder of Markdown — is sized not at all, and copies as it did.

![A terminal showing a dry-run copy of two tasks into another source: the first names its counterpart there and reads `updated`, the second has none and reads `created`, followed by the line counting rewritten, unresolved and ambiguous references](docs/screenshots/task-copy-dry-run.svg)

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journey
     `every_source_kind_can_be_copied_into_a_folder_of_markdown_with_its_fields_intact`,
     which asserts the written fields equal and `url` and `key` absent on every copy, and by
     `a_re_copy_keeps_a_key_only_the_destination_holds_and_changes_nothing_else` beside it,
     which holds the destination's own key through an unchanged and an updating re-copy. -->
Every field a copy read is written — title, content, status, labels, project,
repositories, metadata and the edges — except `url`, `location`, `key`, `created_at` and
`updated_at`, which are the destination's own: the short handle a backend shows people is
issued by that backend, so the one the source wore is not the one the destination does. Nothing is silently dropped: a field the destination cannot
represent, or a metadata key it cannot carry, refuses the write and names it. A copy never
deletes work either, so a destination item the source no longer holds is left exactly as it
is and reported as `orphaned` — and a copy over an item the destination already holds never
deletes a caller's metadata key that item holds: a key only the destination holds stays, a
key both hold takes the source's value, and a re-copy that changes nothing else is
`unchanged`. Which reserved keys are whose, and the two an update drops when they no longer
describe the item, are in [`docs/metadata.md`](docs/metadata.md).
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-e2e/tests/e2e/copy_per_item.rs`, which drive each kind of task a folder
     cannot represent, the order the checks arrive in at a stdio plugin, a refusal at the write and
     one of another kind, and a project copy. -->
**A task copy carries every task past one its destination cannot represent.** Before anything
is written, the copy asks the destination about each task it plans — its status, its content,
the tasks it delivers and every other field a write of it would refuse as "cannot represent
the field …". A task the destination refuses is answered
`{"source": …, "action": "failed", "error": {"class": "refused", …, "message": <the
destination's reason>}}` and left out; an edge or a `delivers` entry of another task naming it is
written the way one naming an item outside the copy is — by its qualified id where it is, and
never as a copy that is not there; and every other task lands as one copy, all or nothing among
themselves. The command exits `4`, as for any failed item. A source answers that question
through `check_task_write` ([`docs/plugin-protocol.md`](docs/plugin-protocol.md) §4.9b); a folder
of Markdown does, and a source that does not is read as able to represent everything, refusing
in its write instead. **`project copy` is unchanged**: a project and its tasks still copy whole
or not at all, representability included.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

**Otherwise a copy either completes or leaves the destination as it found it.** A copy that
cannot finish — a write the destination refuses after accepting the task at the check, a
credential that expires, a rate limiter — undoes what it has already written and takes back
the items it created in that run, exiting `1`, so the retry starts from the destination you
started from. That is what stops a half-written
project having to be re-run, and the re-run is the burst of writes that trips a hosted
destination's rate limiter. When the destination will not take one of them back, the
refusal says so and names what is still there rather than leaving you to find it. A
destination that stores an item and then answers its create in a shape the engine refuses
has kept it under an id the copy was never told, so that one item cannot be taken back: the
copy undoes everything else, names the destination and the method whose answer was malformed,
and tells you to inspect that destination before copying again, rather than to copy again and
land a second one beside it.

### A create safe to retry

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-e2e/tests/e2e/copy_retry.rs`, which stop a copy at each point named
     here and run it again against a second folder, the loopback board and the loopback Linear
     workspace, by `crates/onetaskgraph-e2e/tests/e2e/release_requests.rs`, which holds the
     request figures to the v0.12.13 binary's, and by
     `crates/onetaskgraph-core/src/engine/copy/intent.rs`, the one place the key and the lease
     are spelled, and `crates/onetaskgraph-local-md/src/hold.rs`, the one place the lock is. -->
**Every create a copy makes is safe to retry** — of a task, a project and a document alike —
with no flag, but for the two named below: a copy that dies or times out after the destination
created its item, run again, finds and finishes that item rather than creating a second. Two
reserved keys carry it, neither of which `metadata set` accepts:

- **`onetaskgraph.creating`**, on the item being copied, is an object keyed by destination
  source name, each `{"key": <UUID v5>, "at": <RFC 3339 UTC, when the key was first taken>,
  "attempt": <UUID v4 of the copy holding it>, "lease_until": <RFC 3339 UTC>}`. It is written
  after the copy has planned — read what it needs of the destination — and before its first
  destination write, before any create of the same command is sent — and removed for that destination with the write that records the item's
  `onetaskgraph.copies` link: in that one write for a task, and in one more narrow write of that
  key alone for a project or a document; the key goes when its object empties. A dry run, `--no-link`, an
  update and a copy into a destination a link may not name write none, so an item with no create
  in flight reads exactly as it did before this existed. A source that will not keep the key — a
  narrow write it refuses, a record it no longer holds — is copied as it always was, with no
  intent. An answer to that write the copy cannot read — a plugin refusing the key as malformed
  parameters, a backend whose answer was garbled — says nothing of whether it wrote, so the item
  is read once more, still under its hold: an intent naming this attempt there is held and settled
  as any other, an item holding none there or no longer held copies with no intent, and a read
  that fails, an entry that is not an intent and another attempt's intent each refuse before any
  destination write. One whose hold or read of the item fails refuses the copy before any destination
  write. A copy that cannot finish and takes back everything it wrote puts the item back byte
  for byte, or, where it cannot, takes the intent alone off — an item that takes neither keeps it
  for its retry to resolve once the lease runs out; one whose create may have landed unseen — an
  answer that never arrived — keeps the intent and gives up its lease, so its retry takes over at
  once.
- **`onetaskgraph.create_key`**, on the created item, is the key as a string, written with the
  create itself: in a GitHub issue body's metadata slot, in a folder of Markdown's file, and on
  Linear in its slot and, transformed, as the item's own `id` — sent as `IssueCreateInput.id`,
  `ProjectCreateInput.id` or `DocumentCreateInput.id`.

**Two creates are outside it, and behave exactly as at v0.12.13**, recording no intent:

- **A `--no-link` create.** Its whole promise is to write nothing to the source, and the intent is
  a write to the source. Without one, its retry cannot be told from a deliberate second unlinked
  copy of the same item — which is a thing a caller may want — so a second item is what a second
  `--no-link` copy makes. It still resolves an intent another copy left, without writing.
- **A create from a source that cannot store `onetaskgraph.creating`** — one with no write side,
  one that refuses the narrow write of that key (a stdio plugin built before the key existed
  refuses it as malformed, and is read once to confirm it holds none), or one that no longer
  holds the item. The intent
  has nowhere to live, and an intent kept anywhere else would be work data outside its plugin, so
  the create is made as it always was.

**Linear is sent a v4-shaped id, not the key.** Linear's published schema
([`packages/sdk/src/schema.graphql`](https://github.com/linear/linear/blob/8397645/packages/sdk/src/schema.graphql)
at commit `8397645`) documents each of those three `id` inputs as "The identifier in UUID v4
format. If none is provided, the backend will generate one." The key is a version-5 UUID, so
the id sent is the key with its version nibble — the first digit of its third group — set to
`4`, every other digit, the RFC 4122 variant included, as it was:
`8213b96a-ad84-54be-990c-798d8c12c97b` is sent as `8213b96a-ad84-44be-990c-798d8c12c97b`. The
transform is deterministic, so every attempt sends and looks up one id; the slot keeps the key
verbatim. What Linear answers a create naming an id it already holds is not documented, so the
store reads the item by that id after *any* failed create carrying a key — an error, or an answer
lost on the way — and writes it whole when it is there. A keyed create Linear answers under any
other id, or under none, is refused as an answer the copy cannot vouch for, naming the id Linear
gave, and the destination is to be inspected before copying again: the refusal says so, and it
does not by itself keep a later retry from creating again, since that retry looks the item up by
the id it sent and an item Linear stored under another is not found there.

**The key is deterministic**: the version-5 UUID, under the namespace
`7a1c3f0e-5b2d-4c8a-9e61-0f4d2b7c5a39`, of `<source qualified id>|<destination source
name>|<the qualified id of the link this create replaces, or nothing>` for a task, with
`|project` or `|document` appended for a project or a document, so a task and a project one
folder spells with one id never share a key. Two attempts at one create compute one key without
having seen each other, and `--recreate` after a stale link gets a new one.

**An intent is resolved before every other rule**, and before `--create` is believed. The copy
asks the destination for the item carrying the key: a folder of Markdown reads its own files, and
Linear reads the item whose id is the key's v4 transform — Linear lets one item hold an id, so a
create re-sent with it while the first is still in flight lands once, and one refused is read by
that id and written whole. A GitHub board offers no idempotency key, so the store
lists the repository the creation rule files in through `Repository.issues`, newest first, back
to five minutes before the intent's `at`, and matches the key in each body's slot — never through
search. A listing page the board answers malformed — `nodes` not a list, an issue with no `id` or
`createdAt`, a body neither text nor `null`, no `hasNextPage`, or a next page whose cursor is
missing or does not move — refuses the copy before anything is created again, since an issue it
could not read might be the one carrying the key. Found, that item is the counterpart, reported `via: "intent"`: the copy writes only what
the earlier attempt left unwritten — nothing, for an item it finished — records the link and
removes the intent. On a board, an item not listed before `create_quiescence_seconds` have
passed since `at` is waited for: the copy renews its lease once, to the window's end plus
`create_lease_seconds`, waits out the window, and lists once more — a `--no-link` copy waits
without that renewal, writing nothing to its source. Still not listed, it creates with the same
key. `task locate` resolves an intent the same way but never waits, creates or
writes: it answers the item the key names as `"via": "intent"`, or no counterpart.

**Who owns an intent depends on the copy's source**, through that source's own plugin: an
intent is decided under the hold `TaskSource::hold_item` gives, and a copy that takes everything
back puts the item back through `item_snapshot` and `restore_item` — defaulted, additive methods
a source with no hold or no byte-exact snapshot leaves alone. A folder of Markdown takes, renews
and releases it under an exclusive lock on a file beside the item, `<file>.lock`, which the
kernel holds: an `flock` on Unix, and on Windows the file opened shared with nobody and deleted
as it closes. Two live processes on one host never hold it at once, and a holder that exits or
is killed releases it with no wait — its file is taken at once, whatever it is dated, and
nothing is judged by its age. Only where the filesystem answers `flock` as unsupported does the
lock fall back to creating the file exclusively (`O_EXCL`) — any other failure of `flock` refuses
the copy, the lock file left as it is — and that fallback is weaker: one older than thirty
seconds is taken to be a dead process's and taken over — so a holder slower than that can lose
it — and one dated ahead of this host's clock, whose age cannot be told, is refused at once.
Under the lock the copy
reads the intent, refuses with the failure kind `create-in-progress` — naming the lease's end,
having written nothing anywhere — while another attempt's lease is live,
and otherwise records its own `attempt` and a `lease_until` of now plus the source's
`create_lease_seconds`; a copy that finds, under the lock, that another copy created and
linked the counterpart since it planned the create refuses the same way, naming that
counterpart. So on one host exactly one attempt owns a create; the lock excludes
nothing across hosts, or on a network filesystem that does not honour the lock. A
hosted source — a board or Linear as the copy's source — has no lock to offer, so its lease is
advisory: a live one still refuses, but two attempts that read the item before either wrote both
proceed under one key. Linear then still creates one issue. A board may create two, which is
detected where the listing can see it: every board create made from a hosted source, and every
create that resolves an earlier attempt's intent, lists the repository by key once more, and an
attempt that finds more than one issue carrying it answers that item `failed` with the kind
`create-duplicated`, naming them all and recording no link. The attempt whose intent the other
overwrote finds that at the first write it asks to send after its `createIssue`, and stops there
with `create-in-progress`, its issue left as `createIssue` made it. What this guarantees is at most one
link, and every attempt either linking or refusing with what it saw — not that only one attempt
creates.

**The GitHub residual, as it is.** Safety on a board rests on GitHub ending a request within its
documented ten seconds, and on `Repository.issues` listing a committed issue within the
quiescence window. A duplicate is detected, as `create-duplicated`, only when the other issue is
listed by the time of the listing that follows a create. An issue that becomes listed after that
— a retry's predecessor outliving the window, or a simultaneous attempt from a hosted source
whose create is listed late — leaves a second issue that no process reports.

**What it costs.** The renewal rule is one statement: a holder renews its lease before each
destination write it sends after its create request — a write that finishes the item, a comment
it carries, and a write that finishes an earlier attempt's create alike — never before a read or a
listing, and once before waiting out the quiescence window, to the window's end plus
`create_lease_seconds`, so no renewal falls during the wait. That holds inside a plugin call too:
the engine runs every call it makes to a destination while it owns a create there under a
`WriteGuard` — an additive part of the plugin interface, which a source that never asks it leaves
exactly as it was. A board asks it immediately before sending each write request but its
`createIssue` — the card's field writes, its filing, its parent and dependencies, its content, its
comments, and the delete that would take the issue back — and so does Linear, before each
mutation but its `issueCreate`, `projectCreate` or `documentCreate`: the relations a create writes
in the same call, the whole write that finishes a create answered as an id already held, each
asset upload and each retry of one, and each comment. Each ask renews the lease first; once one
cannot be renewed the answer is a refusal, to that request and to every later one, so a
destination that has lost its create sends nothing more — not even a delete of what it made.
A folder of Markdown writes files and sends no request, and is renewed for before every call the
engine makes to it. So is a source over the stdio protocol, which is sent one request per call —
but the guard does not cross the pipe, so what the plugin behind it sends its own backend in
answer to that one request is not asked for one by one. A renewal that finds the intent naming another attempt — one that
took it over after this copy's lease ran out — or finds it gone, that attempt having since linked
the counterpart or released it, stops the copy with `create-in-progress` before that write,
naming what it found, sending nothing more and taking nothing back, since what it wrote is now the
other attempt's to finish. On the normal path the cost is exact: an update and an `unchanged`
copy send what they always sent, to the destination and to the source; an uninterrupted create
from a folder of Markdown sends the destination exactly what it always did and no remote party
anything for its intent, whose take and release are lock-guarded file writes; and one from a
hosted source sends a folder exactly what it always did, and a board that plus one repository
listing after `createIssue`. A create into Linear, from either, also sends the one read by id
before it that "How a copy to Linear finds what it wrote" below describes. The rare paths are bounded rather than counted. A hosted
source is sent at most one intent write before the first destination write plus one renewal
per destination write request sent while it owns the create — on a board, every write request of
the creating call but `createIssue` itself — each its existing narrow metadata write, and the intent
is removed in the link write it already sends; an intent write answered in a shape the copy cannot
read adds one read of the item. Resolving an intent adds, on a board, one listing
— two when it waits, and one more after a create made because nothing was listed — a page each
for every hundred issues created since five minutes before `at`, plus the reads the counterpart's
own copy makes; on Linear, one read by id, and a re-sent `issueCreate` answered as a duplicate
adds one read; on a folder, a read of its files. A re-create adds nothing to a create but the
listing after it on a board.

Those exact figures are held, not asserted: `crates/onetaskgraph-e2e/tests/e2e/release_requests.rs`
runs a create, an update, an `unchanged` copy and every task write verb against the loopback board
and Linear and holds the requests each sends to the ones the v0.12.13 binary sent for the same
commands — recorded with that binary in `release_requests.v0.12.13.json` — allowing only the one
listing above and, on Linear, the one read by id before each create. A project's or a document's create from a hosted source adds the one narrow write
that takes its intent off beside its link.

**Settings**, in each source's own block beside `cache` and `routes`, in whole seconds, each 120
when absent and left out of `config show` when unset: `create_lease_seconds`, read off the copy's
**source**, and `create_quiescence_seconds`, read off its **destination**, and only where that
destination's lookup by key can lag a create (a GitHub board).

```yaml
sources:
  notes:
    plugin: local-md
    config: { root: ~/notes/tasks }
    create_lease_seconds: 120
  board:
    plugin: github-projects
    config: { owner: example-org, project_number: 7, repository: example-org/store }
    create_quiescence_seconds: 120
```
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### How a copy to Linear finds what it wrote

A copy whose source records no link to what it wrote — a copy into a Linear source declared
private from one that is not, which may not name the private item, and a `--no-link` copy —
finds each counterpart on every later copy by asking Linear for the records whose
`onetaskgraph.origin` is the item copied. That question goes through Linear's filtered reads: an
`issues` search of the description, and listings of the projects and documents. On the real
`TES2` team those once missed records written moments before, and once missed records a repeat a
minute earlier had found. A copy then created all five records of a plan a second time, and a
document copied straight after its project left a budget link unresolved. Why Linear's listings
missed them is not established, so nothing here relies on a listing being current.

**Every create of a copied item carrying an origin is sent under ids that item's origin decides.**
Where the copy gives it no create key of its own (above), Linear is sent the ids of the key the
store derives for that origin with no replaced link:
`<source qualified id>|<destination source name>|`, with `|project` or `|document` appended,
under the namespace above, transformed and tried generation by generation as a keyed create's
are. A copy that recorded an intent and one that could not therefore send one create under the
same ids, and the slot records no key for a create that carried none.

**A lookup by origin that lists nothing reads that id before anything is created.** Each of a
copy's lookups — of an issue, a project and a document, its own and those a document copy makes
to resolve the links it carries — is answered from the filtered read when that read lists the
counterpart. When it lists nothing, the id the origin's key derives is read directly, and a read
by id answers a record the moment it is written. A record there recording that origin is the
counterpart, found as if listed. One recording another origin is read past to the next
generation's id. The first id reading as no record — none there, or a trashed one — ends the
read, since a create of that origin tries that id before any later one.

**A create Linear refuses because its id is taken is never followed by a second record of the
same origin.** The record holding the id is read. One recording the create's origin is the
counterpart an earlier copy made, which the lookup did not reach — a trashed record left in front
of it, for instance — and the copy is refused naming it, having created nothing: copy again. Any
other holder sends the create under the next generation's id, and any other refusal is the
create's answer, as it was.

**What it costs.** A lookup that lists its counterpart sends nothing more, so a repeat copy of an
unchanged plan sends what it always did. Each record a copy creates on Linear costs one read by
id, before its create. No cheaper read replaces it: Linear refuses operation batching, an id that
names nothing fails a whole aliased query, and an `id: {in: …}` filter is a filtered read like the
lookup it backs up. Nothing is kept on the host: the ids are computed from the origin each time,
so a copy run on another machine, holding nothing of the first, finds the same records.

**What the ids say, and where they go.** The destination's name is part of the key, so one item
copied into two Linear sources lands once in each, under ids of each one's own key. A copy out of
a source declared private into one that is not records no origin there and takes no intent toward
it (see "The public boundary"), so its create carries no origin to derive an id from: Linear
chooses the id, and nothing the issue holds names the private item. A counterpart a person
deleted on Linear frees its id, and the next copy creates it again under that id; one trashed
keeps its id, and the next copy creates it under the following one.

### What a write reports it left

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-e2e/tests/e2e/write_results.rs`, which read every item back through
     `task show` and hold each answer to it on a folder, the loopback board and the loopback
     Linear workspace. -->
Every `created`, `updated` and `unchanged` item of a copy that writes carries `result:
{"version", "status", "priority"}` — what the destination item holds once the copy is done, the
status exactly as `task show` spells it and `priority` `null` for a project or a document (and
`status` for a document). It is read off the write's own answers, and a field the copy kept — a
preserved status, a priority the guard kept, an `unchanged` item — off the copy's read before it;
no request is added for it, so a dry run carries none, and nor does an item a repeat copy
`skipped`, which read nothing of the destination. `task update`, `task status set`, `task
priority set`, `task content set` and `task metadata set` answer the same `result` under
`--json`, on the same terms; under `--expect` it also carries the `parts` it always did. A
`failed` item the copy refused after reading its counterpart — `destination-status` — carries
`held: {"version", "status", "priority"}`, the item as that read held it, and so does the failure
document of a `target-task-origin` refusal; one never read carries none, a `stale-link`'s
included, since its link names nothing there. `task comment add --json` answers the comment as it
always did, plus `item`: the qualified id it commented on.

`version` is the change token a read would then give, and `null` only where the write's answers
cannot vouch for one without another read:

- **a folder of Markdown** answers it: it is the digest of the file it wrote;
- **Linear** answers it from the issue, project or document its mutation selected — `task
  priority set` included, and with the source's local cache on as off — except for an item the
  same command went on to give relations, whose answers do not say the token after them: `null`
  there;
- **a GitHub board** answers `null` for an item it wrote, because a board's token is given only
  once an item has gone sixty seconds unchanged — the `task show` after a write answers `null`
  too — and the version its read held for an item it left `unchanged`;
- **an in-memory source** gives no tokens at all, and **a source hosted over the stdio plugin
  protocol** answers `null`: the protocol does not carry what its write left.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

<!-- llmlint: ignore[contracts_have_one_source_or_a_drift_gate] Held by a drift gate: `the_readmes_example_of_a_copy_report_validates_against_the_schema_the_binary_emits` in `crates/onetaskgraph-e2e/tests/e2e/surface.rs` validates the example below against the `CopyReport` root the binary emits. -->
`--json` gives one entry per item for a script to read:

```json
{"items": [{"source": "notes:ENG-142", "action": "updated", "destination": "work:ENG-142",
            "via": "link", "link": "unchanged"}]}
```

`action` says which of the things above happened to that item — `created`, `updated`,
`unchanged`, `orphaned`, `skipped` for an id `--status` left out, or `failed` for a task its
destination cannot represent or a copy left unwritten because it kept changing while it was
read — and `destination` is `null` for a dry run that would have created something and absent
from a `failed` and a `skipped` item. The vocabulary itself is the
`CopyAction` root of `onetaskgraph schema`, which is what both SDKs are generated from and
what the journeys validate this output against.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by a drift gate:
     `the_readme_names_every_word_of_the_copy_reports_via_and_link` in
     `crates/onetaskgraph-e2e/tests/e2e/surface.rs` reads this paragraph and fails unless the words
     it names are exactly the ones `onetaskgraph schema` emits for these two fields. -->
`via` is one field with seven words, on every `created`, `updated` and `unchanged` item. On
`updated` and `unchanged` it names the rule above that found the counterpart: `target-task`,
`link`, `origin`, `scan`, `match` for `--match-by`, or `intent` for the item an earlier attempt of
the same copy created, found by its create key ([below](#a-create-safe-to-retry)). On `created`
it is always `created`, because none did. `link` says what became of the item's own link, on those same three actions of a
copy that writes: `recorded`, `unchanged`, `unrecorded` where its source could not hold
one, or `skipped` under `--no-link`, which writes nothing at all to the source. A dry run
reports `via` and leaves `link` out, and an `orphaned` item, a `failed` one and a dry run's `oversize` one carry neither.
In the generated SDKs the six rule words are the type `CopyVia` and `created` is
`NoCounterpart`, and the link words are `CopyLink`.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-github-projects-e2e/tests/e2e/project_status.rs`,
     `crates/onetaskgraph-linear-e2e/tests/e2e/linear_status.rs` and
     `crates/onetaskgraph-e2e/tests/e2e/project_status.rs`, which assert the document below
     exactly, the requests each source is sent, and each refusal's exit code. -->
A project's **status** is set on its own with **`project status set <ID> <CATEGORY>`**, `<ID>`
the project's qualified id and `<CATEGORY>` one of the categories `task status set` takes. It
writes that one field — title, content, labels, metadata, repositories, dependencies and the
project's tasks stay exactly as they are — and answers with the status as the source reads it
back. A project delivers no task, so nothing is kept in step after it. On a GitHub board it is
the paired write a task status takes, for the project kind: a terminal category selects the
project kind's mapped option first and then closes the issue with the category's fixed reason —
completed for `done`, not planned for `cancelled` — leaving an issue already closed with a
reason that reads that category as it is; an open category reopens a closed issue and then
selects its option; a draft item is refused as a task's is. On Linear it is one `projectUpdate`
carrying only `statusId`, the project status the source's `status_mapping` gives the category; a
folder of Markdown rewrites the project file's own `status` and nothing else of the file; an
in-memory source sets the held project's status. `--json` answers:

```json
{"id": "plans:I_40", "status": {"category": "done", "name": "Done"}}
```

and its human form is one line naming the project and the status it now reads. It exits `0` on
a write, and `1` for a refusal, with the failure document — a source nothing configures, one
with no write side, a project the source does not hold (a task's id among them), and a
category the source has disabled, each refused before anything is written. An out-of-process
plugin answers it only if its handshake declares `project_status`; one that does not is sent
nothing and refused in the words a plugin with no such write refuses with
([`docs/plugin-protocol.md`](docs/plugin-protocol.md) §3.15, §4.17a). **Its library form** is
`onetaskgraph_core::Engine::set_project_status(&self, id: &GlobalId, category: StatusCategory)
-> Result<ProjectStatusSet, EngineError>`, answering `ProjectStatusSet { id, status }` — the
document above — through the same public boundary a task's status write passes.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### Holding a write to the version you read

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-e2e/tests/e2e/held_writes.rs`, which drive every verb named here on a
     folder, an in-memory source, the loopback board and the loopback Linear workspace, each
     hosted one with its cache on and off. -->
`--if-version VERSION` on `task update`, `task priority set`, `task content set`, `task metadata
set`, `task status set`, `task comment add`, `task comment edit` and `task copy` makes the write a
precondition: it lands only while the task is at the version a read of it reported (`task show`'s
`version`), and otherwise exits `1` with nothing written and the failure kind `version-moved`,
whose `held.version` — and message — name the version the task is at now. Read again, decide
whether the write still applies, and write at that version. On `task copy` a bare `VERSION`
names the version of the one destination task the copy writes over, and is refused beside more
than one task, a query, a project or a document as `if-version-one-task`; `--if-version
ID=VERSION`, repeatable, holds the counterpart of each task `ID` names to its own version, decided
on the copy's own read of it, so one that moved is answered `failed` as `version-moved` — its
`held.version` the version it is at — and written nothing while every other task is copied. A
copy that creates its counterpart holds nothing. It needs no cache, and leaves `--expect`, which is about cost and never a
precondition, exactly as it was.

Which guarantee a write gets is its source's:

| Source | Guarantee |
| --- | --- |
| `local-md` | **Atomic.** The check and the write are one step under the lock beside the task's file, which every held write of it in any process waits for, so of two held writes racing at one version exactly one lands. A write made without `--if-version`, and an edit made outside the store, take no lock and are not excluded. |
| `in-memory` | **Atomic.** The check and the write are one step under the one lock over the source's work. Its version is a digest of the task, its comments and its forward edges. |
| `github-projects` | **Checked immediately before the write.** GitHub takes no conditional write, so the task is read just before the first mutation; a change landing between that read and the mutation is GitHub's residual race, the time one request takes. A token is given only once an item has settled (see "A local cache of a remote source"), so a task changed within the last minute reads with no version and every held write of it is refused until it settles. |
| `linear` | **Checked immediately before the write.** Linear takes no conditional write either, so the race is the same one request wide. A Linear task's version is the token its `updatedAt` and facets give, which a comment does not move. |
| a stdio plugin | **Checked immediately before the write**, as for a hosted backend. |
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### Reading the comments made since a watermark

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The shape is
     `CommentsSince` in `crates/onetaskgraph-core/src/engine/comments_since.rs`, published in
     `onetaskgraph schema` and generated into both SDKs; the journeys in
     `crates/onetaskgraph-e2e/tests/e2e/comments_since.rs` hold every answer to exactly these
     members on every source kind. -->
`task comments <SOURCE> --since <RFC3339>` lists every comment made or edited at or after the
instant on that source's tasks — narrowed, as `task list` narrows, by `--metadata`, `--status` and
`--item` — across every page of the tasks and of each one's comments, oldest first by when each
last changed:

```json
{"schema_version": 1, "watermark": "<RFC3339>", "comments": [{"item": "<source>:<native id>",
 "id": "<comment id>", "url": "<url or null>", "author": "<login or name or null>",
 "author_kind": "person", "created_at": "<RFC3339>", "updated_at": "<RFC3339>",
 "version": "<opaque>", "body": "<markdown>"}]}
```

`author_kind` is `person`, `bot` or `unknown`, the source's own call: a GitHub board reads a
`Bot` author and spells it `<login>[bot]`; Linear names no user for an integration's or a bot's
comment; a folder of Markdown and an in-memory source read a name ending `[bot]` as a bot. A
comment's `version` is when it last changed, at the backend's own precision. The `watermark` is
where the next read starts: the earlier of the source's own time (a hosted service's `Date`) and
this host's at the read's start, set back by the source's consistency window — two minutes for
a GitHub board, whose issue search lags the comments it indexes, one for Linear, none for a
folder or an in-memory source. So a comment the source had not yet shown is read on the next
pass, and a comment read twice carries the same `id` and `version`: deduplicate by the two.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### When an issue closes: reconciling and completing its card

A GitHub board holds a card beside an issue that closes on its own — a person closes it, a
change request merges with a closing line, a landing closes it — and the card stays where it
was. Every read already follows the closure (see [Status mapping](#status-mapping)); these two
verbs move the cards themselves. A source whose status *is* its closure — Linear, whose
workflow state is the closure, a folder of Markdown, an in-memory source — has no card to move:
each verb sends it nothing and says so.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-github-projects-e2e/tests/e2e/status_reconcile.rs` and
     `status_complete.rs`, and `crates/onetaskgraph-e2e/tests/e2e/status_complete.rs`, which
     assert each document below exactly, the requests each verb sends, and its exit codes. -->
**`task status reconcile --source <SOURCE> [--dry-run]`** is an on-demand tool, run by a person
— nothing runs it on a schedule — to bring one source's cards in line after closures made
outside the stack. A task needs it when its issue is closed with a reason that reads `done` or
`cancelled` and its card does not sit at the option the mapping gives that category, a card
with no option included; it is written to that option and nothing else — never the issue's
state, which already says it. Open items, other closures and cards already in line are never
written and never listed. **A project item is brought in line exactly as a task item is** —
a plan's own home card, the item whose issue carries the plan's task issues as sub-issues —
each judged against its own kind's half of `status_mapping`: `done` for an issue closed as
completed or with no reason, `cancelled` for one closed as not planned or as a duplicate.
Candidates come from the board's own `is:closed` filter, never its
unfiltered items, and each is confirmed from the item it reads, so a filter that let open items
through would cost reads and write nothing wrong. Writes go one per item through the source's
own write pacing (`pacing.min_mutation_interval_ms`), which is what keeps a run of a couple of
hundred under GitHub's secondary limit on content-creating requests. `--json` answers:

```json
{"items": [{"source": "board:I_7", "kind": "task", "from": "Todo",
            "to": {"category": "done", "name": "Done"}, "action": "updated"},
           {"source": "board:I_40", "kind": "project", "from": "In Progress",
            "to": {"category": "done", "name": "Done"}, "action": "updated"}]}
```

`source` is the item's qualified id and `kind` what it is, `task` or `project`, always present;
`from` is the card's option before, or `null`; `action` is `updated`, `would-update` under
`--dry-run` — which writes nothing — or `failed`, with `error` the failure object a failure
document carries. It exits `0` when every listed item was updated or would be, `4` when any
failed, and `1` when the whole command was refused, with the failure document. Its human form is
one line per item and a closing count, a project item's line naming `project` before its
qualified id, or, for a source whose status is its closure, that nothing needs reconciling. A source answers it only if its plugin declares the capability
`reconcile_status`; an out-of-process plugin that does not is sent nothing
([`docs/plugin-protocol.md`](docs/plugin-protocol.md) §4.23).

**`task status complete --issue <OWNER/NAME#N>... [--source <SOURCE>]... [--dry-run]`** moves to
`done` every item the configured sources hold for the issues a landing closed, by issue
reference — `owner/name#N`, exactly the spelling a landing event's `closes` reports, its owner
and name compared ignoring case; anything else is refused as a usage error naming it. **The
caller names no source**: every source the configuration declares is asked — not only the
`default_sources` — one after another in configuration order, or only those `--source` names, a
narrowing for a person or a test; a name the configuration does not hold is a usage error
naming it. Each source whose plugin declares the capability `complete_by_issue` — a GitHub
board does — is read once per 50 references, and nothing else of it: no read of the board's
items, no search, no project read, so a call costs one read per 50 references per GitHub
source plus the writes for the items they hold, and never grows with a board's size. Every
other source — Linear, a folder of Markdown, an in-memory source, an out-of-process plugin that
does not declare it — is sent no request and listed in `unsupported`. Per reference and source:

- no item for the issue, or an issue the source's credential cannot see: `absent`, no write.
  **A project item is held exactly as a task item is** — a plan's own home card, its issue
  carrying the plan's task issues as sub-issues — each judged against its own kind's half of
  `status_mapping`, from the same read, and answered with `kind` `project`. On a GitHub board a
  number past GitHub's 32-bit
  issue numbers names no issue at all, so it is `absent` without being sent;
- closed as not planned or as duplicate: `skipped`, with `reason` `closed as <reason>`, no
  write — the closure already says it, and a landing does not overrule it;
- closed as completed with its card already at its kind's `done` option: `unchanged`;
- otherwise the item is written `done` through its kind's status write — its card moved to its
  kind's `done` option, and its issue closed as completed if it is still open, never reopened —
  so every task a task item delivers is kept in step exactly as after `task status set`:
  `updated`, or `would-update` under `--dry-run`, which writes nothing.

Because sources are asked one after another, an open issue held on two boards is closed by the
first board's write and read closed as completed by the second, which moves only its card: one
call closes an issue at most once. `--json` answers one item per distinct reference for each
source asked, sources in configuration order and references in the order given:

```json
{"items": [{"issue": "acme/work#12", "source": "plans", "task": "plans:I_12", "kind": "task",
            "from": "Todo", "to": {"category": "done", "name": "Done"}, "action": "updated"},
           {"issue": "acme/work#40", "source": "plans", "task": "plans:I_40", "kind": "project",
            "from": "In Progress", "to": {"category": "done", "name": "Done"},
            "action": "updated"},
           {"issue": "acme/work#12", "source": "followups", "task": null, "kind": "task",
            "from": null, "to": null, "action": "absent"}],
 "unsupported": ["notes"]}
```

`task` is the held item's qualified id whichever its kind, and `kind` — always present — says
which it is: `task` or `project`, and `task` for an `absent` item and for a reference a source
could not be asked about. A reader that took `task` to be a task id reads `kind` first. `reason` is present on a `skipped` item and `error` — the failure object — on a `failed` one; an
`updated` item whose task delivers others carries `delivered`, the entries `task status set`
reports, and has none when it delivers nothing — a project never does. It exits `0` when no item
failed, `4` when one did or a delivered task could not be kept in step, and `1` when the whole
command was refused. Its human form is one line per item, a project item's line naming `project`
before its qualified id, a line naming the unsupported sources, and a closing count.

**Its library form** is `onetaskgraph_core::Engine::complete_issues(&self, issues: &[IssueRef],
only: &[SourceName], dry_run: bool) -> Result<IssuesCompleted, EngineError>`, an empty `only`
asking every configured source; it answers the very document above, each item with its `kind`,
as `Engine::reconcile_statuses` answers the reconcile's, and a delivered task it could not keep
in step is reported in its result rather than failing the call. `IssueRef` is
`onetaskgraph_plugin_api::IssueRef`, parsed from and displayed as `owner/name#N`.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### Carrying comments, and copying only what changed

Every task a source reads carries a **change token**, `version` in the JSON of `task list`,
`task show`, `task show-many` and every list, search and `--commented-since` result, when its
source can promise that two equal tokens of one task mean nothing in it changed — its title,
body, status, priority, labels, board fields, metadata, edges and comments, a comment added,
edited or deleted included. A GitHub Projects board builds it from the issue's `updatedAt`,
its board item's `updatedAt`, its comment count, the count of dependency and project
membership events on its timeline — GitHub moves neither time for either — and its
repository, which its reads already select; a folder
of Markdown gives the digest of the task file; Linear gives none until its live lane proves
which changes its `updatedAt` covers, and a task without one is always read. The token is
opaque: `docs/plugin-protocol.md` §4.13d states what each plugin's keys on.

**`--comments`** keeps the destination's copies of each task's comments in step with the
source's, in either direction, by one rule: the copy adds a copy of each source comment the
destination holds none of, edits a copy whose origin comment changed, and deletes a copy whose
origin comment is gone. A comment written at the destination is never edited or deleted, and
a source comment that is itself a copy of one of the destination's own is never sent back — so
copying a local task back to the board it came from carries only the comments written locally.
Each copy names where it came from as its `origin` — the task, the comment's id there, its
author, its times and its address — and a chain of copies keeps naming the first one. A folder
of Markdown keeps the origin natively, taking the origin's id, author and times for the copy;
GitHub and Linear set a comment's author and times themselves, so there it rides as a hidden
marker at the end of the body, which every read takes back out and reports as `origin`, and an
edit through `task comment edit` keeps. Each item's outcome counts what was done:

```json
{"source": "board:I_1", "action": "updated", "destination": "notes:I_1", "via": "scan",
 "link": "skipped", "comments": {"added": 1, "edited": 1, "deleted": 0, "unchanged": 3}}
```

Every copy records itself on each task it writes — the source token it read, whether it
carried the comments, and a digest of what it left (`docs/metadata.md` has the three keys) —
and a `--comments` copy or a copy by query **skips** a task nothing has changed at either end
since: it is reported `unchanged` with `skipped: true`, and nothing more of it is read than the
listing that named it, or by id the read that found it. An edit at the destination since —
a copied comment edited or deleted, a comment written there — is never skipped: the copy reads
the task again and restores what it copied, leaving the destination's own comment alone. A copy
by id without `--comments` reads and compares every task as it always did. Two bounds, stated
rather than discovered: a task skipped from its listing keeps the edges its last copy wrote, which
a copy of a different set of tasks might name differently; and a task the rest of a query keeps
but which no comment has touched since the instant, and which was never copied, is read on every
copy to be ruled out.

**A copy by query** — `task copy --to notes --source board --commented-since <INSTANT>
--metadata <KEY>=<VALUE>` — copies exactly what `task list` with the same narrowing answers,
and the query's own answer carries the tokens it skips by. Its comment activity is confirmed
from the comments the copy reads anyway, or for a skipped task from the destination's copies of
them, so no task's comments are read twice in one copy.

**A copy is one revision of each item, or none of it.** A task whose comments fit one page is
read whole in one request; one whose comments run longer is read a page at a time, each page
beside the task, and when two of those reads disagree — a comment edited, added, or the body
changed in between — the task is read again from its first page, once. A task whose source
gives it no change token — every Linear task, and a GitHub task changed within the minute
before the read — has nothing on a page read that moves when a comment alone is edited, so when
its comments run past one page it is read again in full until two passes in a row agree on
every comment's id, body and time, at most three passes. Read differently again, or never
settling, it is written nowhere and reported `failed`, with the failure document every verb reports in,
while every other item lands and the command exits `4`. So a copy by query into a fresh folder
of Markdown, with `--comments --no-link`, is a snapshot of the board: nothing writes to that
folder afterwards but another copy into it, and the board is left exactly as it was.

A copy that reaches a source which counts its own requests — `github-projects` does — also
says what it **spent**: the requests those sources sent for the command, and what that cost
each budget they draw on, with a flag on any amount that is partly a modelled lower bound
rather than a figure the backend reported. It is the source's own account, summed over the
command, and a copy whose sources count nothing carries no `spent` at all rather than a
zero. Its shape is the `spent` property of the `CopyReport` root of `onetaskgraph schema`.

### Task templates

A **task template** is a [minijinja](https://docs.rs/minijinja) document whose variables
are declared in YAML front matter, so the shape of a task is a file you own rather than
prose several tools restate:

```jinja
---
onetaskgraph_template: 1          # required when front matter is present
description: <string>             # optional
variables:                        # optional; name -> declaration
  <name>:                         # ^[a-z][a-z0-9_]*$
    description: <string>         # required, non-empty: what a prompt shows
    type: string                  # string | text | integer | boolean | list | object; default string
    items: string                 # list only: string | object; default string
    required: true                # default: true without `default`, false with one
    default: <value of `type`>    # optional; `required: true` beside a default is refused
    schema: <JSON Schema>         # object and list only: draft 2020-12, inline; optional
members: []                       # project templates only: the plan graph (below); optional
---
# {{ title }}
{% for criterion in criteria %}
- {{ criterion }}
{% endfor %}
```

A file without front matter declares nothing; front matter with an unknown key is refused
by that key. `string` is one line and `text` any number. The body is minijinja, whole —
filters, loops, conditionals, macros, a variable used many times or not at all.
`{% extends %}`, `{% include %}` and `{% import %}` find the files they name in the
`--search-path` directories, in order, and never in the working directory unless you name
it. The declared set is every chain file's front matter together: a redeclaration may change
`description`, `default`, `required` and `schema` — the file nearer the rendered one wins — but never
`type` or `items`, which is refused naming both files. A template named by an expression —
`{% include kind ~ ".md" %}` — is part of the chain too, for the answers that name it, and
joins it at the tag that names it, exactly as a file a literal names would: its front matter
joins the declared set at its own distance from the rendered file, and its bytes join the
digest where rendering first reads it. Rendering is strict — a name that is
neither declared nor set fails, naming it and its file — with no auto-escaping, trailing
newlines kept, and `trim_blocks` and `lstrip_blocks` on. An optional variable with no
answer and no default is `none`.

An `object` or `list` variable may declare a `schema`: an inline JSON Schema (draft 2020-12,
no remote references) its answer — and its `default` — must satisfy as well as its type. An
answer the schema refuses is refused before anything renders or is written, by every verb
that renders (`template render`, and `create` and `render` of a task, a project and a
document), exit `2`, naming the variable and the JSON pointer of the first violation:
`the answer to "plan" does not satisfy its schema at JSON pointer "/budgets/1": …`. A
refused `render` leaves the item's content and stored answers as they were. A schema on a
variable of any other type, or one that does not compile, is refused by its key; a variable
with no `schema` is held to its type alone, exactly as before. `template variables --json`
reports each declared schema, and a redeclaration nearer the rendered file gives its own.

`template variables` lists the declared set and the template's **digest**: `sha256:` over
every chain file's name and bytes, which moves whenever any file of the chain does. Each
variable `template render` needs takes the first of:

1. `--var NAME=VALUE` — literal text for a `string` or `text` variable, YAML for any other;
2. the answers file, `--answers FILE` (`-` for standard input): a YAML mapping of name to
   value;
3. a prompt, when the `interactive` setting is on — one variable at a time, showing its
   description, type and default, asking again when a value is not of its type;
4. its declared `default`.

An answer to no declared variable, an answer of the wrong type, and — when not interactive
— every required variable left unanswered, listed in one refusal, each exit `2`. So does an
interactive run with something left to ask whose standard input is not a terminal: it never
waits for an answer nobody can type. Automation passes `--no-interactive`, which both SDKs
always do.

### Creating and regenerating from a template

`task create`, `project create` and `document create` make an item in one source, its body
(a project's description) rendered from a template — `--template FILE`, or `--template-loader
FILE` for a template a caller states (below) — with its answers taken exactly as `template
render` takes them, or given as it is with `--body-file PATH` or on standard input. `task create` prints the new item's qualified
id, and under `--json` the item exactly as `task show --json` prints it; `document create`
answers as `document show` does, and with `--id DOC` naming a document the source holds it
replaces that document rather than adding a second. `project create` names the project's own
id with `--id` and answers as `project show` does; over a project the source holds it
replaces the content, the answers and the provenance whole and keeps everything else — its
status, labels and repositories unless `--status`, `--label` or `--repository` names them,
every metadata key `--metadata` does not set, and its dependencies. A key of the reserved `onetaskgraph.`
namespace given with `--metadata`, and a source that cannot be written, are refused by
name before anything is written.

An item rendered from a template records where it came from under the reserved metadata key
`onetaskgraph.template`: the template's reference — a template file's absolute path, or a
loader document's `reference` verbatim — the chain **digest** it rendered with, and the
SHA-256 of its content (`body_digest`) and of its resolved answers as canonical JSON
(`answers_digest`). An item made from a plain body records none. From that entry alone a
hand edit (the content's hash is not `body_digest`) and a changed template (the chain's
digest is not `digest`) are both visible. **It proves nothing about who wrote it**: a check
reading only these hashes trusts them, so provenance forged by hand passes it. The entry is
four strings, whatever the template's reference — nothing caps its length but what the
destination caps a whole item at.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys
     `a_rendering_whose_references_a_copy_rewrites_records_the_digest_of_what_it_was_given`, `a_copy_carries_provenance_it_cannot_vouch_for_verbatim` and `a_rendering_whose_references_a_copy_leaves_alone_carries_its_provenance_unchanged` in `crates/onetaskgraph-e2e/tests/e2e/rendered.rs`, which assert each case below through the binary. -->
A copy carries the entry like any other metadata, with one exception. A document copy
points the references in a document's content at the destination's own records, and when it rewrote at least one of them in a rendering that
still hashes to its recorded `body_digest`, the copy records as `body_digest` the digest of
the rendering **as the copy rewrote its references** — so the content that lands matches
its entry — and carries `template`, `digest` and `answers_digest` as they were. The answers
kept where the item was authored, and the `answers_digest` naming them, still hold the
pre-copy locations, so the copy is not a fresh rendering of those answers, and a regenerate
from them reproduces the pre-copy content. A rendering edited by hand, an entry this product
did not write, no entry at all, and a copy that rewrote nothing are carried verbatim, so a
hand edit stays visible wherever it is copied.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

**The answers themselves are kept in one place only**: beside the item in a `local-md`
folder's own file, which is where an item is authored — never in its content or its
metadata, so they cost a hosted item nothing, and nothing of a task is written twice into a
GitHub issue. `task answers`, `project answers` and `document answers` print them as YAML (`--json`: one JSON
object), and refuse, naming the item, when none are stored — which is every item of a
source that keeps none. A copy carries content and metadata alone, **never the answers**,
at either end.

`task render`, `project render` and `document render` regenerate an item in place, and write its content, its
provenance and its stored answers in one write and **nothing else** — its id, title, status,
labels, project, repositories, dependencies and every other metadata key stay as they were.
The answers start from the stored ones when they hash to the recorded `answers_digest`;
otherwise every required variable has to be answered again, and a render that leaves one
unanswered is refused as `supply every required answer`, naming each and why the stored
answers were not used (exit `2`). `--var`, `--answers` and `--unset NAME` — which drops an
answer so its default applies — are laid over that base. The template is the one given;
else the recorded reference, re-read when it is a readable file — its `extends`, `include`
and `import` resolved over `--search-path` again, because the provenance records none;
else the render is refused naming that reference, because a reference is never turned into
a location. `--dry-run` renders and writes nothing; `--json` prints `{id, digest,
body_digest, changed, body}`, and a render that would change nothing reports `changed:
false` and writes nothing.

### Plan graphs from a project template

A project template may declare `members`: the tasks one `project create` makes beside the
project, each rendered from a task template of its own, and the edges between them.

```yaml
members:
  - id: "{{ slug }}-design"              # the member's native id
    title: "Design {{ goal }}"
    template: {file: design-task.md}     # or {loader: design.loader.json}
    answers: {topic: "{{ goal }}"}       # its template's answers
  - id: "{{ slug }}-ship"
    title: "Ship {{ goal }}"
    template: {file: ship-task.md}
    answers: {steps: "[{{ steps | join(', ') }}]"}
    depends_on: ["{{ slug }}-design"]    # each a `blocks` edge, written first
    metadata: {myapp.size: 3}            # caller-owned keys
    repositories: [github.com/acme/widgets]
```

Every string in a member, at any depth, is rendered from the project template's own resolved
answers, strictly. A rendered answer written as a string is read the way `--var` reads one —
literally for a `string` or `text` variable, as YAML for any other — and any other value is
taken as it is. A relative member template is looked for beside the file that named the
project template (the template file, or the loader document) and then over its search path.
Only the rendered project template declares members; one an `extends`, `include` or `import`
names is refused.

`project create` then renders every member and holds its answers to its own template —
schemas included — **before anything is written**: a member refused for anything, a graph
naming one id twice, naming a member nobody declares, or going round a cycle, refuses the
whole create, naming the member, and writes nothing. Otherwise it creates the project, then
each member under it, dependencies first, exactly as `task create` creates one — offered its
`id`, its provenance recorded and its answers stored as that verb stores them — and prints the
project's id and each member's, one per line (`--json`: the project as `project show` answers).
A source that issues its own ids — a GitHub board, Linear — still issues them, and each edge names
what it issued. On a public write a member's offered `id` is screened as a fixed entry, as an
explicit project or document id is: a declared term in it is refused, never rewritten. A member
whose write fails after the project and the members before it landed is reported naming it and
every item already created (exit `1`); nothing that landed is taken back.
`--dry-run` renders all of it and prints the plan instead — `--json`: `PlannedProject`, the
project and each member with its content, answers and edges — writing nothing. A project
template that declares no `members` creates exactly what it always did. `project render`
regenerates the project's description alone and leaves its members as they are.
`MemberDeclaration` and `PlannedProject` are roots of `onetaskgraph schema`.

A caller that layers templates of its own states the result with a **template loader
document** — `--template-loader FILE`, `-` for standard input, but never together with
`--answers -` — and this product renders exactly that, never resolving a caller's layers
nor running a caller's command:

```json
{"reference": "<string>", "entry": "<name>", "search_path": ["<absolute dir>"],
 "templates": [{"name": "<name>", "source": "<text>"}], "digest": "sha256:<hex>"}
```

`reference` (required, non-empty) is what the item records; `entry` (required) is loaded over
the `search_path` directories, then the inline `templates`; `digest`, when present, must be
the digest that chain computes, or the render is refused naming both. Every other key is
ignored. `template variables` and `template render` take one in place of their `<FILE>`.
[`docs/local-md.md`](./docs/local-md.md) describes the answers block.

### Writing to a destination that rewrites Markdown

Linear stores an issue's description, a project's content and a document's content as Markdown
it normalizes on write — `- ` bullets as `* `, a table's delimiter row as `| -- |`, a blank line
before a list after a heading or a paragraph line, an indented code block as a fenced one, and
more — keeping the rendered meaning. Nothing predicts that rewrite. A copy, a create or a render
sends the content as it composed it, and the write's own answer says what the destination now
stores. Where that is what was sent, the write is done. Where it is not, the store recomputes
every fingerprint the item keeps over what was stored — a rendering's `body_digest`, moved onto
it only where the provenance vouched for what was sent, so a hand edit's entry is carried as it
came, and a copy's `onetaskgraph.copied_destination_version` — and writes the stored content
back once, beside them, as its own request. That follow-up's own answer has to report the same
content again; a destination that stores it otherwise is refused as `stored-content-diverged`,
naming the item and the first line that moved. A follow-up that also finishes a link holds text
Linear has not seen, so an answer differing only as Linear autolinks a URL (a bare issue URL under
the issue's identifier, a bare document's or project's under the title the copy wrote it with, any
other under itself), escapes a bracket or a tilde, or puts a destination in angle brackets is no
divergence: that answer is written back the same way, and has to come back unchanged. An edit to
the item that the follow-up's answer reports, and that those rewrites do not explain, is refused:
such a link under any other text among them. An
edit made before the follow-up write and absent from its answer is overwritten, because that write
replaces the whole content and no read precedes it. No read of the item follows either write. So
every item a copy lands records a `body_digest` that a read of it verifies, whatever Markdown its
template holds.

An unchanged item is recognised without comparing what was composed with what was stored, which
never compare equal on such a destination. A copy reads the copy record every task, project and
document it writes carries (see [docs/metadata.md](docs/metadata.md), "The copy's record of
itself"): an item whose source still matches `onetaskgraph.copied_version` and whose destination
still digests to `onetaskgraph.copied_destination_version` is written nothing, and an edit at
either end — a destination's title, status or edges included — is written back. A re-render
writes nothing when the item's rendering record names the same template and answers, its
`body_digest` is still that of the content it holds, and its assets are the same set.

A first copy of a realistic plan into Linear — one project, five tasks and a design document
with two new images — sends 20 requests: one request for every origin lookup of the project
copy, which carries the organization and the team's workflow states and project statuses too;
the document copy's own origin lookup; one write of each item and one follow-up of each item
Linear rewrote; and a `fileUpload` and a PUT per new image. A document is filed under the
project its project's copy link names without that project being read; a link left naming a
project the destination no longer holds is refused as a stale link, writing nothing, unless
`--recreate` asks the destination to be walked for it, which points the link at the project
found. An unchanged repeat copy writes nothing at all. `linear-requests-per-plan-copy` in
`crates/onetaskgraph-linear-e2e/budgets.yaml` holds the figure.

### Section links

A copy points every reference between the records of one project at the destination's own
records, in the content of every task, project and document it writes, and in the comments a
`--comments` copy carries. A public view of a private plan's task has its links to its own
headings written in the destination's form too, and its links to other views as "Links in a
public view" below states. A **section link** is
a reference that names one heading of its record as well, and each destination addresses a
heading its own way, so a copy writes every section link in the destination's form.

**The authored form.** In a folder of Markdown, a section link is a Markdown link whose
destination is `<path>#<slug>`:

- `<path>` is the linked record's file, written relative to the directory of the linking
  record's own file — `../tasks/budget-quota.md` from `documents/design.md`. The linked
  record's absolute path is recognised too.
- `<slug>` is the target heading's **slug**: its text lowercased, every character that is not a
  letter, a digit, a space, `-` or `_` removed, and each space replaced by `-`. The n-th
  repeat of a slug in one record is suffixed `-<n>`, so the second `## Budgets` is
  `budgets-1`. `### Step 2.b: retry (once)` is `step-2b-retry-once`, and
  `### Budget <id>` is `budget-<id>`. GitHub's file view and VS Code's preview read the same
  rule.
- A link to a heading of the same record is `#<slug>` alone. Task reads use the authored
  same-record form for every source, as the final row of the table below records.
- The authored form carries no anchor markup (`<a id>`, `<a name>`). A destination that needs
  some is the copy's to write.

**Each destination's form.** Each form is one entry of `SectionForm` in
`onetaskgraph-core`'s `section_links` module. That entry is the one place a destination's form is
decided: it writes the form, parses it back and supplies its markup.

| Destination | Link to another record's heading | Link to its own heading |
| --- | --- | --- |
| A folder of Markdown | `<path relative to the linking file>#<slug>` | `#<slug>` |
| A GitHub Projects issue | `https://www.github.com/<owner>/<repo>/issues/<n>#user-content-<slug>` | `https://www.github.com/<owner>/<repo>/issues/<n>#user-content-<slug>` |
| A Linear issue, project or document | `<record URL>#heading-<Linear slug>-<id8>` | the record's own URL with that fragment |
| Linear without a stored heading id | `<record URL>` | `#<slug>` |
| Task read (every source) | as stored | `#<slug>` |

- **GitHub.** An issue page gives a heading no `id`. So the GitHub source writes
  `<a id="<slug>"></a>` on its own line before every heading of every body it writes, which
  GitHub stores as `id="user-content-<slug>"`. It strips exactly that line again when it reads a
  body, so a body read back through the store is the body as it was authored. A link on
  GitHub's canonical `github.com` host, clicked inside another issue, opens its target at the
  top, because GitHub's in-page navigation drops the fragment. The `www.github.com` host is
  answered with a redirect that keeps it. The link then lands both when opened directly and
  when clicked inside an issue. A link to a heading of the issue's own body takes that same form,
  naming the issue itself. Clicked inside the issue, `#user-content-<slug>`, `#<slug>` and the
  `github.com` URL do not scroll, because GitHub's page handles the hash itself. A new issue has no
  number until it is created, so the copy writes such a link in a follow-up write of what it just
  created. **That rests on undocumented GitHub behaviour**, which the
  credentialed live journey re-observes in a real browser.
- **Linear.** Linear keeps each record's content as an editor state beside its Markdown
  (`descriptionState`, `contentState`). There every heading carries an id Linear assigns when
  the record is written and keeps across an update that leaves the heading's text alone. Its
  own "Copy link" on a heading gives
  `#heading-<slug>-<first 8 hex characters of that id>`. The heading is found in the
  counterpart's stored state by its position among the headings of the same text, so the two
  `## Budgets` carry different ids.
  - Linear's slug is the text lowercased, `&` read as `and`, `_` as `-`, every other character
    that is not a letter, a digit, a space or `-` removed, and each run of spaces and `-` one
    `-`. A repeat is not suffixed.
  - A recognised Linear link is read by its id alone, so a wrong slug beside the right id still
    names its heading.
  - The ids are read off each write's own answer, so a record this copy writes costs no read. A
    record it does not write, linked from one it rewrites, is read once per copy.
  - A link whose target is written later in the same copy is finished in the follow-up write the
    copy already sends when Linear rewrote the linking record, and otherwise in one added
    update.
  - With no stored heading id, a same-record link keeps its authored fragment, as the fallback
    row above records. It remains lossless for task reads and later copies. A cross-record link
    falls back to its target's URL alone, so the record can still be opened.
  - No anchor markup is written into a Linear body, where Linear would show it as text.
  - **The form rests on Linear's undocumented editor-state heading ids and "Copy link" form**,
    which the credentialed live journey re-observes against Linear itself.
  - `linear_section_form` is one named decision. Setting `OTG_LINEAR_SECTION_FORM` to
    `record-url` writes the record's URL alone, for a measurement.
- Any other destination, a stdio plugin among them, receives every fragment byte for byte on
  the rewritten link, as before section forms existed.

**A reference-style link on Linear.** Linear keeps no reference-style link: it stores `[name][1]`
and `[1]: <target>` with their brackets escaped, as text, and autolinks a URL after them. So a copy
to Linear writes each `[text][label]`, `[text][]` or `[label]` whose definition the copied body
holds as an inline link to that target, in a body and in a carried comment alike. It is then read
as any inline link is below, so the cases below that leave a definition as written do not apply to
a used one here. A definition every use of which was inlined is not written. A definition's title
is left out, an image reference keeps its definition, and a definition nothing uses is left as
written. A copy out of Linear reads the inline link, so a folder of Markdown gets it back inline
as a relative path with the authored `#<slug>`. GitHub Projects and local Markdown keep the
reference-style spelling. A Linear plugin served over the stdio protocol has no section form, so
its bodies are not inlined and its follow-ups are held byte for byte.

**A relation Linear adds itself.** When an issue's description links another issue, Linear adds a
`related` relation between the two on its own. A copy does not count that relation as a change when
it writes both issues in one call and the body it sends for either links the other, so such tasks
copied again write nothing. A `related` relation between two tasks whose bodies do not link each
other, or to a task outside the call, is still a change.

**Which spellings a copy reads.** What a copy reads as a link to a record depends on where the
body is bound. Each case below holds for every destination kind:

- **A destination declared private.** An inline link, a reference-style definition
  (`[t]: ../tasks/t.md#what`), a location, and a path relative to the body's own file written
  alone, as a table cell names one, all land alike: on the counterpart and its heading, in the
  form the table above gives that destination. A path that names no record is left as written.
  So is one naming a record of the project with no counterpart there, which is counted
  unresolved.
- **A public or undeclared destination, with a source declared private.** The same spellings are
  read, and the body is then screened as the next paragraph states.
- **A public or undeclared destination, with no source declared private.** An inline link and a
  location are rewritten. A reference-style definition or a path written alone that gives a
  relative path is left as written.

**Links from a private source.** Before writing a public body, a copy inventories every
source declared private and removes its record addresses. A reference with a verified
counterpart on that destination, whichever kind it is, is translated to the counterpart's
address, in its section-link form where it has one. That covers an inline link, a
reference-style definition (`[t]: ../tasks/t.md#what`), a location, and a path relative to the
body's own file written alone, as a table cell names one. The counterpart may have been written
by this copy or an earlier one: a design document copied after its project's tasks keeps its
budget links. A reference with no counterpart there, including one copied only to another
destination, is counted unresolved and removed: a link becomes its text, and a definition or a
path is dropped. Private record addresses never enter a public body, including provisional
writes made before all counterparts have landed.

**Links in a public view.** A view of a private plan's public task keeps a link to another
public task of that plan whose view lands on the same GitHub Projects or local Markdown
destination, pointed at that view and written in the cross-record form the table above gives
that destination. The other view may have been written by this copy or an earlier one. A link
in a view to anything else of the plan, such as the private home's documents, a private task or
a view on another destination, becomes its link text alone, and so does every such link in a
view on a public Linear source. What the public boundary reads of a view is that text, plus any
fragment of a link to another view that names no heading there, which is carried only if the
check passes it. A view is written once when what it links to lands first; of two views linking
each other, one is written a second time within the same copy. A repeat copy writes nothing.

**Reading a tracker's own form.** A body copied out of a GitHub board or a Linear workspace holds
its section links in that tracker's form, written by an earlier copy or by hand. Each one is
recognised as naming its record and its heading, and written in the next destination's form.
That covers both GitHub hosts, with or without `user-content-`, and Linear's heading links. A
heading travels as its text and its position among the headings of that text, never as a slug,
because the slug rules differ. A link to a record outside the copied project is a real URL that
works from anywhere, and is left as it is. A chain of copies that carries the whole project at
each hop ends, in a folder of Markdown, with every link in the authored form. Each points
at the counterpart of the record the authored link named, with a byte-equal `#<slug>`. The path
names the last hop's file, because a folder of Markdown names a created file after its
source's id.

**What a copy reports.** A fragment naming no heading of its record is carried as it is, never
guessed, and counted in that record's `section_links_unresolved`. A Linear heading the
counterpart's stored state does not hold is written as the record's URL alone and counted in
`section_links_unaddressable`. An update a copy added only to finish a record's section links is
counted in `section_link_updates`. All three figures are left out when zero. A rewritten
rendering's `body_digest` is restamped exactly as a copied document's is, so a regenerate from
its answers still reproduces the pre-copy content. A repeat copy of an unchanged project writes
nothing, because what it compares is the translated body.

### Expandable blocks

A copy writes an **expandable block** — collapsed until a reader opens it — in each destination's
own form, in the content of every task, project and document it writes and in each comment a
`--comments` copy carries, and every read gives it back in the authored form. So a body read
through the store is the body as authored, and a rendering's `body_digest`, a copy's record of
itself and an unchanged repeat copy are unaffected.

**The authored form.** In a record held in a folder of Markdown, an expandable block is exactly
these lines, outside any fenced code block and not indented:

```text
<details>
<summary>SUMMARY</summary>

BODY

</details>
```

- `<details>` stands alone on its line, with no attribute.
- The `<summary>` line holds SUMMARY whole: non-empty, on one line, writing `&`, `<` and `>` as
  `&amp;`, `&lt;` and `&gt;`. Its only inline markup is HTML: `<code>TEXT</code>` and
  `<strong>TEXT</strong>`, neither nested in the other, TEXT holding no tag, no backtick and no
  `*`. A `<summary>` line is HTML, so GitHub and a local viewer show backtick code and `**bold**`
  in it as literal backticks and asterisks.
- One blank line, then BODY — one or more lines of Markdown: paragraphs, lists, fenced code,
  tables, images, headings — then one blank line, then `</details>` alone on its line. GitHub
  renders Markdown inside a block only with these blank lines.
- Blocks do not nest. The authored form carries no destination markup; a destination's own
  form is the copy's to write and to read back.

**Each destination's form.** Each form is one entry of `ExpandableForm` in `onetaskgraph-core`'s
`expandable_blocks` module, the one place a destination's form is decided: it writes the form
from the authored one and parses it back.

| Destination | An expandable block |
| --- | --- |
| A folder of Markdown | the authored form, byte for byte |
| A GitHub Projects issue | the authored form, byte for byte; the anchor line the source writes before a heading reaches one inside a block's body too |
| A Linear issue, project, document or comment | `+++ LSUMMARY` alone on its line, a blank line, BODY, a blank line, `+++` alone on its line |

LSUMMARY is SUMMARY with each `<code>TEXT</code>` written `` `TEXT` ``, each `<strong>TEXT</strong>`
written `**TEXT**`, and `&amp;`, `&lt;` and `&gt;` written as the characters they stand for,
which is how Linear stores them. Reading back reverses it: a backtick span becomes `<code>…</code>`,
a `**…**` span `<strong>…</strong>`, and every `&`, `<` and `>` of the text is escaped again. A
`+++` section a person made in Linear reads back as an authored block. The copy writes `+++`,
never Linear's editor shortcut `>>>`, which closes every open section at once when nested. Linear
normalizes the content inside a section as it normalizes any other ("Writing to a destination
that rewrites Markdown", above).

**What is carried as it stands.** Nothing but the authored form is translated. These reach a
folder of Markdown and a GitHub issue byte for byte, and Linear as sent, subject only to Linear's
own normalization:

- `<details open>`, or `<details>` with any other attribute;
- a block missing either blank line;
- a block inside a fenced code block, or indented;
- a block nested in another, the outer one with it;
- a summary holding any other tag;
- a block whose Linear form would not read back as itself: a summary whose text holds a literal
  backtick or `*`, which Linear would read as a code or strong mark, a raw `&`, `<` or `>`, or a
  space at either end; and a block written with carriage returns, which Linear does not keep.

Linear stores each such block as literal paragraphs with its body expanded, as it stores every
`<details>` block. Its own rewrite can give such a block the blank line it lacked, so that it reads
back in the authored form; the Linear source remembers every block it read back as literal text
and writes it back as it was stored, so a copy's follow-up never turns it into a section. A
plugin served over the stdio protocol has no expandable form: it is sent the authored form.

**What the Linear form rests on.** What Linear stores for each form sent is recorded, not
predicted: spike `spike-expandable-blocks` wrote every form to team `TES2` through the API and
recorded what Linear stored, and that recording is committed unchanged in
`crates/onetaskgraph-e2e-support/fixtures/expandable_blocks/` — every record sent and stored,
its editor state, and every change Linear made. The loopback Linear the journeys copy into is
held to storing each recorded record exactly so, and a chain of copies through Linear is
accounted for line by line against those changes. Linear entities in a block's *body* are
decoded as they are anywhere else in a body: one of the recorded normalizations. It rests too on
what the credentialed live check `scripts/live-expandable-blocks.py`, run by hand, observes of the
real trackers: a project copied to a copy of board #1 stores every block in the authored form, and
copied on from the board to team `TES2` stores each as `+++ LSUMMARY` … `+++`, a
`collapsible_section` node of the record's editor state whose `collapsible_heading` holds the
summary, with every body construct inside it, and reads back through the store in the authored
form.

### Image assets

A task or a document carries pictures — a before-and-after screenshot in a design document,
say — as **image assets** stored with the record. Its content references each one as a
Markdown image whose target is `./<name>`:

```markdown
![The settings page before](./before.png)
![The settings page after](./after.png)
```

`<name>` is a bare file name — no `/`, no `\`, no `..` — ending, in any case, in `.png`,
`.jpg`, `.jpeg`, `.gif` or `.webp`. That is the only shape that is an asset reference: an
image at an absolute URL, one whose target has a directory (`./img/before.png`) or starts
`../`, one with no `./` (`before.png`), and a plain link to `./before.png` are content like
any other, stored, shown and copied exactly as written. So is an image outside the
convention's reach because it is not an image at all: one whose `!` is escaped
(`\![before](./before.png)`), and image syntax inside an inline code span, a fenced code block
or an indented code block, which is how a document shows the convention without using it. A
record whose only image syntax is of those kinds holds no asset.

`task create`, `task render`, `document create` and `document render` store an asset from
`--asset PATH`, repeatable, under the file's base name:

```bash
onetaskgraph document create notes --project launch --title "Settings redesign" \
  --body-file design.md --asset shots/before.png --asset shots/after.png
```

Each is refused by name before anything is written: a file whose extension is not one of the
five, two `--asset` with one base name, a reference in the content naming an asset the record
would not hold, and an `--asset` the content does not reference. A `render` keeps every asset
the record holds that the regenerated content still references, an `--asset` replaces a stored
one of the same name, and an asset the new content no longer references is dropped; `document
create --id` over a document the source holds leaves it holding exactly the assets that call
gave. A source whose plugin stores no assets refuses the record, naming the source and the
asset.

`task show --json` and `document show --json` list them as `assets`, in the order the content
first references them — `[]` for a record that holds none:

```json
"assets": [{"name": "before.png", "sha256": "9f86d0…", "content_type": "image/png",
            "path": "/home/someone/notes/documents/settings-redesign.assets/before.png"}]
```

`path` is where the bytes are on this machine — a `local-md` folder keeps a record's assets in
a directory beside its file ([`docs/local-md.md`](./docs/local-md.md)) — and `null` for a
hosted source. `content_type` is `image/png`, `image/jpeg`, `image/gif` or `image/webp`,
decided by the extension.

`task assets <SOURCE:ID> --output-dir DIR` writes every asset the task holds to `DIR/<name>`,
under the name its content references it by, creating `DIR` when it is not there; `--json`
prints the names it wrote as one list, `[]` for a task holding none. The bytes come through the
task's own source — a `local-md` folder's files, a board's uploaded images — so it reads the
same on every store, and a Rust caller reaches the same read as `Engine::task_assets`. A task
that is not there, or a source that cannot be read, fails naming the id and the store's reason.

**A copy carries every asset a record's content references.** `task copy`, `document copy` and
every project copy that carries tasks or documents hand each asset's bytes to the destination
with the record:

- into a `local-md` folder, the bytes are stored as the copied record's own assets and the
  content is unchanged;
- into a source whose plugin stores them at a URL, the content's `./<name>` references are
  rewritten to where that destination serves each image, and the destination records what it
  uploaded under the reserved metadata key `onetaskgraph.assets` — each asset's `sha256` and
  `url` — so a later copy whose bytes have not changed reuses the URL and uploads nothing
  again;
- into a source whose plugin stores no assets — any
  plugin that declares nothing — the record is refused, naming the source, the record and the
  asset, before anything is written for it, because a design document whose pictures silently
  vanished on the way would be approved without them.

A record whose content references an asset it does not hold is refused, naming both. Rewriting
references follows the rule above for a copy that rewrites references: a rendering that still
hashes to its recorded `body_digest` is re-recorded with the digest of the content as
rewritten, and `template`, `digest` and `answers_digest` are carried as they were. A record
whose content references no asset reads, renders, copies and digests exactly as it does without
this feature, on every source, and a plugin that has never heard of assets is never sent
anything about them. [`docs/plugin-protocol.md`](./docs/plugin-protocol.md) §4.9a states what a
plugin is sent.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success — every source asked, every source answered. A dependency read (`task deps`, `project graph`) of an item outside a source declared private also asks that private source what it records of it; one that could not be asked is not a failure of the read, because the item lives elsewhere, and is reported in the answer's `unanswered` and on standard error as a notice. `--complete` makes it a failure. |
| `1` | The command failed while running: an id that names nothing, a configuration it will not run on, a source name nothing configures. |
| `2` | The invocation itself was wrong — an unknown flag, a value out of range, or answers a template refuses. |
| `3` | An `attestation check` ran and the record it checked is not valid — `void`, or `absent`; its answer on standard output says which. (`sources sanitize` uses it for a sweep whose coverage is incomplete.) |
| `4` | The query ran and at least one source did not answer. The others' results still stand and the failure is named on standard error. A write — a copy, `task status set`, `task update` or `task status complete` — also exits `4` when it landed and a task it delivers could not be kept in step; its `delivered` list says which. `task status reconcile` and `task status complete` exit `4` when an item they list failed while the others were written, and name each such item and why on standard error beside the report. A copy's `--dry-run` exits `4` when it answers an item `oversize`, which the copy itself would refuse, writing nothing. |

`--allow-partial` says a partial answer is acceptable and turns `4` into `0`. Nothing else
does: a run that lost a source never exits `0` unless you asked for that. A source in a
dependency read's `unanswered` — declared private, and unable to be asked what it records
because its credential is absent or refused, or it is unreachable or rate-limited — is not a
lost source: it holds nothing the read was asked about, and it is never in `errors`. Asked
with `--complete`, the read counts it as one: it moves to `errors`, and `task deps` exits `4`
while `project graph` exits `1`.

With machine output selected — `--json`, `--output json`, or `output: json` set in a
document, by `--set` or by `ONETASKGRAPH_OUTPUT` — a command that exits `1` also writes
exactly one document to standard output, and nothing else goes there. Its shape is
published rather than restated here: it is the `FailureDocument` root of `onetaskgraph
schema`, which describes every member and is what both SDKs are generated from and what
the journeys validate this output against.

Its `class` is the member to branch on: `refused` means asking again unchanged gets the
same answer, and `transient` — a rate limit, or a source that could not be reached — means
it may not. The standard error line and the exit code are the same with
or without it, and exit `2` writes no document. A partial answer at exit `4` carries the
same `class` on each entry of its `errors`.

### Paging

`--limit N` gives you a page and, when there is more, the token for the next one:

```bash
onetaskgraph task list --limit 20
# ... rows ...
# next page: --page 5b7b22736f7572...
```

Rows are interleaved across the selected sources — one from each in configured-name order,
then the next from each — and within a source they keep that source's own order. The turns
carry on across page boundaries, so a walk returns the same rows in the same order whatever
page size you choose: `--limit 3` and `--limit 50` differ in how many round trips they cost
and in nothing else. The token is the engine's own; a source's cursor travels inside it
untouched, and a walk returns every row exactly once.

A token belongs to the query that produced it. Resuming it from a query that asks for
something else — another `--label`, another `--search`, another `--source`, the other
`--direction` — is refused rather than answered, because every cursor inside it is a
position in the result set the original query returned, and picking up there under new
filters returns real rows from a walk you are not doing. Change the query, drop `--page`.

## Install

The command-line tool ships as a self-contained binary. Once a release is cut, install it
whichever way suits your machine:

```bash
cargo install onetaskgraph            # from crates.io
uv tool install onetaskgraph-cli      # from PyPI, no Rust toolchain needed
npm install -g onetaskgraph-cli       # from npm, no Rust toolchain needed
```

For Rust, the SDK surface is the engine crate itself rather than a separate wrapper
package. Add it when the application should link the engine, or add either subprocess SDK
when it should drive the installed binary:

```bash
cargo add onetaskgraph-core onetaskgraph-plugin-api serde_json
cargo add tokio --features macros,rt-multi-thread
uv add onetaskgraph-sdk               # Python
bun add @onetaskgraph/sdk             # TypeScript
```

The engine links its local sources — `in-memory`, `local-md` and `subprocess` — in every
build. The two that reach a network are features no default enables, so an application that
wants only a local store does not compile an HTTP and TLS stack it never runs: add
`--features github-projects,linear` (or either one) to the first line for GitHub Projects or
Linear. A configuration naming one a build left out is refused with the feature that
enables it.

This complete example constructs an engine over two in-memory sources, copies a task through
`Engine::copy`, inspects the outcome, and reads the destination back through the engine:

```rust
use onetaskgraph_core::{
    Config, CopyItems, CopyRequest, CopyScope, Engine, Environment, GlobalId, Secrets, StatusPolicy,
};
use onetaskgraph_plugin_api::SourceName;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_document(json!({
        "sources": {
            "drafts": {
                "plugin": "in-memory",
                "config": {"tasks": [{
                    "id": "T-1",
                    "title": "Ship the guide",
                    "content": "Publish the Markdown workflow.",
                    "status": {"category": "todo", "name": "Todo"},
                    "labels": [],
                    "metadata": {},
                    "repositories": []
                }]}
            },
            "work": {"plugin": "in-memory", "config": {}}
        }
    }))?;
    let secrets = Secrets::load(Environment::default())?;
    let engine = Engine::build(&config, &secrets);

    let report = engine.copy(&CopyRequest {
        items: CopyItems::new(vec!["drafts:T-1".parse::<GlobalId>()?])
            .expect("a copy names at least one item"),
        scope: CopyScope::Tasks,
        status_policy: StatusPolicy::Source,
        destination: SourceName::new("work")?,
        match_by: None,
        target_task: None,
        recreate: false,
        create: false,
        dry_run: false,
        comments: false,
        no_link: false,
        expect: None,
        statuses: Vec::new(),
        conditions: Default::default(),
    }).await?;

    let outcome = &report.items[0];
    assert_eq!(outcome.source.to_string(), "drafts:T-1");
    assert_eq!(outcome.destination().unwrap().to_string(), "work:T-1");
    assert_eq!(outcome.action.name(), "created");
    println!("{} -> {} ({})", outcome.source,
        outcome.destination().expect("the copy created a destination"),
        outcome.action.name());

    let copied = engine.task(outcome.destination().unwrap()).await?;
    assert_eq!(copied.items[0].item.title, "Ship the guide");
    println!("{}", copied.items[0].item.title);
    Ok(())
}
```

Unlike the Python and TypeScript SDKs, which spawn the compiled binary, a Rust consumer
links `onetaskgraph-core` and calls `Engine` in process. The engine and its copy semantics
remain the single implementation in either case.

A caller that holds one `Engine` for many units of work — a worker writing one settlement
back after another over a run that lasts hours — must call `engine.end_command().await`
between every two of them, after a unit fails as well as after one lands. Within a command a
source may reuse what it read: a GitHub Projects source holds the board, its search answers
and the item records it resolved, on the grounds that nothing but itself writes the board
while one command runs. Across units that stops being true, and without the call a person's
edit to an issue's body, or a card they moved, between two units would be overwritten or
mis-moved by the next write. After the call no source answers a read or bases a write on
item content, statuses, board contents or search results it held before; each keeps only
identifiers and vocabulary that stay valid — Linear keeps its team's workflow states and its
workspace's project statuses, so a warm status write is still one request. The caller names no
plugin: every source decides what it may keep under the one rule. A one-shot use — the binary,
or an engine built for a single unit and dropped — needs no call, because dropping the engine
drops everything it held.

```rust
use onetaskgraph_core::{Engine, EngineError};

async fn work_through(engine: &Engine, units: &[&str]) -> Result<(), EngineError> {
    for unit in units {
        // One unit of work: its reads and writes through `engine`.
        println!("settling {unit}");
        // Then, before the next unit, whether this one landed or not:
        engine.end_command().await?;
    }
    Ok(())
}
```

A failure reaches a linking caller as a typed value rather than as a document to parse: a
write that landed while a task it delivers could not be kept in step reports that entry as
`DeliveryOutcome::Failed`, whose `Failure` reads each member of the failure document —
`class()` among them — through the read-only getters its own API documentation lists.

To work on the repository instead, clone it and run `just bootstrap`; `just --list` shows
the rest.

## Configure

One YAML document, `onetaskgraph.yaml`, discovered upward from the working directory and
layered over a user-level file at `$XDG_CONFIG_HOME/onetaskgraph/config.yaml`:

```yaml
default_sources: [work, notes]   # omitted means every configured source
page_size: 50
output: text                     # text | json
interactive: true                # prompt for what a command was not given
sources:
  work:
    plugin: linear
    config: { api_key_env: LINEAR_API_KEY, team: ENG }
  notes:
    plugin: local-md
    config: { root: ~/notes/tasks }
```

A layer may refine a source another layer declares, carrying only its `routes`, one `config`
field or its `visibility` — the user-level file adding routes to a source a checkout's own
document declares, say. Where no layer read declares that source's `plugin`, the entry is a
refinement with nothing to refine: the source is not configured there, and everything else
loads. `config show` reports its settings with the layer each came from and lists it under
`incomplete_sources`; a command naming it — by `--source`, a qualified id, `default_sources`, or
a route it would follow — is refused, saying no layer declares its plugin; under `--json` every
such refusal, `cache clear` naming it included, is the failure kind `incomplete-source`.

A `linear` source's `project`, the id of one Linear project of the team, scopes the source to
that project: every read and listing is narrowed to it, a task written with no project is
placed in it, and a write to any project but that one is refused naming the scope. A status-only
write — `task status set`, or `task update` naming a status alone — is one mutation to the item
it names, wherever that item is filed, with no read before it.

```yaml
sources:
  example-team:
    plugin: linear
    config:
      team: ENG
      project: 986a467e-775a-4f8f-80dd-aca405063cf4   # optional
      status_mapping:
        backlog:     { task: Proposed,        project: Proposal }
        draft:       { task: Backlog,         project: Idea }
        todo:        { task: Todo,            project: Planned }
        queued:      { task: Queued,          project: Accepted }
        in-progress: In Progress
        unknown:     { task: Needs Attention, project: Blocked }
        done:        { task: Done,            project: Completed }
        cancelled:   Canceled
```

### A local cache of a remote source

A source reading a remote — a GitHub Projects board, a Linear team — can keep a **local cache**
of its tasks, projects and documents on this host, so a read the store already knows is
unchanged costs no request:

```yaml
sources:
  board:
    plugin: github-projects
    config: { owner: acme, project_number: 7, token_env: GH_PROJECTS_TOKEN }
    cache:
      enabled: true          # off unless set
      dir: .cache/board      # optional; a relative path is measured from this document
```

Both settings take every layer every setting takes: `ONETASKGRAPH_SOURCES__BOARD__CACHE__ENABLED`
and `__DIR` in the environment, `--set sources.board.cache.enabled=false` on a command line, a
later layer over an earlier one. Without `dir` the cache lives under
`${XDG_CACHE_HOME:-$HOME/.cache}/onetaskgraph/<digest>`, the digest naming the plugin and the
remote — the board's endpoint, owner and number, or Linear's endpoint and team — so every
process on the host configuring the same remote shares one cache: the command line, both
SDKs, and a Rust caller of the engine. A source with no remote, such as a folder of Markdown,
ignores the setting. Each task, project and document is one file, and so are a task's forward
and reverse edges and a project's forward edges read on their own, each written whole to a staging file and renamed into place, with no
lock; each holds the record's `version` and what it was read with under an entry schema
version, every kind in a directory of its own named with its schema, and a reader that does
not know that schema version, or cannot parse the entry, reads the record from its source
instead and never fails on it. Releases of different ages share the directory that way. An
image asset's bytes are kept once per SHA-256, under that digest, and served only while they
still hash to it.

**How large it grows, and how it is reclaimed.** An entry is filled lazily, by a load, and
replaced in place, so the item files hold at most one per record a load has ever read; byte
files hold one per distinct image ever read and are never replaced, so an image edited ten
times leaves eleven. Nothing expires by age and nothing caps the total. A board the GitHub cache
budget's size — a pass loading 5 projects, 20 documents and 20 tasks, 8 of the documents holding
a 256 KiB image — holds at most 45 item files and 2 MiB of bytes, plus 256 KiB for each image
replaced; at ten times that size, 450 files and 20 MiB. `onetaskgraph cache clear [<SOURCE>]`
empties one source's cache, or every source's, reclaiming its item and byte entries together and
removing only the files this store wrote there.

**The freshness rule.** The cache is keyed on the change token a task's `version` carries
(`docs/plugin-protocol.md` §4.13d), and it **never serves stale data**: a served entry is never
older than the version the caller pinned, and never a version the store cannot vouch for — with
one member named below, a comment author's display name. Its cost is held over an hour rather
than per read: one read may cost more than it did, and the hour sends fewer requests than no
cache. Freshness comes only from a token the store already holds — the
cache never asks a source whether something changed:

- **Within one command**, the version a listing, a search or a `--commented-since` result
  carries pins the reads that follow it in that command, so a `project copy`, a `task copy` by
  query, or an engine caller's `task_details` after a `tasks` listing reads only the tasks
  whose listed version is not already cached.
- **Across commands**, a caller pins a read with a version it already holds: `task show <ID>
  --pin VERSION`, `task comment list <ID> --pin VERSION`, `task show-many <ID>... --pin
  <ID>=VERSION` (repeatable), `task deps <ID> --pin VERSION`, `project show <ID> --pin VERSION`,
  `project deps <ID> --pin VERSION` and `document show <ID> --pin VERSION` — and a Rust caller with `Engine::pin`, `Engine::pin_project` and
  `Engine::pin_document`. A pinned read of a record the cache holds at that version or a later
  one sends **no request**; otherwise it makes the one read it always made, and refreshes the
  entry. **A pin means "at least as fresh as the version I hold", not "current"**: a pinned
  read answered from the cache can predate a change at the source nobody has read yet.
- An entry older than the pin — one a racing writer left — is never served, and neither is
  one the source cannot put in order beside it.
- An **unpinned** read by id, a plain `task show <ID>`, reads the source as it always did, and
  refreshes the entry.
- A listing — `task list`, `project list`, `document list`, a search — only pins: it never fills
  an entry. Nothing is pre-warmed.
- A part of a record the source's token does not cover is never served, and neither is a
  record with no token — a GitHub item changed within the minute before its read, a draft, or a
  Linear item whose labels, comments, relations or inverse relations came back as only a first
  page, which with the cache on is a task or a project with more than ten relations or
  dependents, and a task with more than ten comments. The read goes to the
  source and says why.

**What each source's token is, and what it covers.** `docs/plugin-protocol.md` §4.13d states
both tokens part by part with the live evidence for each, and §4.13e every read with the reason
each remote one is remote.

- A **GitHub** item's token is `gh3`: the issue's and the board item's times, its comment count,
  its relationship events, a digest of its *facets* — its own kind, its board field values and
  its labels by name and colour — and its repository. The facets are what `gh2` missed, measured
  on a real board: a label renamed or recoloured and a field option renamed left it equal, a far
  end deleted made it order the new token *earlier*, and a task gaining a sub-issue left it
  equal. It covers a task's fields and comments, a project's fields, and a document's fields
  and its image manifest. It does not cover forward edges, because an item's token cannot see a
  far end become a document or be deleted, so every dependency read goes to GitHub.
- A **Linear** item's token is `lin1`: its `updatedAt` and a digest of its facets — for a task its
  identifier, labels, project, state and url; for a project its labels and status; for a document
  its content's digest, project, title and url — and, with the cache on, digests of first pages:
  a task's comments (each one's id and update time), relations and inverse relations, and a
  project's relations and inverse relations. Each part is what `updatedAt` was measured missing on a real team: a label
  renamed or recoloured and a workflow state renamed (3 of 3), a relation added (3 of 3), a
  comment added, edited or deleted (0 of 3 moved it), a project relation added or removed (0 of
  2), and a document's content edited (4 of 4). With the cache on it covers a task's fields,
  comments and edges both ways, a project's fields and edges both ways, and a document's
  fields.
- **A Linear comment's author is the one member served older than its token.** A person renaming
  themselves moves no part of any token, so a comment served from the cache names its author as
  they were named when the entry was filled; its id, body, times and the set of comments are
  never older than the pin. Renames are rare, and the name labels a person rather than work, which
  is the user's ruling of 2026-10-14.

**What a plugin tells the cache, and how a dependency read fills it.** Through `TaskSource`, and
never over the stdio plugin protocol: `attach_cache`, called once when the cache wraps the source
and before any read, which is what lets Linear widen the queries below; `forward_edges_version`
and `edges_version`, defaulted to none, the token the response that answered an item's last
dependency read in a direction spelled; and `reverse_edges_coverage`, defaulted to uncovered. A
Linear task's and project's forward and reverse edges are each kept in an entry
of their own keyed by that version, served only to a pin at that version or a later one — so
`project graph` lists a project's tasks and the next run answers their dependencies from the
cache, sending no dependency request while the listed tokens are unchanged — and never vouch for
the item's own fields. A GitHub board keeps the defaults: its edges are always read.

**Turning the cache on widens seven Linear queries.** `ISSUES`, `ISSUE` and `ISSUE_DETAIL` gain
the comments, relations and inverse relations pages the token is read from, `PROJECTS` and
`PROJECT` the project relations and inverse relations pages, and `ISSUE_RELATIONS` and
`PROJECT_RELATIONS` the token's other parts, so a dependency read can fill an entry; a write's
own read of its target sends the plain query. A copy filing items under two or more of a caching
source's projects reads one page of that source's projects first, which pins them, so the
filing lookup's read of each is answered from the cache while unchanged — at most one request
over reading each, and only when that page pins none of them. With the cache off every query is the one 0.6.0 sent. On GitHub nothing is widened,
and `github-cache-points-not-above-uncached` holds the cache to adding no point at any kind,
sequence or scale.

Every read from a caching source carries `cache: {"served": "cache" | "remote", "bypassed":
null | "<reason>"}` in its JSON, and a source keeping no cache carries no such member. A
command's `spent` counts only the requests it really sent.

**A write keeps the cache current from its own response** — `task comment add`, `edit` and
`delete`, a copy onto the caching source, and `task metadata`, `status`, `priority`, `update`
and `content`. A write to a project or a document — a create, an update, a metadata write, a
render, an asset write, a deletion, a copy's rollback — takes every entry naming it out of
service and fills none, so the next pinned read reads it. It advances the entry only to what that response vouches for: the whole task,
comments included, at the version the response determines. A response showing only the fields
it wrote leaves the entry unserved, because a person's concurrent edit to another field, or a
new comment, can share that version; so does one that determines no version. The entry is
taken out of service before the write is sent, so no other process is served it once the write
lands; a cache directory that will not let it be removed, emptied or made writable refuses the
write, naming the directory, before anything is sent.

**`--expect VERSION`** on a write says the caller knows the task at that version: the write
takes the task's node id, the board's field and option ids and the current values it compares
against from the cache, at that version or a later one, and sends **only its mutations** —
each field it changes and nothing else, so a person's concurrent edit to a field it does not
set is never touched. Its JSON carries `result: {"version", "parts": [{"part", "outcome",
"fields"}]}` built from the mutations' own responses, each part `landed`, `rejected` with the
backend's error, or `unknown` when its answer was lost — never sent again, because that could
apply it twice. When the cache does not hold what the write needs, it reads once and then
writes, and says so as `fallback: {"read": true, "reason": …}`, counted in `spent`. **It is
not a conditional write**: neither GitHub nor Linear takes a precondition, so a person's edit
to a field the write *does* set, made after the caller's read, is overwritten. The one request
the cache can add is the first write after a board option it kept was renamed or deleted: the
refused mutation, one read that refreshes the board's kept ids, and that part once more.

On `document metadata set`, **`--expect VERSION` is the document's version, and it is a
precondition**: the document is read first, and a read answering any other version — a change
made on the board since the caller's read, whether or not anything has read it since — refuses
the write, naming both versions and writing nothing. At that version the write follows in one
more request, taking what the read answered rather than reading again: two requests, and no
cache needed. Neither GitHub's `updateIssue` nor its REST API takes a precondition, so the read
is what holds the version; a change landing in the moment between the read and the write is the
one still overwritten.

The cache is an efficiency layer and never a snapshot: it serves the pinned version or a later
one, keeps one entry per record, and is shared and cleared freely. To keep an exact version of a
task, copy it into a folder of Markdown of your own with `task copy`.

### Which owners a GitHub Projects source creates issues for

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-github-projects-e2e/tests/e2e/creation_owners.rs`, which drive every
     road and the routed and locate cases against the loopback board. -->
A `github-projects` source's `creation_owners` is a list of GitHub logins whose repositories a
create may file its issue in; empty or absent, the default, restricts nothing and is left out of
`config show`. Set, a create the creation rule would file in a repository of another owner —
compared ignoring case, as GitHub compares logins — is refused before anything of the copy is
written, its intent to create included, with the failure kind `creation-owner`, naming the
repository and the allowed owners: by `task copy`, `task copy --create`, a member create of a
project copy, and a dry run of each. An update of an issue that already exists is never refused
by it, and a routed copy is held to the setting of the destination it was placed at. An entry
that is not a GitHub login is refused when the configuration loads, naming it. `task locate`
reports such a create as `creates_in.refused` rather than failing.

```yaml
sources:
  board:
    plugin: github-projects
    config:
      owner: example-org
      project_number: 7
      repository: example-org/store
      creation_owners: [example-org]
```
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### What a GitHub Projects source's sub-issues mean

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the `relations`
     journeys of `crates/onetaskgraph-github-projects-e2e`, which drive both meanings and the
     refusal of two sources sharing a board against the loopback board. -->
A `github-projects` source's `sub_issues` says what an issue's sub-issues are. `project`, the
default and the meaning every board had before this setting: an issue with sub-issues reads as a
project and its sub-issues as that project's members. `parent`: every issue reads as a task, a
sub-issue is a child of its parent under `task parent set-many` and `task relations`, and every
project write to the source is refused naming the setting. On a `project` source `task
relations` reports a member as having no parent: the two meanings never mix on one source.

**A `parent` source needs a board of its own.** On one board the two meanings answer differently
for the same issues, and an edge written from either side rewrites the other's view, so two
configured sources naming one board with different `sub_issues` are refused when the
configuration loads, naming both. No configuration check can see what GitHub does on its own
side: an issue of a `parent` source added to a board read with the default meaning reads there as
a project, and GitHub's default `Auto-add sub-issues to project` workflow pulls its sub-issues
onto that board too.

```yaml
sources:
  follow-ups:
    plugin: github-projects
    config:
      owner: example-org
      project_number: 9
      repository: example-org/store
      sub_issues: parent
      link_types:
        caused-by: {native: relates-to}
```
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

### Status mapping

A `github-projects` or `linear` source names its statuses with one `status_mapping`, the same
grammar on both. Its keys are status categories — `draft`, `backlog`, `todo`, `queued`,
`in-progress`, `done`, `cancelled`, `unknown` — and each value is one of:

- **a name**, which is the category's name for a task and for a project alike;
- **`null`**, which disables the category for every kind;
- **an object with the keys `task` and `project`**, each optional and each a name, which names
  the category per item kind; a kind the object leaves out leaves the category unmapped for
  that kind.

```yaml
status_mapping:
  todo: Todo                                 # every kind
  draft: null                                # disabled for every kind
  done: { task: Done, project: Completed }   # per kind
```

Anything else is refused when the configuration loads, naming the source, the category and the
part: a key that is not a category, an object naming no kind (`{}` — write `null` to disable a
category), naming another key, holding a `null`, or a blank name. So are two categories mapped
to one name of one kind, compared ignoring case — that name could read back as only one of them
— naming the source, the kind, both categories and the name; a bare name counts for both kinds,
and one name used by different categories in different kinds is fine.

**Writes.** A status is written as the name its category maps to for the kind of the item being
written. A write the source has no name for — a category it does not map, one set to `null`, or
one its per-kind object leaves out — is refused before any mutation, naming the source, the kind
(`task` or `project`), the category and the key to set, `status_mapping.<category>.<kind>`. So
is a mapped name that kind's vocabulary does not hold, naming the name. Nothing falls back by a
status's type, or by the name it was called where it came from.

**Reads.** An item at a name its kind maps reads as that category, under the name; every other
name reads as `unknown`, under its own name. `--status` returns exactly the items that read as
each category it names, `unknown` among them.

**On a GitHub Projects board** both kinds' names are options of its one `Status` field. A
category the source does not mention keeps its shipped default for both kinds — `backlog` →
`Backlog`, `todo` → `Todo`, `queued` → `Queued`, `in-progress` → `In Progress`, `done` → `Done`,
`cancelled` → `Cancelled`; `draft` and `unknown` have none — and a category it does mention uses
only what it configures. `done` and `cancelled` close the issue as completed or not planned,
for both kinds.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Held by the journeys in
     `crates/onetaskgraph-github-projects-e2e/tests/e2e/closed_status.rs`, which close each item
     on the loopback board and read it through `task show`, `task list --status` and a copy into
     a folder of Markdown, and by `a_closed_issue_reports_the_closed_category_and_its_column_name`
     in the plugin's own `tests/plugin.rs`, which reads every reason. -->
**A closed issue reads as its closure says, whatever its card.** However it closed — by hand, by
a change request merged with a closing line, by a landing's issue-closing line — what any reader
of it sees is only its state and the reason it was closed with, so those decide:

- **Category:** closed as completed, or with no reason, reads `done`; closed as not planned or as
  duplicate reads `cancelled`; any other reason reads `unknown`.
- **Name:** the card's own option, when the mapping reads that option as the same category or
  does not name it at all — an issue closed as completed in a `Shipped` column reads `done`
  named `Shipped`. Otherwise — the card's option reads as another category, or there is none —
  the option the mapping gives the category: an issue closed as completed while its card still
  says `Todo` reads `done` named `Done`, so no destination that reads `Todo` as todo is ever
  handed it. An `unknown` closure keeps its option, or `Closed`.

An open issue reads by its option alone, as above. `task status reconcile` brings the cards
themselves in line; see [below](#when-an-issue-closes-reconciling-and-completing-its-card).
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

**On Linear** a task's names are the configured team's workflow states and a project's are the
workspace's project statuses — two vocabularies. Linear has no built-in names, so a source
without a `status_mapping` reads every item as `unknown` and refuses every status write. A
source resolves its team, the team's workflow states and the workspace's project statuses in
one request, the first time a write or `sources fields` needs them, and holds them for its own
lifetime — never longer, and never shared with another source. A name it does not find is
looked for once more in a fresh read, so one added in Linear since is found; nothing a failed
call answered is held. A status write sends its mutation alone, with no read of the issue
first — one request once the resolution is held. `sources fields <SOURCE> --apply` creates every
mapped name a vocabulary lacks — a workflow state on the team for a task's, a project status in
the workspace for a project's, both for a bare name — of the type its category derives, in
Linear's neutral grey, a project status after the workspace's last; it never renames, retypes
or deletes a name that is there, and reports one present under another type as it is:

| Category | Workflow state | Project status |
| --- | --- | --- |
| `backlog`, `draft` | `backlog` | `backlog` |
| `todo`, `queued` | `unstarted` | `planned` |
| `in-progress`, `unknown` | `started` | `started` |
| `done` | `completed` | `completed` |
| `cancelled` | `canceled` | `canceled` |

A create Linear refuses stops the run, naming the name and its kind, and the report names every
name created before it; run `--apply` again once the refusal is lifted, and it creates only what
is still missing.

**Migrating to 0.3.** This grammar changes what an existing configuration does:

- On Linear, a write no longer falls back to the team's first state of the category's type, or
  resolves a category by the status's own name: a category with no mapped name is refused.
- On Linear, a project write resolves its status through the project side of the mapping, and
  no longer through the item's own status name.
- On Linear, a name the mapping does not give its kind reads as `unknown`, and no longer by
  its `WorkflowState.type` or `ProjectStatusType`.
- A Linear source without a `status_mapping` refuses every status write.
- On GitHub Projects, a category configured as a per-kind object no longer gets the shipped
  default for the kind it leaves out.
- On Linear, a status write no longer reads the issue first. Setting the category an issue
  already reads as sends a same-state update; setting `unknown` on an issue at a name the
  mapping does not give a task moves it to the mapped `unknown` name; and an issue Linear does
  not hold is reported from the mutation's own not-found refusal.
- On Linear, a status-only write through a source scoped to one project (`project` set) goes
  to the item it names wherever that item is filed, while reads, listings and creation stay
  scoped.
- On Linear, `sources fields --apply` creates every missing mapped name, as workflow states and
  project statuses, where it used to be refused.

Every setting is reachable at three layers, lowest precedence first: **the file, then the
environment, then a command-line flag.**

An environment variable is `ONETASKGRAPH_` followed by the config path, each segment
upper-cased with `-` replaced by `_` and segments joined by a double underscore; a list is
comma-separated:

| Variable | Sets |
| --- | --- |
| `ONETASKGRAPH_PAGE_SIZE=100` | top-level `page_size` |
| `ONETASKGRAPH_DEFAULT_SOURCES=work,notes` | top-level `default_sources` |
| `ONETASKGRAPH_INTERACTIVE=false` | top-level `interactive` |
| `ONETASKGRAPH_SOURCES__WORK__CONFIG__ROOT=/tmp/tasks` | the `root` of the source named `work` |
| `ONETASKGRAPH_SOURCES__GH_MAIN__PLUGIN=github-projects` | the plugin of the source named `gh-main` |

The mapping is unambiguous because a source name may not contain an underscore.

On the command line the same dotted path is `--set`, and a few common settings have named
flags of their own:

```bash
onetaskgraph config show --set sources.work.config.root=/tmp/tasks
onetaskgraph config show --page-size 100 --default-sources work,notes --json
onetaskgraph config show --no-interactive       # or --interactive; they conflict
```

`onetaskgraph config show` is what makes precedence something you can see rather than
something you have to reason about: it prints every setting, its value, and the layer it
came from — which file, which environment variable, or which flag — and `--json` renders
the same thing for a script.

![A terminal showing every effective setting in three columns — the dotted path, the value, and the layer it came from: one row naming the environment variable that set it, one naming the command-line flag, one reading `default`, and the rest naming the configuration document, with the secrets file underneath](docs/screenshots/config-show.svg)

That third column is the whole point: `default_sources` above came from an environment
variable, `page_size` from a flag that outranks the document's own value, and every
`sources.*` entry from the document, named by path.

### Relative paths in a configuration document

**A relative filesystem path a configuration document supplies is resolved against the
directory holding that document.** `onetaskgraph.yaml` is discovered by walking *upward*
from the working directory, so the document is very often not in the directory you ran the
command in — a `local-md` `root:` of `plans` in a checkout's own document means that
checkout's `plans`, whether you run from the checkout, from a crate three levels inside it,
or from a worktree beside it.

**A relative path the environment layer or a flag supplies keeps resolving against the
process working directory**, because there is no document to rebase it on:
`ONETASKGRAPH_SOURCES__PLANS__CONFIG__ROOT=plans` and `--set
sources.plans.config.root=plans` both mean `plans` under wherever the command is running.
Naming the directory outright is still how a launcher points a run at a store that is
neither beside its own document nor beside its working directory:

```bash
ONETASKGRAPH_SOURCES__PLANS__CONFIG__ROOT=/srv/plans onetaskgraph task list
```

`onetaskgraph config show` reports the resolved path beside the document that supplied it,
so what a run will really read is something you can see rather than something you have to
work out.

The rule reaches only the configuration fields a plugin itself declares as paths, and each
plugin's own page says which of its fields those are. It holds across the `subprocess` seam
too, so a `local-md` root relative to its document names the same directory in process and
behind the seam: the engine rewrites nothing in a `settings:` block, and the child learns
which document's directory to measure its declared fields from through the handshake.
[`docs/plugin-protocol.md`](./docs/plugin-protocol.md) §3.8 is the one statement of that
member, including when it is absent and what a plugin written before it does.

### Credentials

A configuration document never holds a credential — it names the environment variable that
does (`api_key_env: LINEAR_API_KEY`). Before resolving sources the CLI reads
`$XDG_CONFIG_HOME/onetaskgraph/secrets.env` (override with `ONETASKGRAPH_SECRETS_FILE`) as
`KEY=VALUE` lines with `#` comments, and applies each value **only where the process
environment does not already define that name** — so anything you exported wins. A missing
file is not an error. A credential is never printed, never in debug output, and never in a
log line.

There is exactly one name per credential everywhere — in that file, in the configuration,
in the documentation and in CI: `LINEAR_API_KEY` and `GH_PROJECTS_TOKEN`. Nothing anywhere
translates between spellings.

## Routing, and plans that span sources

A source may send what is written to it somewhere else, chosen by the repositories the item
concerns. `routes` sits beside `plugin` and `config` — it is the engine's, not the plugin's:

```yaml
sources:
  plans:
    plugin: github-projects
    config: { owner: nickderobertis, project_number: 2 }
    routes:
      - repositories: ["github.com/example-org/*"]
        to: example-team
  example-team:
    plugin: linear
    config: { api_key_env: LINEAR_API_KEY, team: ENG }
```

**An entry matches an item when every one of the item's repositories matches at least one of
its patterns.** A pattern is a normalized origin, `host/owner/name`, where `*` matches exactly
one whole segment. An item naming no repository matches nothing. Among entries of one kind the
first that matches wins, and an item no entry matches stays in the source itself. A
configuration with no `routes` writes exactly where it always did.

An entry comes in three kinds: `{repositories, to}` above; `{classification, to}`, matching
every item of that classification (see [Routing by
classification](#the-public-boundary)); and a **combined** entry naming both, which matches an
item of that classification every one of whose repositories matches one of its patterns — and,
like a repository entry, never an item naming no repository:

```yaml
routes:
  - repositories: ["github.com/example-org/special"]
    classification: private
    to: example-special
  - repositories: ["github.com/example-org/*"]
    classification: private
    to: example-private
  - classification: private
    to: vault
  - repositories: ["github.com/example-team/*"]
    to: example-team
```

**The kind decides the order, wherever an entry is written.** Among the entries an item may
try, combined entries are tried first, then classification entries, then repository entries,
each kind in the order written, then the source itself. A private item may try only entries
whose `to` is declared private, or names a source no layer gives a plugin; when none places it,
it stays in the source itself — and is refused there when that source is not private. A
placement still names the entry by its index as written. So:

- **A private repository is sent elsewhere by a combined entry naming it, written above the
  organization-wide one** — above, `example-org/special`'s private work goes to
  `example-special` and the rest of `example-org`'s private work to `example-private`.
- **A public repository is sent elsewhere by a repository entry**, which a combined private
  entry never shadows: a public item of `example-org` is placed by `{repositories:
  ["github.com/example-org/*"], to: …}` wherever that entry is written.

Each of these is refused when the configuration loads, naming the source and the entry: a
`to` naming no configured source, a `to` naming the source itself, a `to` naming a source
that has routes of its own (a route never chains), an entry whose `repositories` is empty,
and a malformed pattern — for a repository entry and a combined entry alike — and a
`classification: private` entry, combined or not, whose `to` is not declared private.

`routes` is set at every layer. A document writes it as a list; `--set` and the environment
address one entry by its index, because neither can spell a list of objects:

```bash
onetaskgraph --set 'sources.plans.routes.0.repositories=github.com/example-org/*' \
             --set sources.plans.routes.0.to=example-team  task copy plan:T-1 --to plans
ONETASKGRAPH_SOURCES__PLANS__ROUTES__0__REPOSITORIES='github.com/example-org/*' \
ONETASKGRAPH_SOURCES__PLANS__ROUTES__0__TO=example-team  onetaskgraph sources route plans
```

A combined entry is spelled the same way, one field at a time —
`--set sources.plans.routes.0.classification=private` beside its `repositories` and `to`, or
`ONETASKGRAPH_SOURCES__PLANS__ROUTES__0__CLASSIFICATION=private`. A comma in a `repositories`
value makes it a list of patterns, as a comma does everywhere else. Entries are
numbered from 0 with no gaps, and an entry one layer sets replaces the whole `routes` a lower
layer set rather than merging into it. `config show` prints `routes` with the layer it came
from — the document's list as one row, a flag's or a variable's entries one row per field.

`onetaskgraph sources route <SOURCE> --repository <R>...` answers where an item would go —
`{"source": "plans", "destination": "example-team", "route": 0, "reachable": ["plans",
"example-team"]}`, with `route` null when no entry matched — from configuration alone;
`--classification private` asks for a private item, which combined private entries place.
`reachable` is every source an item written to SOURCE could land in, whatever its
repositories and classification: SOURCE itself first, then each source its routes name as
`to`, in the order they are written, each once. It is the set a routed copy reads ahead from.

### Which writes route, and where each item lands

A routed copy is held to the destination it places each item at: its `creation_owners`, its
`create_quiescence_seconds`, and the intent to create the item records naming that destination.

`task create <SOURCE>`, `task copy --to <SOURCE>`, `project copy --to <SOURCE>` (with or
without its tasks, and with `--member`) and `document copy --to <SOURCE>` all place each item
by the named source's routes. So does the engine's own member copy, which is what a running
engine's write-back uses.

- **A task** goes where its own repositories and classification route it.
- **A project's home** is placed by its **voters**: every task the copy carries that names at
  least one repository, and the project's own `repositories` when it names any. A task naming
  no repository, and every document, never votes.
  - When every voter places at one source, the home goes there — so a plan of tasks routed
    to one source, plus a person's step naming no repository, lands wholly in that source.
  - When the voters disagree, the home is the named destination, with a member project for
    each routed task — unless the [public boundary](#the-public-boundary) is active and any
    item of the plan classifies private. Then the home goes to the source declared private
    that the plan's private voters place at, by their own repositories and the placement a
    private item takes. When those voters place at two different private sources and the
    plan has no home yet, the copy is refused before anything is written, naming each source
    and the qualified ids of the items placing there. Every task of the plan then lands
    canonically beside that private home, and each public voter is seen where its own
    repositories place it only through a neutral **public projection** — see [Public
    projections of a private plan](#public-projections-of-a-private-plan).
  - With no voter at all, the home is the named destination. With `--no-tasks` only the
    project's own repositories can vote.
  - A `{classification: private, to: <source>}` entry keeps its precedence: a project holding
    anything private goes there whole, with every task and document in it, its public tasks
    projected where their own placement is. A combined entry never does this — it matches no
    item naming no repository, and a project is asked whole — so without a classification-only
    entry the home goes by its voters, its private voters placed through combined entries.
  - **A home that already exists stays where it is**, found by the project's link or origin
    and naming it back: a task added later that routes elsewhere goes into a member project
    there. A private plan whose existing home sits in a source not declared private is refused
    before anything is written, while the boundary is active; the home is never moved.
- **A task naming no repository**, carried by a project copy, goes with its home, wherever the
  home went; its own placement is never consulted.
- **A task routed away from its home's source** goes into the home's **member project** in
  that source, found through the home's `onetaskgraph.members`, or else as the project there
  whose `onetaskgraph.member_of` names the home — or created, with the home's title and status
  and `onetaskgraph.member_of` naming the home, and recorded on the home in the same copy. A
  member in a source not declared private, of a home in one that is, records no
  `onetaskgraph.member_of`: the home's own list is what ties them. A home has at most one
  member per source. `task create` files a routed task the same way, under the member of the
  project it names.
- **A document** goes with its project's home.

**A copy never creates a second project of one plan in a source.** When a copy would create
a home or a member project in a source that already holds this plan's project there — the
member the plan's counterpart home names in its `onetaskgraph.members` for that source, or a
project whose `onetaskgraph.member_of` names that home — it uses that project instead. The
counterpart home is the project the copied project's `onetaskgraph.copies` links to in another
source, naming it back as its origin — or naming none, where a copy out of a private source
withheld it. Taken
over as the home, it drops `onetaskgraph.member_of` and records `onetaskgraph.origin` naming
the copied project, the copied project's `onetaskgraph.copies` records the link to it as any
copy's link is recorded, and the former home's `onetaskgraph.members` no longer names it; its report says `via: scan`. So
copying a plan `--to` the source its routed tasks went to, after a routed copy gave it a home
elsewhere, leaves that source holding one project of the plan, carrying every routed task. No
key is renamed, and none is removed from an item the copy does not write.

A task's project is still a native id of its own source: a plan spanning two sources is a
home plus a member project in the other, never a project holding another source's tasks. An
edge between two items that land in different sources is recorded as a cross-source edge on
its near end, either way round.

Every outcome of a routed copy's report carries `placed`: the source it landed in and the
index of the route entry that placed it, or `null` — a dry run's included, so a would-be
create says where it would go. A copy into a source with no routes reports no `placed`, and
its output is what it always was. A copy across several sources is still complete or never
happened: its undo covers every source it wrote. The `onetaskgraph.copies` link a copy
records is keyed by the source the item really landed in.

**An item whose counterpart already sits in another source than the one it routes to now is
refused before anything is written**, naming both: a repository changed after a copy is a
person's decision about where the work lives, not a silent move. What finds a counterpart
there is its link or its origin; a counterpart neither names is looked for only in the
source the item routes to.

### Reading a home with its members

```bash
onetaskgraph task list --project plans:42 --members
```

reads the home's tasks and the tasks of every project its `onetaskgraph.members` names, each
under its own source, paged across all of them as one walk. The members are read off the home
on every request — nothing about them is kept — and a member that cannot be read is reported
in the response's errors, as any source failure is, never left out in silence. `project show`
of a home prints its members, and of a member its home. The SDKs spell it
`task_list(project=..., members=True)` and `taskList({ project, members: true })`.

`project copy` of a home reads its tasks the same way, so the home copied on carries every
member's tasks with it, and the destination's own routes file each one — under the home there,
or under the member they create or find.

### Where a copy would land: task locate

```bash
onetaskgraph task locate drafts:run-1/root-cause --to example-store --json
```

answers, for each task named, what `task copy --to example-store` of it would do, and writes
nothing anywhere:

```json
{
  "items": [
    {
      "source": "drafts:run-1/root-cause",
      "classification": "private",
      "destination": "example-private",
      "route": 0,
      "counterpart": {"id": "example-private:ABC-12", "via": "scan"},
      "carriers": ["example-private:ABC-12", "example-private:ABC-9"],
      "creates_in": {"repository": "github.com/example-org/store"}
    }
  ],
  "spent": {"requests": 2, "budgets": []}
}
```

**It and `task copy` share one placement and one lookup.** Each task is read from its source
and classified exactly as a copy classifies it before a write — its own recorded classification
and, while the [public boundary](#the-public-boundary) is active, each of its repositories put to
the `write_policy`'s `visibility_command`. A source declared private withholds its tasks'
provenance and does not by itself classify them private, for either verb. The task is then
placed through the very function a copy places through, over `--to`'s routes, and its
counterpart there is looked up by the copy's own rules in the copy's order: its
`onetaskgraph.copies` link read by id, then its own `onetaskgraph.origin` read by id, then the
destination's origin query — run only when neither answers, and read to its last page. So a
locate and the copy after it cannot disagree, and every refusal that lookup makes before a copy
writes is made here with the copy's own error kind, message and exit code: a stale link or
origin, a counterpart sitting where the task no longer routes, and a private task bound for a
source not declared private. `--match-by` and `--target-task` are not taken; `--target-native`
below is the mirror of the second.

- `destination` and `route` are where the copy would write the task and the index of the
  route entry that sent it there, `null` when none matched, as a copy's `placed` says them.
- `counterpart` is the item the copy would update and the rule that found it — `link`,
  `origin` or `scan`, as a copy's `via` spells them — or `null` when the copy would create one.
- `carriers` is every destination item the origin query found carrying the task's origin, in
  listing order, or `[]` when the query did not run; when it ran, `counterpart` is the first of
  them, which is the one a copy picks.
- `creates_in` is the repository a create there would file the task's issue in, whether or not
  a counterpart exists — an existing issue is never moved — and `null` where the destination
  files no issue in a repository. It carries `refused`, the refusal's message, only when the
  destination's `creation_owners` would refuse a create there; the locate does not fail for it.
- `spent` is as a copy reports it, and absent when no source meters its requests.

**In a private destination, a counterpart is found by the origin query.** The store writes no
`onetaskgraph.copies` link naming a private landing on a source not declared private, so a task
of a public source copied into a private one has no link to follow: the next locate, and the
next copy, find it by the destination's own record of where it came from, and answer it `via:
scan`.

`--no-counterpart` looks no counterpart up: no link, origin or origin-query read at the
destination, and no `counterpart` or `carriers` in the answer. The one destination read it may
make is that of the project issue a task in a project would be filed under, which
`creates_in` needs.

`--target-native NATIVE-ID` is how to locate a task whose counterpart the caller already knows
— the general mirror of `task copy --target-task`. It is taken beside exactly one id. The task
is placed as usual, and its counterpart is the item with that native id where it is placed,
read by id and verified by its `onetaskgraph.origin` naming the task back; the task's link, its
origin and the origin query are not followed, so a stale or re-pointed link neither refuses nor
misleads. The answer is that item `via: target-task` with `carriers: []`; an item missing there,
or one not naming the task back, is refused as `task copy --target-task` refuses it. Beside
several ids or beside `--no-counterpart` it is refused before anything is read.

What a locate never does is what a copy does only to write: it never runs the boundary's
`check_command`, never reads a destination's live visibility but the one read `creates_in` needs
on a GitHub board declared private (below), and reads no asset and no forward edge. A project or a document id is refused, naming it.

`creates_in` is the destination's own answer, through `TaskSource::creation_repository`, a
defaulted method of the plugin contract answering `None` — no repository — for every source that
does not say otherwise. A GitHub Projects source answers it from the same rule its create
applies: a task naming one repository is filed there — on a source declared private only when
that repository reads private; one naming none or several is filed in its parent project issue's
repository, else in the source's configured `repository`; and a target the create would refuse —
a repository of another owner than its parent issue's, a parent the board does not hold, no
configured repository — is refused the same way. It reads the parent project's issue only when
the task is in a project. Linear, a folder of Markdown and an in-memory source answer `null`.

So a locate reads one live visibility in exactly one case: a task naming exactly one repository,
located for a GitHub Projects source declared private, has that repository's visibility read, at
most once per destination for the whole locate, because that read is what decides where the
create files its issue. Every other locate reads none.

The SDKs spell it `task_locate(ids, to=..., no_counterpart=..., target_native=...)` and
`taskLocate(ids, { to, noCounterpart, targetNative })`; a refusal's failure document, with its
`kind`, is on the error each raises, as `failure`.

## The public boundary

Private work may inform public work, but nothing of it may reach a public destination. Three
things decide where an item may be written, and one decides what it may say there.

**Who may read an item: its `classification`.** A task, a project and a document are each
`public` or `private`. A folder of Markdown writes `classification: private` in the front
matter and nothing while it is public; a hosted source keeps it under the reserved metadata key
`onetaskgraph.classification`. An item written before it existed reads as public. It only ever
tightens: an item naming a repository that is not public is private, a document filed under a
private project is private, and an explicit `public` changes neither — a task's project never
tightens it (see below). A project is the strictest
of its own classification, its tasks' and its documents', and a store writes it so — so a
project stays private after the members that made it private are gone, and a copy never writes
an item looser than the destination already holds it. `task create`, `document create` and
`project create` take `--classification public|private`.

**Who may read a source: its declared `visibility`.** `visibility` sits beside `plugin` and
`config` and is `public`, `private` or `unknown`, the default. A private item is written only
to a source declared `private` **whose backend agrees at that write**:

- a folder of Markdown and an in-memory source are on this machine, so declaring one private
  declares a host-local destination;
- a GitHub board is private only while its Project and the repository the issue lives in are
  both private. Each is read with the source's own `token_env` credential, and a copy reads
  each destination — the board, and each repository it reads — **once for the whole copy**,
  all before its first write, and holds those reads for every write it makes; the next copy
  reads afresh, so a destination whose visibility changes during one copy is seen by the next
  copy, never by the one already running. A linking process holding its sources for hours reads
  afresh at each copy, and so does an interrupted copy run again. Any other write reads its
  destination before it is sent. Reading a Project's visibility needs the `read:project`
  scope, and a credential without it is refused naming the scope;
- a Linear workspace is readable by its members alone, which no setting changes, so a Linear
  source is verified once per source instance: the workspace its credential reaches is read
  inside the team and status read its first write sends anyway, and later writes send nothing
  more. A Linear source with no `team` cannot be verified and is refused;
- a source hosted over the stdio plugin protocol cannot answer, and is never private.

A visibility that cannot be read is never guessed: the write is refused.

**Where a GitHub board declared private creates an issue.** A GitHub Projects source files a
new issue — for a task, a project or a document — in the item's one named repository, else in
the repository of the project issue it is filed under, else in the source's configured
`repository`. On a source declared `visibility: private` the first arm is read first: an item
naming exactly one repository has that repository's visibility read, with the source's own
credential and once per copy, and is created there only when it reads private. One reading
anything else decides nothing, and the issue is created where an item naming zero or several
repositories is — its project issue's repository, else the configured one; a read that cannot
be answered refuses the write, naming the repository, and takes no fallback. The item still
names its repository: every read of it answers `repositories: [<that repository>]` while its
issue lives in the private one, and a later copy or update keeps the issue where it was
created — nothing moves or transfers an issue. What a write demands is unchanged: the board and
the repository the issue is actually created in must both read private, so a private source
whose fallback repository reads public still refuses such an item, and an existing issue
already in a public repository is refused where it is. Every source not declared private, and
every item naming a repository that reads private or naming zero or several, is placed exactly
as before.

**What it may say: `write_policy`.** A write to a destination not verified private is put to
the caller's check, and a repository's visibility is the caller's answer too. The store derives
no term and asks no host about a repository itself: it runs the two commands `write_policy`
names, each handed one JSON document on standard input.

```yaml
write_policy:
  visibility_command: [onevcs, boundary, inspect, --input, "-"]
  check_command: [onevcs, boundary, check, --destination, public, --input, "-"]
sources:
  plans:
    plugin: github-projects
    config: { owner: example-org, project_number: 2, repository: example-org/plans }
    visibility: public
    routes:
      - classification: private
        to: private-plans
  private-plans:
    plugin: local-md
    config: { root: ~/private-plans }
    visibility: private
```

`visibility_command` is handed `{"repository": "github.com/owner/name"}` and answers
`{"visibility": "public" | "private" | "unknown"}`; no command, a failure, or an answer outside
onevcs's versioned boundary schema is `unknown`, which is private. `check_command` is handed
the write — `{"destination", "text", "paths", "metadata", "scope"}`, titles, labels and
metadata values among `metadata`, content among `text`, asset names and recorded template
references among `paths` — and **passes it only by exiting 0 with `{"verdict": "pass"}`**. A
refusal names where the check stopped and never what it found; no command, a failure, or any
other answer is unavailable, which never passes.

**A copy is screened as one batch.** A copy composes every write it would make — a project's
tasks and, with `--documents`, its documents among them — and hands `check_command` **one**
input for the whole copy before it writes any: its `text`, `paths` and `metadata` each the
multiset union of what each write would have been screened as alone — a value as many times as
the write holding it most often holds it — under the one `scope` they share, and
`visibility_command` runs once per distinct repository the copy names, its answer used for every
item naming it. A copy of one item is a batch of one. A refusal refuses the whole copy before
anything is written, on that one check: the policy is asked nothing more, and the refusal names
the copy — `this copy's N writes` — never which item or what of it the check stopped at. An
unavailable check makes the copy unavailable the same way. With a `sanitize_command`, which rewrites per item, the batch keeps each write's
rewritable and fixed values apart, so a value fixed in one write is never screened only behind
the same value another may rewrite.

**Replacing a declared term instead of refusing it.** With a `sanitize_command`, every write
the boundary screens is put to it **in place of** `check_command` — the same one call, no second
process — and what it answers is what is written:

```yaml
write_policy:
  visibility_command: [onevcs, boundary, inspect, --input, "-"]
  check_command: [onevcs, boundary, check, --destination, public, --input, "-"]
  sanitize_command: [onevcs, boundary, sanitize, --input, "-"]
  terms_command: [onevcs, boundary, terms, --input, "-"]
  replacements:
    - {term: old-name, with: a private repository}
    - {term: other-name, reject: true}
```

The command is handed the write split into what may be rewritten — content and bodies, titles,
label names, comment bodies, metadata string values and rendering answers' string values — and,
under `fixed`, what is checked and never rewritten: `repositories`, status, priority, every id
(a project, a parent, `delivers`, `delivered_by`, a dependency's far end), metadata keys and
asset names. A term left in a fixed entry refuses the write, nothing written; a destination
declared private receives the original text. An id a create makes up from a title, or a copy
from its source's id, is made up as written, and the store's own `onetaskgraph.origin` and
`onetaskgraph.copies` values are written replaced like any other: a re-copy of an item whose id
carries a replaced term finds the item it copied before by its link, or — when the origin
search for the id as it stands finds nothing — by one more sanitisation of that origin and a
search for its replaced spelling, made only under a configured `sanitize_command`. The store matches no term itself: onevcs folds the caller's layer
over every declaration level it reads.

`replacements` is the store's own replacement layer, sent with every sanitisation above every
declaration level, a later entry winning. It takes every layer — the document,
`ONETASKGRAPH_WRITE_POLICY__REPLACEMENTS__<n>__TERM` / `__WITH` / `__REJECT`, and `--set` — and
`config show` names each entry's layer. Two global flags append entries after it for one
invocation: `--replace TERM=WITH`, split at the first `=`, and `--reject TERM`, each repeatable
and kept in the order typed. `replacements`, `--replace` or `--reject` with no
`sanitize_command` is refused at load, naming the key, so nobody believes replacement is on
when it is not; with no `sanitize_command` every write is screened exactly as before, through
`check_command`.

A write that replaced anything answers, under `--json`, a `sanitization` member beside what it
already answered — `{"replaced": <spans>, "side_effect": [{"item", "field"}], "dry_run": false}`
— absent when nothing was replaced or sanitised, so every other answer is byte for byte what it
was.

**A term already on the item.** By default a narrow write to an existing task (`priority set`,
`status set`, `content set`, `metadata set`, `update`, a comment) screens and sanitises only the
text it writes, and sends the source exactly the requests the same write sends with no
`sanitize_command`: a source not verified private still has its classification read, and a task
already private by its own record is refused as described below. A term already stored on the
task and not written by this call stays as it is, and `sources sanitize`, described next, is
how such terms are cleaned.

`sanitize_stored_on_write: true` under `write_policy` opts a store into a read-first guard
instead, off unless set, at the cost of reading the task, every comment on it and its
dependencies beside each narrow write. It takes every layer — the document,
`ONETASKGRAPH_WRITE_POLICY__SANITIZE_STORED_ON_WRITE` and `--set` — and is refused at load
with no `sanitize_command`, naming the key; a linking caller's own policy opts in with
`Engine::with_sanitize_stored_on_write`. Each such write is screened in its one call together
with the task and every comment on it as stored. A term an older store left there is rewritten
before the write is made: the task and its comments are written back, conditional on the
version the read answered — a task that moved meanwhile is read again once and sanitised from
that fresh record, keeping what changed; one that moves again is refused as
`sanitize-conflict` — and each field and comment changed is named under
`side_effect`, a human answer adding one `sanitised <item> <field>` line per change after
today's line. This guard reads immediately before writing; the plugin interface supplies no
atomic compare-and-set. An edit arriving between that final read and the write can still be
overwritten. A term replacement cannot fix is refused saying it was already on the item before this write
and which kind of field replacement cannot fix (`repositories`, an id, a term no replacement is
declared for).

`--sanitize-dry-run` asks what the guard would change, so a narrow write given it makes the
guard's reads whether or not the setting is on: an item carrying a term writes nothing and
answers `{"would_pass", "sanitization"}`, and an item needing nothing takes the write as usual.

Under either, an item whose stored `repositories` already name a
private repository is private, and a narrow write of it to a source not declared private keeps
the classification refusal's kind and words and adds that its `repositories` already named a
private repository before this write, that replacement cannot fix `repositories`, and the two
remedies: remove that repository from its `repositories`, or move it to a source declared
private. Both refusals' failure documents carry `"predates_write": true`, absent otherwise; no
sanitisation is attempted for the classification refusal.

**Sweeping a source: `onetaskgraph sources sanitize <SOURCE> [--dry-run] [--complete]
[--json]`.** It asks `terms_command` for the scope's terms — refused, naming it, without one —
and covers tasks and their comments. It does not enumerate projects or documents; their
ordinary screened writes still sanitise them. It **searches rather than lists**: one narrowing
text search per term over the source's tasks,
reading only the items those return, each with its comments, putting each to one sanitisation
before rewriting it through the same version-checked write. A search cannot establish three
classes — a hit only in stored metadata (the store's own `onetaskgraph.origin` and
`onetaskgraph.copies` values included), one only in a comment, and a spelling the policy matches
(an NFKC variant, a zero-width character inside a term) that a backend's index does not — so the
report lists each it did not cover under `coverage.uncovered`, and the sweep **exits 3** rather
than claim clean task coverage. `--complete` reads every task once, a page at a time, each
page's items decided and written before the next is asked for, covering all three; a source
whose requests spend no quota, a folder of Markdown, is always read whole; one whose request
count cannot be read is searched, as one spending a shared allowance is. `--dry-run` sends
every search and read and no write. These are source-plugin API pages: GitHub's provider
may prefetch HTTP pages into its per-command cache before returning its first caller page.

```json
{"source": "plans", "dry_run": false, "complete": false,
 "requests": {"total": 30, "page_operations": 12, "detail_operations": 9, "rewrite_operations": 9},
 "coverage": {"complete": false, "uncovered": ["metadata", "comments", "normalized-spellings"]},
 "items": [{"id": "plans:PVTI_x", "kind": "task", "outcome": "rewritten",
            "fields": [{"field": "content", "outcome": "rewritten"}]}]}
```

`outcome` is `rewritten`, `would-rewrite`, `unchanged`, `unfixable`, `conflict` or `not-a-match`
(a hit the policy does not confirm). There is an entry for every item a search returned and for
every item a whole read found carrying a term, and none for a term-free item read whole. Exit 0
when coverage is complete and everything matched was fixed or needed nothing; 1 when anything is
left unfixable or in conflict; 2 when the policy could not decide; 3 when coverage is incomplete
and nothing else failed. `requests.total` is the source's own request accounting, zero for a
folder. The three operation counters count search/list pages, detail batches or comment walks,
and rewritten items; they do not partition that total. Guard reads and individual comment
writes can send additional requests inside those operations. No term, nor any query built
from one, is written to the report, standard error, a log or the cache.

**Diagnostics name records by screened locators.** The source's name, a qualified item id, a
metadata key and a comment id are put to the write's own sanitisation as `locators` and never
rewritten; one that passes prints verbatim, and one carrying a term prints only as
`{"redacted": true, "kind": "item", "ordinal": 2}` — `<redacted item 2>` in a human answer —
ordinals counted from 1 per report or answer, its record `unfixable` with `"why": "identifier
carries a private term"`, and the command exits 1. A policy that cannot decide redacts every
locator, and the sweep exits 2. A linking caller's policy that implements only the two methods
that existed before cannot say which entry it stopped at, so its refusal redacts every locator.

**A task owns its classification.** Who may read a task is its own declaration and every
repository it names, and nothing else: all-public repositories leave it public unless it says
`private`, and any private or unknown one makes it private. The project it is filed under never
tightens it, so a public task in a private coordination project is public — read, depended on,
narrowly written and copied on its own as the public task it is, a copy on its own naming no
private project it is filed under. The tightening runs the other way: a project holding any
private task or document is private, and so is every document filed under a private project. A
classification already recorded private — declared, or written by an earlier copy — stays
private: no copy, re-copy or update loosens it.

**When the boundary is active.** It is active when the configuration names a `write_policy`,
when any source declares a `visibility`, or when a program linking the engine supplies its own
policy. An inactive store still persists, copies and routes classification, and still refuses
an explicitly private item anywhere not declared private — at a create, a copy, a route and a
render — but it consults no repository's visibility, reads nothing extra before a narrow
write, and does not read a project to learn whether a document filed under it inherits a
private classification, so a store that has not opted in answers and spends exactly what it did before. **A
store that wants repositories screened has to configure `write_policy` or declare a
visibility.** Active:

- a repository the policy does not answer `public` for makes an item private;
- a write to a source not declared private — `unknown` included — is screened as public, and a
  missing or failed check refuses it;
- a narrow write (`task status set`, `project status set`, `priority set`, `content set`,
  `metadata set`, `update`, `comment add` and `edit`, `render`) to an item of a source not verified private reads the
  item for its classification, with what it inherits — a document's project, a project's
  tasks and documents; a task inherits nothing — and refuses a private one; to a source verified private it
  reads nothing more.

**Routing by classification.** A route entry may match on `classification` instead of
`repositories`: `{classification: private, to: <source>}`, whose `to` must be declared private —
or on both together, `{repositories: [github.com/example-org/*], classification: private, to:
<source>}`, a **combined** entry matching an item of that classification every one of whose
repositories its patterns match, held to the same rule about its `to` and to every refusal a
repository entry gets. Safety is decided before matching — a private item is only ever placed in
a source declared private — and then the kind decides the order wherever an entry is written:
combined entries, then classification entries, then repository entries, each in the order
written. So an owner whose repositories are partly private routes its private work by one
combined entry, a single private repository is sent elsewhere by a combined entry naming it
written above that one, and its public work by a repository entry no combined private entry
shadows — see [Routing, and plans that span sources](#routing-and-plans-that-span-sources). A
project holding anything private goes, with every task and document in it, wholly to the
private source a classification entry names, whatever its tasks' repositories would say. Without one, its home goes where its private tasks' repositories route
it, through combined entries as through repository ones — see [Which writes route, and where each item lands](#which-writes-route-and-where-each-item-lands);
with no private source reachable, the whole copy is refused before anything is written. Either
way every task of the plan lands canonically beside that private home, and its public tasks are
seen publicly only through the views below. `sources route <SOURCE> --classification private`
answers where such an item would go.

**Public projections of a private plan.** A private coordination project copied, while the
boundary is active, to a home declared private — `project copy PRIVATE_HOME --to
PRIVATE_DESTINATION` — keeps every canonical task, edge, description, design document, approval
and rendering provenance in that destination, and makes one **view** of each public task in the
source its repositories route it to as public, when that is a source not declared private:

- one project per such source, created under an opaque id (`pw-…`) and titled `Public work`,
  naming the views' own public repositories and carrying one reserved key,
  `onetaskgraph.allocation`, a random nonce derived from nothing private — and nothing else;
- one task per public task, created under an opaque id (`pt-…`), carrying its screened title,
  content, repositories and status, and depending only on the views of other public tasks — no
  labels, priority, metadata, origin, copy link, delivery, member list, count or path of the
  private plan;
- every view put to the caller's check, every Markdown link in its content held to being a URL,
  a fragment or one of its own image assets, and any raw HTML tag carrying an attribute — or a
  `style` or `script` element — refused, since a view cannot read where such HTML points,
  **before the copy writes anything**: a view naming private material, or carrying a link it
  could not, refuses the whole copy.

A view is a view, never another executable task: the engine reads and writes the canonical
records, a view never enters its task enumeration, and a person's edit of a view changes nothing
of the plan and is overwritten by the next copy, which takes every view's content from the
canonical records again. The private destination project alone records which view is each
canonical task's, at `onetaskgraph.public_projections`:

```json
{"schema_version": 1,
 "tasks": [{"task": "vault:widget", "projection": "site:pt-57c4c9592576a5b1"}],
 "projects": [{"source": "site", "allocation": "3f9c…", "project": "pw-0c1d2e3f4a5b6c7d"}]}
```

`task` is the canonical destination task and `projection` its view, both qualified, unique by
either id, and a projection is never in a source declared private; a map that breaks any of that
is refused rather than trusted. `projects`, absent when empty, names each public source's
`Public work` project: the `allocation` it was created carrying, recorded before the create is
sent, and the `project` id that source assigned it, recorded once it answers and before any view
is filed under it; one entry per source, never a source declared private. A copy never carries the map anywhere, and records each view in
it the moment the view is created, so a copy interrupted part way and run again finds every view
it made — by the map, by its opaque id, or, at a destination that issues ids of its own, as the
one task under the public project no map entry names with the view's title and repositories —
rather than making a second. The public project is found by ownership alone: by the `project`
its entry records, or — when the copy never learned that id, its
create's answer lost or the copy ended first — as the one project, whatever its title,
carrying its recorded `allocation` — refused, writing nothing, when that one is no longer a
neutral `Public work` or when two carry it — created afresh, carrying that same allocation, only when none does. No other
project is ever taken or altered, however much it looks like one: an unrelated empty `Public work`
naming the same repositories is left as it is, so is a project a person moved a view into —
that view is left where they put it, the next copy records a new view under the copy's own
project, and a status is not mirrored to a view filed anywhere else — and so is one already
sitting at the opaque
id a create would ask for — a destination honouring that id would write over it, so the copy is
refused, naming that project, before anything is written. A copy that fails takes back the project it asked
for by its allocation even when the answer was lost; when the destination will not take it back,
the private project keeps the allocation, is named among what is still there, and the next copy
resumes into that project rather than making another. A view recorded private is refused rather than written public again,
and every field a person adds to a view in public is taken off by the next copy. A canonical task
with a view hands it every new status category — `task status set` and a `task update` naming a
status each answer the view as `projection`, and a ticket the delivery rule moves moves its view —
through the view's own screened status write; nothing but the category crosses, and a view's other
fields change only with the next copy.

**A GitHub board as the private home.** A home on a GitHub Projects source declared private
holds every canonical task of the plan, a task naming only a public repository included: its
issue is created in a private repository — the plan project's issue's, else the source's
configured `repository` — under [the placement rule above](#the-public-boundary), and it still
names its public repository, so its view carries that repository and is filed there on the
public side. A canonical issue in a private home therefore never lives in the public repository
its work changes, and a closing line in that repository (`Fixes …`) never closes it: the task
reaches `done` through its own status write — `task status set`, or the delivery rule moving it —
which its view mirrors. Each destination is read once per copy, as everywhere: a repository made
public during one copy is seen, and the copy refused before it writes, by the next.

**Every check runs before the first write.** A copy holds every item of every project to the
boundary before it writes any, reading each destination's reality once for the whole copy
before its first write; a write refused part-way is undone, as a copy always is. A private task or
document is refused by a project held public rather than filed under it. A copy out of a source
declared private into one that is not records no `onetaskgraph.origin` naming it, and a copy
into a private source records no `onetaskgraph.copies` link naming that source on a source that
is not private; the correspondence is kept on the private side instead. A dependency's far
end, a task delivered and a delivering task each name their item by its source, so the same
holds for them, whether or not any term list knows that source's name: a copy into a source
not declared private leaves out every dependency edge and `delivers` entry naming an item of a
source that is, or an item the copy carries classified private under any id it is known by,
whatever its source declares — and a delivered task there is never given a deliverer of such a
source, or one classified private, in its
`delivered_by` — the rule still moves it, and the private item keeps the relationship in its
own record. Between public items nothing is left out. While the boundary is active, a copy
into a source not declared private first reads, once per command, every item outside the copy
that a carried dependency or delivery names in a source not declared private, and withholds
its id when that item is private by its own record, its repositories, or what it inherits —
a project every task and document it holds, a document its project — whatever its source
declares; a reference whose classification cannot be read — its source not configured, any of
those reads failing, or no such item — is refused before anything is written. A `--depends-on` or
`--delivers` naming such an item — or, while the boundary is active, an item of any other
source read as private, with what it inherits, or one it cannot classify — is the caller asking
for it to be written, so that write is refused before anything is written rather than quietly
answering something else.

**A public task names a private prerequisite opaquely.** Onto a source not declared private, a
dependency on a private item — a `--depends-on <private source>:<native id>`, or an edge a copy
carries — is written as `{"designator", "id", "kind"}`: the private tracker's designator, which
conveys nothing about it, and the item's native id, read once at its source to confirm it is
one. A prefixed human key (`ABC-123`), a URL, a slug, and an endpoint object carrying any key
beyond those three are refused before anything is written, with `--json` kind
`private-reference-content`, naming the form or the key and never its value. A private source
whose backend has no identifier of its own is named by its `public_alias` — a source key, 16 to
64 of `A-Z a-z 0-9 _ -`, declared identically on every host configuring that tracker. A source
with neither keeps the rule below. `docs/metadata.md`, under "The opaque far end" and "Which
sources a dependency read asks", states the form and the rule.

**A public task's dependency on a private task is kept on the private side**, when the private
task's source has no designator. A copy that
withholds an edge from a task it lands somewhere not declared private to a task of the copied
set that lands in a source declared private records that edge on the private task instead,
under the reserved key `onetaskgraph.depended_on_by`, one entry per dependent's qualified id.
So a plan whose home is on a public board and whose member project is in a private source keeps
its order both ways. `task deps` of the public task also asks every source declared private
which of its tasks record it — only while the public task carries a copy's record and not
`onetaskgraph.edges_on_record`, the mark a copy leaves on a task whose every edge is its own,
so a task no copy landed never costs a private source a request — a copy of a task filed under a home naming a member in such a
source asks that source and carries the edge to the private task's counterpart, `task deps
--direction depended-on-by` of the private task reads what it records, and `project graph`
draws the edge. A source declared private that could not be asked — its credential absent or
refused, or it unreachable or rate-limited — leaves those reads of a task of another source
answering every edge the other sources hold, exiting 0 and naming that source on standard
error and in the `--json` answer's `unanswered`, never in `errors`,
so a reader without that source's credential still reads the public graph; `--complete` refuses such an
answer, and a program linking the engine reads `ProjectGraph::is_complete` or asks for
`Completeness::Required`. A read of a task that lives in that source still fails, and anything
else a source answers with — a record that does not say what it names, a refusal, a
configuration it reports wrong — fails the read either way rather than leaving the edge out. An
edge naming a private task the copy does not carry is withheld as before, recorded nowhere,
because the copy was not asked to write to that task's source. A later copy keeps exactly one
entry per dependent, takes off a dependent it landed whose edges no longer name that task —
asking every source declared private which of its tasks record it, so a dependent copied on its
own with `--member` takes its dropped edge off a task the copy does not carry — and leaves every entry naming a task it did not land as it was; a copy whose dependent names a
private task its source no longer holds, or holds with a record it cannot read, is refused and
taken back. Nothing of this reaches the public destination; the home's own
`onetaskgraph.members` list is unchanged by it.

**Term scope.** Which private repositories a check derives terms from is the caller's to say.
Every verb that writes takes `--term-scope <host/owner/name>`, repeatable, or
`--term-scope-empty`, refused together; neither means every private repository the policy
knows of, and an empty scope derives none. The SDKs take it as `term_scope` (a list, an empty
list, or none), and a program linking the engine sets it with `Engine::with_term_scope`.

The refusals name what happened and what to do: `not-private-destination`,
`destination-not-private`, `visibility-unreadable`, `boundary-refused`,
`boundary-unavailable`, `private-member`, `private-reference`, `reference-unclassified` and
`project-unclassified` are their `--json` failure kinds. While the boundary is active, an item
filed under a project its source does not hold, or cannot read, is unclassified rather than
public: it reaches a source declared private as private, and every other write of it — a create,
a copy, a narrow write, a delivery — is refused as `project-unclassified` before anything is
written. A project found public earlier in the same command is read again before that answer is
used, so one deleted, made unreadable or made private part way through is never taken as public.

A private source and its routes need no checkout to live in: the user-level document,
`$XDG_CONFIG_HOME/onetaskgraph/config.yaml`, is read beneath every project document, so a
private source configured there is never committed beside public work.

## Addressing

Every item is qualified by the source it came from and rendered `<source>:<native>` —
`work:ENG-142`, `notes:2026-08-inbox`. Parsing splits on the **first** colon, so a native
id may contain colons freely.

## Custom metadata and repositories

A task and a project each carry a caller-defined `metadata` map and the `repositories`
they concern, and a dependency edge names both its ends by kind and qualified id — so an
edge may cross projects, cross the task and project levels, and cross sources.
[`docs/metadata.md`](./docs/metadata.md) says which keys are reserved and where each source
keeps them. Two of them make a create safe to retry: `onetaskgraph.creating`, which an item
being copied holds only while a create of it is in flight, and `onetaskgraph.create_key`, which
the item a copy created keeps from then on — see [A create safe to retry](#a-create-safe-to-retry).

![A terminal showing two dependency rows from `task deps`: the same task blocking another task of its own source, and blocking a task of a different source, each end named by kind and qualified id](docs/screenshots/task-deps.svg)

The far end of the second row is in another source. It is reported by qualified id and kind
and never followed: opening it is a command of your own.

## Attestations

An **attestation** records that someone cleared an item's authored content against a bar —
a review that passed a task, a person's approval of a design document — keyed on that content,
so it goes void the moment the content it cleared moves, the same way in every source:

```sh
onetaskgraph document attest board:ISSUE-9 --kind design-approval --by "ada" \
  --bar sha256:… [--exclude myapp.owner]… [--note "looks right"] [--if-version V]
onetaskgraph document attestation check board:ISSUE-9 --kind design-approval [--bar sha256:…]
onetaskgraph project attestation check notes:plan-1 --kind plan-review --tasks
```

Each of `task`, `project` and `document` has `attest` and `attestation check`. A record is
held under the reserved metadata key `onetaskgraph.attestations`, one record per kind —
`{"kind", "by", "at", "content_digest", "bar", "note"?, "excludes"?}` — and recording a kind
again replaces that kind's record alone. The store computes `content_digest` itself: the
SHA-256 of the item's title, body, template answers (the `answers_digest` of its
`onetaskgraph.template` entry), repositories, its own dependency edges (each far end named by
its lineage root — the root of its chain of copies, which a copy records on what it lands at
`onetaskgraph.lineage` beside `onetaskgraph.origin`: one read of the far end, its lineage, else its
origin, else its own id — so a copy that files it under another id keeps the edge, deleting a copy
between it and its root keeps it, renaming it keeps it, and pointing the edge at another item voids
the record whatever the two are titled; an opaque far end is named by its designator and its id
there, so the same id in another tracker voids it too) and every caller-owned
metadata key but those `--exclude` names. Never its status, priority, labels, ids, location or
any store-reserved key — the attestations included — so a copy, which carries all of it,
carries a record that still checks `valid` where it lands. `--if-version` records it only while
the item is at that version, read first, writing nothing at any other. A write that sanitises a
task as stored first (`write_policy.sanitize_stored_on_write`) records the digest of what that
sanitisation left.

A check answers `{"kind", "state", "reason", "record"}`: `valid` (exit `0`); `void` (exit `3`)
with `reason` `content-moved` when the content is not what the record cleared, else
`bar-moved` when `--bar` is given and is not the record's bar; or `absent` (exit `3`) when the
item holds no record of the kind. `project attestation check --tasks` answers
`{"project", "kind", "tasks": [{"id", …}]}`, one entry per task of the project, and exits `0`
only when every one is valid. The store records and checks and decides nothing else: who may
attest, and which kinds anything requires, are the caller's.

## Writing a source in another language

A source that is not a Rust crate speaks a line-oriented JSON protocol over stdio:
[`docs/plugin-protocol.md`](./docs/plugin-protocol.md) specifies it completely — the
framing, the capability handshake, one method per trait method, and the error envelope.

Configure one with the `subprocess` plugin, which names the program to run and hands it
its own settings verbatim:

```yaml
sources:
  notes:
    plugin: subprocess
    config:
      command: /usr/local/bin/my-source
      args: [--serve]
      secrets: [LINEAR_API_KEY]   # forwarded in the handshake; nothing else is
      settings: { root: ~/notes } # this source's own `config:` block
```

A source behind that seam is a source like any other: it declares its own capabilities,
so a plan says `pushed down` for what it applies itself, and the engine compensates for
the rest exactly as it does in process.

`onetaskgraph-source` is the **reference implementation of that plugin side**, and the test
host this repository drives its own journeys against: it hosts any built-in plugin over the
same protocol, so every journey runs a second time over a real pipe to a real second
process, and so you can read a working peer beside the specification.

**It is not part of the command-line interface.** Nothing `onetaskgraph` does needs it at
run time, and nothing may depend on finding it beside an installed CLI or resolve it off a
search path — a downstream test chain that did picked up an unrelated stale build and
treated a reference host as a runtime dependency. Read it, or build it, from a checkout:

```bash
cargo build -p onetaskgraph --bin onetaskgraph-source
```

## Licence

MIT. See [`LICENSE`](./LICENSE).
