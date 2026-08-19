# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
