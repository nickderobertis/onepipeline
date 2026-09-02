# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.18.6](https://github.com/nickderobertis/onepipeline/compare/v0.18.5...v0.18.6) - 2026-09-02

### Added

- *(views)* serve a run's row from a summary document instead of folding its journal ([#184](https://github.com/nickderobertis/onepipeline/pull/184))

## [0.18.5](https://github.com/nickderobertis/onepipeline/compare/v0.18.4...v0.18.5) - 2026-09-01

### Fixed

- *(edits)* carry a node's release targets across every live edit that moves its deps ([#182](https://github.com/nickderobertis/onepipeline/pull/182))

## [0.18.4](https://github.com/nickderobertis/onepipeline/compare/v0.18.3...v0.18.4) - 2026-09-01

### Fixed

- *(writeback)* back off a failing plan-store projection instead of retrying it four times a second ([#176](https://github.com/nickderobertis/onepipeline/pull/176))

## [0.18.3](https://github.com/nickderobertis/onepipeline/compare/v0.18.2...v0.18.3) - 2026-08-31

### Added

- render the producer's adoption instruction at both places a consumer reads one ([#174](https://github.com/nickderobertis/onepipeline/pull/174))
- check a plan through the engine's own loader, with consumer checks plugged in ([#172](https://github.com/nickderobertis/onepipeline/pull/172))

## [0.18.2](https://github.com/nickderobertis/onepipeline/compare/v0.18.1...v0.18.2) - 2026-08-31

### Added

- *(notes)* deliver a manager note to both parties of a live dispatch ([#170](https://github.com/nickderobertis/onepipeline/pull/170))

## [0.18.1](https://github.com/nickderobertis/onepipeline/compare/v0.18.0...v0.18.1) - 2026-08-31

### Added

- settle a fast-adoption node complete-but-draft until its dependency publishes ([#164](https://github.com/nickderobertis/onepipeline/pull/164))

## [0.18.0](https://github.com/nickderobertis/onepipeline/compare/v0.17.5...v0.18.0) - 2026-08-30

### Added

- give every dispatch its own scratch directory, and stop killing completed turns ([#162](https://github.com/nickderobertis/onepipeline/pull/162))
- *(settlement)* check a criterion naming a literal in a named file, and report a mismatch as a finding ([#161](https://github.com/nickderobertis/onepipeline/pull/161))

### Fixed

- give the settlement write-back a truthful status vocabulary and fixtures that can fail ([#163](https://github.com/nickderobertis/onepipeline/pull/163))
- make the run views and the ledger say what actually happened to a node ([#159](https://github.com/nickderobertis/onepipeline/pull/159))

## [0.17.5](https://github.com/nickderobertis/onepipeline/compare/v0.17.4...v0.17.5) - 2026-08-29

### Fixed

- *(deps)* link the newest engine releases each requirement already permits ([#156](https://github.com/nickderobertis/onepipeline/pull/156))

## [0.17.4](https://github.com/nickderobertis/onepipeline/compare/v0.17.3...v0.17.4) - 2026-08-29

### Changed

- *(release)* declare release targets in the canonical release-targets.toml ([#154](https://github.com/nickderobertis/onepipeline/pull/154))

### Fixed

- *(ci)* read back what gh answered a write with, rather than printing it as a URL ([#153](https://github.com/nickderobertis/onepipeline/pull/153))

## [0.17.3](https://github.com/nickderobertis/onepipeline/compare/v0.17.2...v0.17.3) - 2026-08-29

### Fixed

- *(engine)* report a settled run and a dead provider as what they are ([#151](https://github.com/nickderobertis/onepipeline/pull/151))

## [0.17.2](https://github.com/nickderobertis/onepipeline/compare/v0.17.1...v0.17.2) - 2026-08-29

### Fixed

- *(writeback)* own what the plan declares and preserve what it does not ([#149](https://github.com/nickderobertis/onepipeline/pull/149))

## [0.17.1](https://github.com/nickderobertis/onepipeline/compare/v0.17.0...v0.17.1) - 2026-08-28

### Fixed

- *(deps)* link the onevcs and onejudge releases carrying three landed fixes ([#147](https://github.com/nickderobertis/onepipeline/pull/147))

## [0.17.0](https://github.com/nickderobertis/onepipeline/compare/v0.16.4...v0.17.0) - 2026-08-28

### Added

- *(edits)* offer a whole accepted edit to a validator, beside the per-command one ([#145](https://github.com/nickderobertis/onepipeline/pull/145))

## [0.16.4](https://github.com/nickderobertis/onepipeline/compare/v0.16.3...v0.16.4) - 2026-08-28

### Added

- *(release)* declare this repository's release targets and prove them complete ([#144](https://github.com/nickderobertis/onepipeline/pull/144))

### Fixed

- *(views)* say a judge rejected the work, and advise reviewing that verdict ([#142](https://github.com/nickderobertis/onepipeline/pull/142))

## [0.16.3](https://github.com/nickderobertis/onepipeline/compare/v0.16.2...v0.16.3) - 2026-08-27

### Fixed

- *(writeback)* preserve authored content the write-back does not own ([#140](https://github.com/nickderobertis/onepipeline/pull/140))

## [0.16.2](https://github.com/nickderobertis/onepipeline/compare/v0.16.1...v0.16.2) - 2026-08-27

### Added

- write node settlement and live graph edits back to the onetaskgraph plan ([#136](https://github.com/nickderobertis/onepipeline/pull/136))

## [0.16.1](https://github.com/nickderobertis/onepipeline/compare/v0.16.0...v0.16.1) - 2026-08-26

### Fixed

- *(deps)* adopt the published onevcs, oneharness-core, oneagentgraph and onejudge ([#137](https://github.com/nickderobertis/onepipeline/pull/137))

## [0.16.0](https://github.com/nickderobertis/onepipeline/compare/v0.15.2...v0.16.0) - 2026-08-26

### Added

- [**breaking**] read a plan from a onetaskgraph project instead of a file ([#134](https://github.com/nickderobertis/onepipeline/pull/134))

## [0.15.2](https://github.com/nickderobertis/onepipeline/compare/v0.15.1...v0.15.2) - 2026-08-26

### Added

- *(driver)* give adopt the attach/detach pair start has ([#133](https://github.com/nickderobertis/onepipeline/pull/133))

### Fixed

- *(sys)* keep a process start token from decaying, and stop reporting a stop that stopped nothing ([#132](https://github.com/nickderobertis/onepipeline/pull/132))
- *(views)* stop prescribing adopt for a settled run, and name requeue for a parked one ([#130](https://github.com/nickderobertis/onepipeline/pull/130))

## [0.15.1](https://github.com/nickderobertis/onepipeline/compare/v0.15.0...v0.15.1) - 2026-08-26

### Fixed

- *(deps)* resolve onevcs 0.15.2, the repair of the 0.15.1 capture regression ([#128](https://github.com/nickderobertis/onepipeline/pull/128))

## [0.15.0](https://github.com/nickderobertis/onepipeline/compare/v0.14.2...v0.15.0) - 2026-08-26

### Added

- add a binding amend op and validate the nodes a live edit introduces ([#120](https://github.com/nickderobertis/onepipeline/pull/120))
- *(engine)* tell a dispatch that died from an agent that failed its task ([#125](https://github.com/nickderobertis/onepipeline/pull/125))

### Fixed

- *(dispatch)* settle library runs on their terminal event ([#126](https://github.com/nickderobertis/onepipeline/pull/126))
- *(engine)* apply every queued message in one reconcile pass ([#127](https://github.com/nickderobertis/onepipeline/pull/127))
- *(release)* ask one release question per release, and tell a pending probe from a failed one ([#121](https://github.com/nickderobertis/onepipeline/pull/121))

## [0.14.2](https://github.com/nickderobertis/onepipeline/compare/v0.14.1...v0.14.2) - 2026-08-25

### Added

- pin every engine at its newest release, refuse a split lock, and stop a stale check ending a publication ([#122](https://github.com/nickderobertis/onepipeline/pull/122))

## [0.14.1](https://github.com/nickderobertis/onepipeline/compare/v0.14.0...v0.14.1) - 2026-08-25

### Fixed

- stop a Windows run deterministically and read a run record an earlier build wrote ([#118](https://github.com/nickderobertis/onepipeline/pull/118))

## [0.14.0](https://github.com/nickderobertis/onepipeline/compare/v0.13.0...v0.14.0) - 2026-08-24

### Added

- *(events)* relay session-correlated release events ([#117](https://github.com/nickderobertis/onepipeline/pull/117))

### Fixed

- *(ci)* read the crates.io index in the shape it serves ([#115](https://github.com/nickderobertis/onepipeline/pull/115))

## [0.13.0](https://github.com/nickderobertis/onepipeline/compare/v0.12.4...v0.13.0) - 2026-08-24

### Added

- give every plan node an adoption mode over its dependencies' releases ([#113](https://github.com/nickderobertis/onepipeline/pull/113))

## [0.12.3](https://github.com/nickderobertis/onepipeline/compare/v0.12.2...v0.12.3) - 2026-08-23

### Added

- *(deps)* link the current engines and route a pushed-unverified publication ([#109](https://github.com/nickderobertis/onepipeline/pull/109))

## [0.12.2](https://github.com/nickderobertis/onepipeline/compare/v0.12.1...v0.12.2) - 2026-08-23

### Fixed

- land the transcript refinements stranded after PR #102 squash-merged ([#108](https://github.com/nickderobertis/onepipeline/pull/108))
- confirm the whole process tree on Windows so a reaped test leaves nothing holding its handle ([#106](https://github.com/nickderobertis/onepipeline/pull/106))

## [0.12.1](https://github.com/nickderobertis/onepipeline/compare/v0.12.0...v0.12.1) - 2026-08-22

### Fixed

- make the transcript carry tool outputs and the telemetry buckets balance ([#102](https://github.com/nickderobertis/onepipeline/pull/102))

## [0.12.0](https://github.com/nickderobertis/onepipeline/compare/v0.11.0...v0.12.0) - 2026-08-22

### Added

- take a surface message off the command line and let a monitor report findings ([#101](https://github.com/nickderobertis/onepipeline/pull/101))

## [0.11.0](https://github.com/nickderobertis/onepipeline/compare/v0.10.1...v0.11.0) - 2026-08-22

### Added

- [**breaking**] let each repository's own merge path verify the change, on the onevcs release with no gate ([#98](https://github.com/nickderobertis/onepipeline/pull/98))

## [0.10.1](https://github.com/nickderobertis/onepipeline/compare/v0.10.0...v0.10.1) - 2026-08-21

### Fixed

- *(deps)* resolve the producer that publishes tool results and per-turn usage ([#96](https://github.com/nickderobertis/onepipeline/pull/96))

## [0.10.0](https://github.com/nickderobertis/onepipeline/compare/v0.9.0...v0.10.0) - 2026-08-21

### Added

- *(lifecycle)* route a failed publication back to a live worker with its evidence ([#93](https://github.com/nickderobertis/onepipeline/pull/93))

## [0.9.0](https://github.com/nickderobertis/onepipeline/compare/v0.8.6...v0.9.0) - 2026-08-20

### Fixed

- *(channel)* [**breaking**] route a reply by the halves it carries, not by arrival order ([#90](https://github.com/nickderobertis/onepipeline/pull/90))

## [0.8.6](https://github.com/nickderobertis/onepipeline/compare/v0.8.5...v0.8.6) - 2026-08-20

### Fixed

- *(views)* tell a recovered identity chain from one that ran out ([#89](https://github.com/nickderobertis/onepipeline/pull/89))

## [0.8.5](https://github.com/nickderobertis/onepipeline/compare/v0.8.4...v0.8.5) - 2026-08-20

### Fixed

- *(deps)* adopt onevcs 0.8.0 in the declaration, the lock, and the call sites ([#86](https://github.com/nickderobertis/onepipeline/pull/86))

## [0.8.4](https://github.com/nickderobertis/onepipeline/compare/v0.8.3...v0.8.4) - 2026-08-20

### Fixed

- *(adopt)* resume or name the dispatch a fresh driver leaves behind ([#84](https://github.com/nickderobertis/onepipeline/pull/84))

## [0.8.3](https://github.com/nickderobertis/onepipeline/compare/v0.8.2...v0.8.3) - 2026-08-19

### Fixed

- link the conversation producer, and prove the stream carries it ([#82](https://github.com/nickderobertis/onepipeline/pull/82))

## [0.8.2](https://github.com/nickderobertis/onepipeline/compare/v0.8.1...v0.8.2) - 2026-08-19

### Added

- *(views)* name every skipped node and the dependency that skipped it ([#80](https://github.com/nickderobertis/onepipeline/pull/80))

## [0.8.1](https://github.com/nickderobertis/onepipeline/compare/v0.8.0...v0.8.1) - 2026-08-19

### Fixed

- *(executor)* hand every node dispatch the run it belongs to ([#76](https://github.com/nickderobertis/onepipeline/pull/76))

## [0.8.0](https://github.com/nickderobertis/onepipeline/compare/v0.7.5...v0.8.0) - 2026-08-18

### Added

- [**breaking**] move onto oneagentgraph 0.3.0 and its persona format ([#74](https://github.com/nickderobertis/onepipeline/pull/74))

## [0.7.5](https://github.com/nickderobertis/onepipeline/compare/v0.7.4...v0.7.5) - 2026-08-18

### Added

- *(lifecycle)* name the ending when a change request's body is not drafted ([#72](https://github.com/nickderobertis/onepipeline/pull/72))

## [0.7.4](https://github.com/nickderobertis/onepipeline/compare/v0.7.3...v0.7.4) - 2026-08-18

### Fixed

- *(ledger)* never leave a torn record, and say when one was found ([#68](https://github.com/nickderobertis/onepipeline/pull/68))

## [0.7.3](https://github.com/nickderobertis/onepipeline/compare/v0.7.2...v0.7.3) - 2026-08-18

### Added

- *(contract)* publish report retention and its path as a supported surface ([#69](https://github.com/nickderobertis/onepipeline/pull/69))

## [0.7.2](https://github.com/nickderobertis/onepipeline/compare/v0.7.1...v0.7.2) - 2026-08-18

### Fixed

- tell a cancelling node from a parked one, and stop heartbeats hiding a stall ([#67](https://github.com/nickderobertis/onepipeline/pull/67))
- prove a recorded pid is still its process, and stop what a run is running ([#65](https://github.com/nickderobertis/onepipeline/pull/65))
- *(deps)* take up a stranded branch's session instead of refusing the retry ([#64](https://github.com/nickderobertis/onepipeline/pull/64))

## [0.7.1](https://github.com/nickderobertis/onepipeline/compare/v0.7.0...v0.7.1) - 2026-08-16

### Fixed

- stop a cancelled dispatch, and stop lying about what a settled node did ([#62](https://github.com/nickderobertis/onepipeline/pull/62))

## [0.7.0](https://github.com/nickderobertis/onepipeline/compare/v0.6.3...v0.7.0) - 2026-08-16

### Added

- *(config)* [**breaking**] bump the launch config to schema 2 for the drafting graph ([#60](https://github.com/nickderobertis/onepipeline/pull/60))

## [0.6.3](https://github.com/nickderobertis/onepipeline/compare/v0.6.2...v0.6.3) - 2026-08-16

### Fixed

- *(deps)* adopt the released oneagentgraph and onevcs ([#58](https://github.com/nickderobertis/onepipeline/pull/58))

## [0.6.1](https://github.com/nickderobertis/onepipeline/compare/v0.6.0...v0.6.1) - 2026-08-15

### Added

- *(plan)* validate node titles at load against onevcs's limit ([#54](https://github.com/nickderobertis/onepipeline/pull/54))

## [0.6.0](https://github.com/nickderobertis/onepipeline/compare/v0.5.0...v0.6.0) - 2026-08-15

### Fixed

- *(views)* report rejections, prove liveness, attribute failures ([#51](https://github.com/nickderobertis/onepipeline/pull/51))

## [0.5.0](https://github.com/nickderobertis/onepipeline/compare/v0.4.0...v0.5.0) - 2026-08-15

### Added

- filter what a run relays, and what one reader of it sees ([#48](https://github.com/nickderobertis/onepipeline/pull/48))

### Fixed

- name the package in the documented install, and let its check block ([#49](https://github.com/nickderobertis/onepipeline/pull/49))

## [0.4.0](https://github.com/nickderobertis/onepipeline/compare/v0.3.1...v0.4.0) - 2026-08-15

### Added

- [**breaking**] replace rounds with a continuous, dependency-driven engine loop ([#46](https://github.com/nickderobertis/onepipeline/pull/46))

## [0.3.1](https://github.com/nickderobertis/onepipeline/compare/v0.3.0...v0.3.1) - 2026-08-15

### Fixed

- link the released sibling and report a node landed, not settled ([#44](https://github.com/nickderobertis/onepipeline/pull/44))

## [0.3.0](https://github.com/nickderobertis/onepipeline/compare/v0.2.0...v0.3.0) - 2026-08-15

### Fixed

- [**breaking**] forward a node's turn budget and refuse a node-level done_when ([#42](https://github.com/nickderobertis/onepipeline/pull/42))

## [0.2.0](https://github.com/nickderobertis/onepipeline/compare/v0.1.15...v0.2.0) - 2026-08-14

### Added

- [**breaking**] say what the run is, not who drives it ([#40](https://github.com/nickderobertis/onepipeline/pull/40))

## [0.1.15](https://github.com/nickderobertis/onepipeline/compare/v0.1.14...v0.1.15) - 2026-08-14

### Fixed

- classify a Windows teardown from what it can check ([#38](https://github.com/nickderobertis/onepipeline/pull/38))

## [0.1.13](https://github.com/nickderobertis/onepipeline/compare/v0.1.12...v0.1.13) - 2026-08-13

### Fixed

- agree on launch state and settle a round honestly ([#32](https://github.com/nickderobertis/onepipeline/pull/32))

## [0.1.12](https://github.com/nickderobertis/onepipeline/compare/v0.1.11...v0.1.12) - 2026-08-13

### Fixed

- cancel a graph run whichever way it is running ([#30](https://github.com/nickderobertis/onepipeline/pull/30))

## [0.1.11](https://github.com/nickderobertis/onepipeline/compare/v0.1.10...v0.1.11) - 2026-08-13

### Fixed

- resolve agent-graph paths against the launch directory ([#27](https://github.com/nickderobertis/onepipeline/pull/27))

## [0.1.10](https://github.com/nickderobertis/onepipeline/compare/v0.1.9...v0.1.10) - 2026-08-12

### Fixed

- refuse to launch onto an identity a live run already holds ([#24](https://github.com/nickderobertis/onepipeline/pull/24))

## [0.1.9](https://github.com/nickderobertis/onepipeline/compare/v0.1.8...v0.1.9) - 2026-08-12

### Fixed

- forward plan persona to node graph ([#22](https://github.com/nickderobertis/onepipeline/pull/22))

## [0.1.8](https://github.com/nickderobertis/onepipeline/compare/v0.1.7...v0.1.8) - 2026-08-12

### Added

- forward per-run graph overrides through start and adopt ([#19](https://github.com/nickderobertis/onepipeline/pull/19))

## [0.1.7](https://github.com/nickderobertis/onepipeline/compare/v0.1.6...v0.1.7) - 2026-08-12

### Added

- deliver a context note into the running turn ([#17](https://github.com/nickderobertis/onepipeline/pull/17))

## [0.1.6](https://github.com/nickderobertis/onepipeline/compare/v0.1.5...v0.1.6) - 2026-08-11

### Changed

- reach onevcs by calling it, not by spawning it ([#15](https://github.com/nickderobertis/onepipeline/pull/15))

## [0.1.5](https://github.com/nickderobertis/onepipeline/compare/v0.1.4...v0.1.5) - 2026-08-11

### Fixed

- read a publication's outcome from the stream onevcs writes ([#13](https://github.com/nickderobertis/onepipeline/pull/13))

## [0.1.4](https://github.com/nickderobertis/onepipeline/compare/v0.1.3...v0.1.4) - 2026-08-10

### Fixed

- make the ingest-refusal journey pass on Windows ([#11](https://github.com/nickderobertis/onepipeline/pull/11))

## [0.1.3](https://github.com/nickderobertis/onepipeline/compare/v0.1.2...v0.1.3) - 2026-08-09

### Fixed

- dispatch through oneagentgraph, and fail a launch it refuses ([#7](https://github.com/nickderobertis/onepipeline/pull/7))

## [0.1.1](https://github.com/nickderobertis/onepipeline/compare/v0.1.0...v0.1.1) - 2026-08-09

### Added

- port e2e suite and implement the onepipeline contract ([#2](https://github.com/nickderobertis/onepipeline/pull/2))

## [0.1.0](https://github.com/nickderobertis/onepipeline/releases/tag/v0.1.0) - 2026-08-08

### Added

- bootstrap the repo and lay the contract down interface-only

### Documentation

- say each thing where it is true and nowhere else

### Fixed

- gate the copies of the contract that live outside it
- close the llmlint findings the first full-tree run surfaced
# Changelog

All notable changes to this project are documented here.

This file is maintained by [release-plz](https://release-plz.dev/) from
Conventional Commits — do not edit it by hand.
