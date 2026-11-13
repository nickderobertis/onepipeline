#![allow(missing_docs, unreachable_pub, dead_code, unused, clippy::all)]
//! `onepipeline` owns a task DAG, executes it over
//! [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph) and
//! [`onevcs`](https://github.com/nickderobertis/onevcs), and merges the three
//! libraries' event streams into one.
//!
//! Dependency direction is one-way: `onepipeline` → `{oneagentgraph, onevcs}`.
//! Agent, harness, and model selection stay in the first; repository identities,
//! sessions, and publication stay in the second. This crate composes them and
//! owns the DAG, its continuous execution, the planner channel, and the
//! [executor seam](executor) that decides *where* a dispatch runs.
//!
//! # The surface, and what is behind it
//!
//! Every public item here is named by
//! [`docs/contract.md`](../../../docs/contract.md) — the approved contract,
//! committed verbatim — and the engine implementing it is private, so a
//! consumer can only reach what the contract promised.
//! `tests/contract.rs` drives the fixtures out of that document through these
//! types, so the two cannot drift.
//!
//! A plan is not a file: it is one **project** of a
//! [`onetaskgraph`](https://github.com/nickderobertis/onetaskgraph) store, named
//! by its qualified id and read through that product's own binary. What this
//! crate owns is the run — its journal, its ledger, and the graph it projects
//! from them — and the plan's *definition* stays where the user already tracks
//! their work.
//!
//! A run's durable state is one directory: the plan it was launched with, the
//! merged event store every view reads, the run's own result, the channel's
//! transport, and — beside the store — the account of any record a writer left
//! half-written. The process driving the run is that ledger's **single writer**,
//! guarded by the run's ownership lock; everything else reads.
//!
//! Composition is by subprocess. The agents come from `oneagentgraph` and the
//! clones, publications, and change requests from `onevcs`, each reached
//! through its own CLI — so a build of either that still refuses will make the
//! dispatches this crate starts refuse too.
//!
//! Where the contract could not be compiled exactly as written, the code
//! compiles against what does exist and the divergence is recorded in
//! [`docs/contract-divergences.md`](../../../docs/contract-divergences.md) for
//! the planner who owns the contract. Every entry there has now been ruled on
//! and the contract amended to carry the ruling.



pub use onepipeline_core::agents;
pub use onepipeline_core::branchname;
pub use onepipeline_core::channel;
pub use onepipeline_core::cli;
pub use onepipeline_core::controls;
pub use onepipeline_core::error;
pub use onepipeline_core::event;
pub use onepipeline_core::executor;
pub use onepipeline_core::filter;
pub use onepipeline_core::human_items;
pub use onepipeline_core::journal;
pub use onepipeline_core::maintenance;
pub use onepipeline_core::note;
pub use onepipeline_core::payload;
pub use onepipeline_core::plan;
pub use onepipeline_base::provenance;
pub use onepipeline_core::report;
pub use onepipeline_core::rules;
pub use onepipeline_core::telemetry;
pub use onepipeline_core::templates;
pub use onepipeline_core::verbs;
pub use onepipeline_core::views;
pub use onepipeline_core::vocabulary;

// The engine behind the contract's surface. These modules are private on
// purpose: `docs/contract.md` names the plan schema, the channel, the executor
// seam, the rules grammar, the views, and the report retention path, and a
// public item it does not name is a promise this crate did not make. The binary
// reaches them through [`run`](crate::run).
pub use onepipeline_core::adoptable;
pub use onepipeline_core::agentgraph;
pub use onepipeline_core::appendstamp;
pub use onepipeline_core::ask;
pub use onepipeline_core::boundary;
pub use onepipeline_core::changes;
pub use onepipeline_core::checkpoint;
pub use onepipeline_core::checkwatch;
pub use onepipeline_core::closures;
pub use onepipeline_core::commandstep;
pub use onepipeline_core::composition;
pub use onepipeline_core::concurrency;
pub use onepipeline_core::continuation;
pub use onepipeline_core::criteria;
pub use onepipeline_core::crossdag;
pub use onepipeline_core::destination;
pub use onepipeline_core::discharge;
pub use onepipeline_core::dispatchenv;
pub use onepipeline_core::driver;
pub use onepipeline_core::edits;
pub use onepipeline_core::engine;
pub use onepipeline_core::findings;
pub use onepipeline_core::flow;
pub use onepipeline_core::flowwatch;
pub use onepipeline_core::freespace;
pub use onepipeline_core::graph;
pub use onepipeline_core::heartbeat;
pub use onepipeline_core::home;
pub use onepipeline_core::hooks;
pub use onepipeline_base::hostphases;
pub use onepipeline_core::hostview;
pub use onepipeline_core::journal_heads;
pub use onepipeline_core::journal_selected;
pub use onepipeline_core::labels;
pub use onepipeline_core::land;
pub use onepipeline_core::ledger;
pub use onepipeline_core::lifecycle;
pub use onepipeline_core::loopstats;
pub use onepipeline_core::ordering;
pub use onepipeline_core::plancheck;
pub use onepipeline_core::planedit;
pub use onepipeline_core::pool;
pub use onepipeline_core::projection;
pub use onepipeline_core::provisioning;
pub use onepipeline_core::publicid;
pub use onepipeline_core::readjust;
pub use onepipeline_core::refusal;
pub use onepipeline_core::release;
pub use onepipeline_base::rendercost;
pub use onepipeline_core::reply;
pub use onepipeline_core::review;
pub use onepipeline_core::ruling;
pub use onepipeline_core::shutdown;
pub use onepipeline_core::stack;
pub use onepipeline_core::stopguard;
pub use onepipeline_core::storeops;
pub use onepipeline_core::summary;
pub use onepipeline_core::supersession;
pub use onepipeline_core::sys;
pub use onepipeline_core::taskgraph;
pub use onepipeline_core::trigger;
pub use onepipeline_core::unanswered;
pub use onepipeline_core::unpublished;
pub use onepipeline_core::unwatched;
pub use onepipeline_core::vcs;
pub use onepipeline_core::watch;
pub use onepipeline_core::watchers;
pub use onepipeline_core::withdrawal;
pub use onepipeline_core::writeback;
pub use onepipeline_core::yaml;

pub use error::{Error, Result};

/// The release of this crate a consumer is linking.
///
/// A host that pins this engine and separately pins a reader of the run store it
/// writes has nothing else to hold the two to one another: the retention path
/// and the resolution path are the same promise only where both sides are the
/// same release, and this is how each side says which one it is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Execute one parsed command line.
///
/// The binary is this function plus argument parsing and an exit code, so every
/// journey a user can reach is reachable from a test that drives the same
/// entry point the binary does.
pub fn run(cli: cli::Cli) -> Result<i32> {
    // The registry every record this process writes or folds is declared in,
    // built before any of them: a payload document of this build's own that did
    // not register is a build fault, and it is met here rather than mid-fold.
    hostphases::mark("run");
    {
        let _registry = hostphases::phase("event-registry");
        event::registry();
    }
    // Reaching here *is* the proof that this process's executable answers this
    // crate's command line, which is what a dispatch given a process of its own
    // is retained with. Nothing else can know it: `current_exe` names whatever
    // program is running, and this crate is linked into programs that are not
    // this one.
    if let Ok(exe) = std::env::current_exe() {
        agentgraph::speaks_this_cli(exe);
    }
    // The flow this process was started in, read once and taken out of the
    // environment before anything could spawn: whatever this process starts
    // inherits none of it — a node dispatch above all — and the one child that
    // must carry it, a flow's program, is handed it explicitly.
    flow::take_inherited();
    driver::dispatch(cli)
}

pub use onepipeline_core::scratch;
