# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.16](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.15...onetaskgraph-core-v0.14.16) - 2026-11-14

### Fixed

- keep an attestation valid through a copy that renumbers its dependency edges ([#4033](https://github.com/nickderobertis/onetaskgraph/pull/4033))

## [0.14.15](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.14...onetaskgraph-core-v0.14.15) - 2026-11-13

### Added

- project lists and maps onto board fields, and metadata fields onto a collapsed Linear section ([#4028](https://github.com/nickderobertis/onetaskgraph/pull/4028))

## [0.14.14](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.13...onetaskgraph-core-v0.14.14) - 2026-11-12

### Added

- assign items to people on every plugin, opt-in per source ([#3984](https://github.com/nickderobertis/onetaskgraph/pull/3984))

## [0.14.13](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.12...onetaskgraph-core-v0.14.13) - 2026-11-11

### Added

- add a parent relation and typed links between items ([#3982](https://github.com/nickderobertis/onetaskgraph/pull/3982))

## [0.14.12](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.11...onetaskgraph-core-v0.14.12) - 2026-11-11

### Added

- batch searches, copies, comments and held updates, and send fewer Linear and GitHub requests ([#3977](https://github.com/nickderobertis/onetaskgraph/pull/3977))
- *(copy)* write expandable blocks in each destination's collapsible form ([#3976](https://github.com/nickderobertis/onetaskgraph/pull/3976))
- report oversize copies before writing and screen a public copy in one batch ([#3971](https://github.com/nickderobertis/onetaskgraph/pull/3971))

## [0.14.11](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.10...onetaskgraph-core-v0.14.11) - 2026-11-10

### Added

- attest item content, validate template answers against schemas and create plan graphs from templates ([#3972](https://github.com/nickderobertis/onetaskgraph/pull/3972))

## [0.14.10](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.9...onetaskgraph-core-v0.14.10) - 2026-11-10

### Added

- hold writes to an item's version and list comments since a watermark ([#3969](https://github.com/nickderobertis/onetaskgraph/pull/3969))

## [0.14.8](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.7...onetaskgraph-core-v0.14.8) - 2026-11-08

### Fixed

- find a copy's earlier Linear counterparts without duplicating them ([#3931](https://github.com/nickderobertis/onetaskgraph/pull/3931))

## [0.14.5](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.4...onetaskgraph-core-v0.14.5) - 2026-11-07

### Fixed

- *(boundary)* screen a narrow write's own text without reading the item first ([#3917](https://github.com/nickderobertis/onetaskgraph/pull/3917))

## [0.14.4](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.3...onetaskgraph-core-v0.14.4) - 2026-11-06

### Fixed

- rewrite reference-style section links on a copy to a private destination ([#3909](https://github.com/nickderobertis/onetaskgraph/pull/3909))

## [0.14.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.2...onetaskgraph-core-v0.14.3) - 2026-11-05

### Fixed

- *(copy)* point a private document's references at records the public destination already holds ([#3900](https://github.com/nickderobertis/onetaskgraph/pull/3900))

## [0.14.2](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.1...onetaskgraph-core-v0.14.2) - 2026-11-05

### Fixed

- keep section links between a private home's public views ([#3901](https://github.com/nickderobertis/onetaskgraph/pull/3901))

## [0.14.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.14.0...onetaskgraph-core-v0.14.1) - 2026-11-05

### Fixed

- keep private-source section links that the copy carries as counterpart links ([#3891](https://github.com/nickderobertis/onetaskgraph/pull/3891))

## [0.14.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.13.5...onetaskgraph-core-v0.14.0) - 2026-11-04

### Added

- *(boundary)* [**breaking**] rewrite declared private terms on every screened write and sweep a source on demand ([#3875](https://github.com/nickderobertis/onetaskgraph/pull/3875))

## [0.13.5](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.13.4...onetaskgraph-core-v0.13.5) - 2026-11-04

### Added

- *(plugin-api)* let a task listing carry each task's first comment page ([#3881](https://github.com/nickderobertis/onetaskgraph/pull/3881))

## [0.13.4](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.13.3...onetaskgraph-core-v0.13.4) - 2026-11-04

### Fixed

- keep a private record's address out of the section links a public reader gets ([#3876](https://github.com/nickderobertis/onetaskgraph/pull/3876))

## [0.13.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.13.2...onetaskgraph-core-v0.13.3) - 2026-11-03

### Added

- *(copy)* land section links on each destination's own heading anchors ([#3868](https://github.com/nickderobertis/onetaskgraph/pull/3868))

## [0.12.14](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.13...onetaskgraph-core-v0.12.14) - 2026-10-31

### Added

- *(copy)* make a create safe to retry and guard a copy's status and priority writes ([#3838](https://github.com/nickderobertis/onetaskgraph/pull/3838))

## [0.12.13](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.12...onetaskgraph-core-v0.12.13) - 2026-10-30

### Added

- project human tasks through existing tracker routing ([#3824](https://github.com/nickderobertis/onetaskgraph/pull/3824))

## [0.12.11](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.10...onetaskgraph-core-v0.12.11) - 2026-10-29

### Fixed

- *(github-projects)* file a private home's public task in a private repository ([#3804](https://github.com/nickderobertis/onetaskgraph/pull/3804))

## [0.12.10](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.9...onetaskgraph-core-v0.12.10) - 2026-10-29

### Fixed

- *(deps)* move pulldown-cmark past the wikilink event-growth advisory ([#3802](https://github.com/nickderobertis/onetaskgraph/pull/3802))

## [0.12.9](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.8...onetaskgraph-core-v0.12.9) - 2026-10-28

### Added

- *(locate)* answer where an item lands, its counterpart and its issue's repository ([#3791](https://github.com/nickderobertis/onetaskgraph/pull/3791))

## [0.12.8](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.7...onetaskgraph-core-v0.12.8) - 2026-10-28

### Added

- *(routes)* route an item by its repositories and classification together ([#3788](https://github.com/nickderobertis/onetaskgraph/pull/3788))

### Fixed

- *(linear)* keep a private home's public projections through a copy's follow-up write ([#3783](https://github.com/nickderobertis/onetaskgraph/pull/3783))

## [0.12.7](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.6...onetaskgraph-core-v0.12.7) - 2026-10-27

### Performance

- *(copy)* batch a private source's origin lookups and reuse carried dependency edges ([#3775](https://github.com/nickderobertis/onetaskgraph/pull/3775))

## [0.12.5](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.4...onetaskgraph-core-v0.12.5) - 2026-10-25

### Fixed

- *(copy)* carry an anchor naming no destination into a public projection ([#3769](https://github.com/nickderobertis/onetaskgraph/pull/3769))

## [0.12.4](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.3...onetaskgraph-core-v0.12.4) - 2026-10-25

### Fixed

- *(linear)* [**breaking**] take what Linear stores from each write's own answer ([#3762](https://github.com/nickderobertis/onetaskgraph/pull/3762))

## [0.12.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.2...onetaskgraph-core-v0.12.3) - 2026-10-25

### Performance

- *(copy)* remove redundant screening and hosted reads ([#3749](https://github.com/nickderobertis/onetaskgraph/pull/3749))

## [0.12.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.12.0...onetaskgraph-core-v0.12.1) - 2026-10-24

### Added

- support task-owned visibility and private coordination projections ([#3725](https://github.com/nickderobertis/onetaskgraph/pull/3725))

## [0.12.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.11.1...onetaskgraph-core-v0.12.0) - 2026-10-24

### Added

- [**breaking**] read dependencies from a task's own record, naming private ones opaquely ([#3724](https://github.com/nickderobertis/onetaskgraph/pull/3724))

### Performance

- cut backend requests per dependency read, status write and settlement ([#3727](https://github.com/nickderobertis/onetaskgraph/pull/3727))

## [0.11.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.10.0...onetaskgraph-core-v0.11.0) - 2026-10-22

### Added

- *(copy)* [**breaking**] bind a copy to an existing destination task the caller names ([#3721](https://github.com/nickderobertis/onetaskgraph/pull/3721))
- *(copy)* [**breaking**] keep a destination item's status on update when the caller asks ([#3718](https://github.com/nickderobertis/onetaskgraph/pull/3718))

## [0.10.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.9.4...onetaskgraph-core-v0.10.0) - 2026-10-22

### Added

- *(copy)* [**breaking**] copy named members into an existing project without reading or writing the project ([#3714](https://github.com/nickderobertis/onetaskgraph/pull/3714))

## [0.9.4](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.9.3...onetaskgraph-core-v0.9.4) - 2026-10-22

### Added

- *(engine)* read the image assets a task holds ([#3713](https://github.com/nickderobertis/onetaskgraph/pull/3713))

## [0.9.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.9.2...onetaskgraph-core-v0.9.3) - 2026-10-21

### Performance

- *(github-projects)* list a project's members and write metadata in one request each ([#3711](https://github.com/nickderobertis/onetaskgraph/pull/3711))

## [0.9.2](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.9.1...onetaskgraph-core-v0.9.2) - 2026-10-20

### Added

- *(copy)* [**breaking**] home a plan where its repository-naming tasks route, and reuse its member project ([#3685](https://github.com/nickderobertis/onetaskgraph/pull/3685))
- *(copy)* [**breaking**] record the body_digest of what Linear stores ([#3687](https://github.com/nickderobertis/onetaskgraph/pull/3687))

### Performance

- *(github-projects)* write a copied item in fewer requests ([#3692](https://github.com/nickderobertis/onetaskgraph/pull/3692))

## [0.9.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.9.0...onetaskgraph-core-v0.9.1) - 2026-10-20

### Performance

- *(copy)* copy and list a plan without reading the whole board ([#3679](https://github.com/nickderobertis/onetaskgraph/pull/3679))

## [0.9.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.8.0...onetaskgraph-core-v0.9.0) - 2026-10-19

### Added

- *(status)* [**breaking**] move a project item with its issue's closure and set a project's status ([#3681](https://github.com/nickderobertis/onetaskgraph/pull/3681))

## [0.8.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.7.3...onetaskgraph-core-v0.8.0) - 2026-10-19

### Added

- *(status)* [**breaking**] complete landed issues and skip unrepresentable copies ([#3677](https://github.com/nickderobertis/onetaskgraph/pull/3677))

## [0.7.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.7.2...onetaskgraph-core-v0.7.3) - 2026-10-18

### Fixed

- *(config)* leave a source no layer defines unconfigured instead of failing ([#3650](https://github.com/nickderobertis/onetaskgraph/pull/3650))
- *(deps)* answer a dependency read partially when a private source cannot ([#3651](https://github.com/nickderobertis/onetaskgraph/pull/3651))

## [0.7.2](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.7.1...onetaskgraph-core-v0.7.2) - 2026-10-18

### Fixed

- *(copy)* [**breaking**] report an uncertain destination when a plugin's create answer is malformed ([#3644](https://github.com/nickderobertis/onetaskgraph/pull/3644))

## [0.7.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.7.0...onetaskgraph-core-v0.7.1) - 2026-10-15

### Added

- *(cache)* serve Linear comments and dependency reads from the cache ([#3619](https://github.com/nickderobertis/onetaskgraph/pull/3619))

### Fixed

- *(plans)* keep a public task's dependency on a private source's task ([#3630](https://github.com/nickderobertis/onetaskgraph/pull/3630))
- *(copy)* file a private source's documents under the home its copy link names ([#3625](https://github.com/nickderobertis/onetaskgraph/pull/3625))

## [0.7.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.6.1...onetaskgraph-core-v0.7.0) - 2026-10-14

### Added

- *(cache)* [**breaking**] cache every vouched project and document read ([#3609](https://github.com/nickderobertis/onetaskgraph/pull/3609))

## [0.6.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.6.0...onetaskgraph-core-v0.6.1) - 2026-10-14

### Fixed

- *(deps)* read and write task YAML with serde-saphyr in place of serde_norway ([#3607](https://github.com/nickderobertis/onetaskgraph/pull/3607))

## [0.6.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.5.0...onetaskgraph-core-v0.6.0) - 2026-10-12

### Added

- *(cache)* [**breaking**] cache remote items and comments per source, served only against a token in hand ([#3595](https://github.com/nickderobertis/onetaskgraph/pull/3595))

## [0.5.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.4.0...onetaskgraph-core-v0.5.0) - 2026-10-12

### Added

- *(copy)* [**breaking**] carry comments, skip items unchanged since the last copy, and copy by query ([#3593](https://github.com/nickderobertis/onetaskgraph/pull/3593))

## [0.4.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.12...onetaskgraph-core-v0.4.0) - 2026-10-10

### Added

- *(github-projects)* [**breaking**] project configured metadata onto board text fields ([#3583](https://github.com/nickderobertis/onetaskgraph/pull/3583))

## [0.3.12](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.11...onetaskgraph-core-v0.3.12) - 2026-10-10

### Added

- classify store items and enforce private destinations ([#3580](https://github.com/nickderobertis/onetaskgraph/pull/3580))

## [0.3.11](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.10...onetaskgraph-core-v0.3.11) - 2026-10-09

### Added

- *(cli)* print a project's task dependency graph as Mermaid ([#3576](https://github.com/nickderobertis/onetaskgraph/pull/3576))

## [0.3.10](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.9...onetaskgraph-core-v0.3.10) - 2026-10-09

### Fixed

- *(copy)* keep metadata only the destination holds when a copy updates it ([#3567](https://github.com/nickderobertis/onetaskgraph/pull/3567))

## [0.3.9](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.8...onetaskgraph-core-v0.3.9) - 2026-10-08

### Added

- *(github-projects)* upload image assets as GitHub user attachments ([#3560](https://github.com/nickderobertis/onetaskgraph/pull/3560))

## [0.3.8](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.7...onetaskgraph-core-v0.3.8) - 2026-10-08

### Added

- *(linear)* upload image assets through Linear's file upload ([#3561](https://github.com/nickderobertis/onetaskgraph/pull/3561))

## [0.3.7](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.6...onetaskgraph-core-v0.3.7) - 2026-10-08

### Fixed

- name no real organization's configuration in onetaskgraph's examples, fixtures and tests ([#3551](https://github.com/nickderobertis/onetaskgraph/pull/3551))

## [0.3.6](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.5...onetaskgraph-core-v0.3.6) - 2026-10-07

### Fixed

- *(copy)* carry a reference's #fragment through a rewrite ([#3518](https://github.com/nickderobertis/onetaskgraph/pull/3518))

## [0.3.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.2...onetaskgraph-core-v0.3.3) - 2026-10-07

### Added

- *(assets)* carry image assets with tasks and documents across copies ([#3392](https://github.com/nickderobertis/onetaskgraph/pull/3392))

## [0.3.2](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.1...onetaskgraph-core-v0.3.2) - 2026-10-06

### Added

- *(projects)* render a project's description from a template, with provenance and stored answers ([#3334](https://github.com/nickderobertis/onetaskgraph/pull/3334))

## [0.3.1](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.3.0...onetaskgraph-core-v0.3.1) - 2026-10-06

### Added

- *(engine)* let a long-lived Engine end a command so sources drop what a person can change ([#3307](https://github.com/nickderobertis/onetaskgraph/pull/3307))

## [0.3.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.58...onetaskgraph-core-v0.3.0) - 2026-10-06

### Added

- *(status)* [**breaking**] scope status_mapping by item kind, refuse unmapped statuses, and drop Linear's by-type fallback ([#3262](https://github.com/nickderobertis/onetaskgraph/pull/3262))

## [0.2.58](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.57...onetaskgraph-core-v0.2.58) - 2026-10-03

### Performance

- *(github-projects)* search the board for project and document text ([#3231](https://github.com/nickderobertis/onetaskgraph/pull/3231))

## [0.2.56](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.55...onetaskgraph-core-v0.2.56) - 2026-10-02

### Added

- *(linear)* bring the Linear source to parity with GitHub Projects ([#3160](https://github.com/nickderobertis/onetaskgraph/pull/3160))

## [0.2.55](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.54...onetaskgraph-core-v0.2.55) - 2026-10-02

### Added

- *(store)* read items in batches and copy without repeating a lookup ([#3105](https://github.com/nickderobertis/onetaskgraph/pull/3105))

## [0.2.53](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.52...onetaskgraph-core-v0.2.53) - 2026-10-01

### Fixed

- *(copy)* keep a rewritten rendered document's body digest true of the copy ([#3021](https://github.com/nickderobertis/onetaskgraph/pull/3021))

## [0.2.52](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.51...onetaskgraph-core-v0.2.52) - 2026-09-29

### Added

- *(github-projects)* answer text, metadata and origin queries with GitHub's own search ([#2983](https://github.com/nickderobertis/onetaskgraph/pull/2983))
- *(copy)* record where an item was copied to and follow that link on every later copy ([#2967](https://github.com/nickderobertis/onetaskgraph/pull/2967))

## [0.2.50](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.49...onetaskgraph-core-v0.2.50) - 2026-09-29

### Added

- *(query)* filter tasks by comment activity with --commented-since ([#2916](https://github.com/nickderobertis/onetaskgraph/pull/2916))

### Fixed

- *(copy)* give a copied project's tasks ids scoped to the destination project ([#2929](https://github.com/nickderobertis/onetaskgraph/pull/2929))

## [0.2.49](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.48...onetaskgraph-core-v0.2.49) - 2026-09-28

### Fixed

- *(local-md)* update a task whose metadata holds block-scalar sequences ([#2887](https://github.com/nickderobertis/onetaskgraph/pull/2887))

## [0.2.48](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.47...onetaskgraph-core-v0.2.48) - 2026-09-28

### Added

- *(task)* add a targeted task update and classify budget exhaustion ([#2805](https://github.com/nickderobertis/onetaskgraph/pull/2805))

## [0.2.47](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.46...onetaskgraph-core-v0.2.47) - 2026-09-27

### Added

- *(templates)* create and regenerate items from templates, keeping answers out of the body ([#2716](https://github.com/nickderobertis/onetaskgraph/pull/2716))

## [0.2.46](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.45...onetaskgraph-core-v0.2.46) - 2026-09-27

### Added

- *(templates)* render minijinja templates with declared variables and answers ([#2654](https://github.com/nickderobertis/onetaskgraph/pull/2654))
- *(core)* expose a failure's class, kind, source and retry-after through typed accessors ([#2645](https://github.com/nickderobertis/onetaskgraph/pull/2645))

## [0.2.45](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.44...onetaskgraph-core-v0.2.45) - 2026-09-26

### Added

- *(task)* add a first-class task priority and a field-setup verb ([#2613](https://github.com/nickderobertis/onetaskgraph/pull/2613))

## [0.2.44](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.43...onetaskgraph-core-v0.2.44) - 2026-09-26

### Added

- *(task)* report the backend's short handle as a task's key ([#2585](https://github.com/nickderobertis/onetaskgraph/pull/2585))

### Documentation

- *(readme)* lead with hash-gated captures of the real CLI ([#2561](https://github.com/nickderobertis/onetaskgraph/pull/2561))

## [0.2.40](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.39...onetaskgraph-core-v0.2.40) - 2026-09-19

### Fixed

- *(build)* make the network destinations optional core features and pin sibling crates exactly ([#2113](https://github.com/nickderobertis/onetaskgraph/pull/2113))

## [0.2.39](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.38...onetaskgraph-core-v0.2.39) - 2026-09-18

### Fixed

- *(subprocess)* hand a hosted plugin the directory of the configuration document that named it ([#2023](https://github.com/nickderobertis/onetaskgraph/pull/2023))

## [0.2.37](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.36...onetaskgraph-core-v0.2.37) - 2026-09-18

### Fixed

- *(github-projects)* write issue state and Status together ([#1791](https://github.com/nickderobertis/onetaskgraph/pull/1791))

## [0.2.36](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.35...onetaskgraph-core-v0.2.36) - 2026-09-17

### Added

- *(github-projects)* add missing board status options safely ([#1610](https://github.com/nickderobertis/onetaskgraph/pull/1610))

## [0.2.34](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.33...onetaskgraph-core-v0.2.34) - 2026-09-16

### Added

- *(metadata)* add task, document and project metadata set verbs ([#1314](https://github.com/nickderobertis/onetaskgraph/pull/1314))

## [0.2.33](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.32...onetaskgraph-core-v0.2.33) - 2026-09-16

### Fixed

- *(local-md)* read the canonical in-progress word and resolve a file-relative root ([#1240](https://github.com/nickderobertis/onetaskgraph/pull/1240))

## [0.2.32](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.31...onetaskgraph-core-v0.2.32) - 2026-09-15

### Added

- *(status)* add a queued category, a status-only write, and a delivers relation ([#1145](https://github.com/nickderobertis/onetaskgraph/pull/1145))

## [0.2.31](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.30...onetaskgraph-core-v0.2.31) - 2026-09-14

### Added

- *(cli)* comment on a task with task comment add, list, edit and delete across every plugin ([#1073](https://github.com/nickderobertis/onetaskgraph/pull/1073))

## [0.2.30](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.29...onetaskgraph-core-v0.2.30) - 2026-09-14

### Added

- *(copy)* copy only a project's named members, and report what it spent ([#904](https://github.com/nickderobertis/onetaskgraph/pull/904))

## [0.2.29](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.28...onetaskgraph-core-v0.2.29) - 2026-09-14

### Added

- *(cli)* report a failed verb as a JSON document naming its retry class ([#903](https://github.com/nickderobertis/onetaskgraph/pull/903))

## [0.2.27](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.26...onetaskgraph-core-v0.2.27) - 2026-09-07

### Added

- *(copy)* point a copied document's references at the destination's own records ([#774](https://github.com/nickderobertis/onetaskgraph/pull/774))

## [0.2.23](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.22...onetaskgraph-core-v0.2.23) - 2026-09-05

### Fixed

- *(copy)* guard the copy path's pagination loops against a repeated cursor ([#397](https://github.com/nickderobertis/onetaskgraph/pull/397))

## [0.2.22](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.21...onetaskgraph-core-v0.2.22) - 2026-09-04

### Added

- *(github-projects)* account for what the live tests spend, reduce it, and refuse a run the account cannot afford ([#280](https://github.com/nickderobertis/onetaskgraph/pull/280))

## [0.2.21](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.20...onetaskgraph-core-v0.2.21) - 2026-09-02

### Fixed

- *(copy)* keep the destination's own origin when a copy comes back to it ([#252](https://github.com/nickderobertis/onetaskgraph/pull/252))

## [0.2.18](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.17...onetaskgraph-core-v0.2.18) - 2026-09-01

### Fixed

- *(github-projects)* read GitHub's secondary rate limiter as one, and stop the copy outrunning it ([#173](https://github.com/nickderobertis/onetaskgraph/pull/173))

## [0.2.17](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.16...onetaskgraph-core-v0.2.17) - 2026-09-01

### Added

- *(github-projects)* hold documents as design-titled issues and report their locations ([#161](https://github.com/nickderobertis/onetaskgraph/pull/161))

## [0.2.15](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.14...onetaskgraph-core-v0.2.15) - 2026-09-01

### Added

- *(in-memory)* serve documents and both location shapes ([#138](https://github.com/nickderobertis/onetaskgraph/pull/138))

## [0.2.14](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.13...onetaskgraph-core-v0.2.14) - 2026-09-01

### Added

- *(plugin-api)* give the source contract documents and locations ([#114](https://github.com/nickderobertis/onetaskgraph/pull/114))

## [0.2.13](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.12...onetaskgraph-core-v0.2.13) - 2026-08-30

### Fixed

- make project copy atomic, publish the npm package, and provision the pre-push gate ([#87](https://github.com/nickderobertis/onetaskgraph/pull/87))

## [0.2.12](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.11...onetaskgraph-core-v0.2.12) - 2026-08-29

### Fixed

- *(github-projects)* apply every predicate a query carries and declare it ([#65](https://github.com/nickderobertis/onetaskgraph/pull/65))

## [0.2.3](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.1...onetaskgraph-core-v0.2.3) - 2026-08-27

### Documentation

- present the Rust SDK and the Markdown authoring flow as first-class ([#37](https://github.com/nickderobertis/onetaskgraph/pull/37))

### Fixed

- *(core)* make the crate packageable with its README doctest intact ([#42](https://github.com/nickderobertis/onetaskgraph/pull/42))

## [0.2.2](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.2.1...onetaskgraph-core-v0.2.2) - 2026-08-27

### Fixed

- *(core)* make the crate packageable with its README doctest intact ([#42](https://github.com/nickderobertis/onetaskgraph/pull/42))

## [0.2.0](https://github.com/nickderobertis/onetaskgraph/compare/onetaskgraph-core-v0.1.0...onetaskgraph-core-v0.2.0) - 2026-08-26

### Added

- *(copy)* add the copy verb and the plugin write seam across the engine, CLI, and both SDKs ([#29](https://github.com/nickderobertis/onetaskgraph/pull/29))
- *(api)* [**breaking**] carry custom metadata, repositories, and edges that leave the project ([#25](https://github.com/nickderobertis/onetaskgraph/pull/25))

## [0.1.0](https://github.com/nickderobertis/onetaskgraph/releases/tag/onetaskgraph-core-v0.1.0) - 2026-08-25

### Added

- *(release)* automate versioning, publication and the proven end-user install path ([#15](https://github.com/nickderobertis/onetaskgraph/pull/15))
- *(github-projects)* draft the GitHub Projects source against its real v2 GraphQL shapes ([#14](https://github.com/nickderobertis/onetaskgraph/pull/14))
- *(linear)* draft the Linear source against its real GraphQL shapes, without live credentials ([#10](https://github.com/nickderobertis/onetaskgraph/pull/10))
- *(npm-sdk)* generate a typed TypeScript client that drives the real binary ([#8](https://github.com/nickderobertis/onetaskgraph/pull/8))
- *(core)* implement the stdio plugin protocol so an out-of-tree plugin needs no C ABI ([#6](https://github.com/nickderobertis/onetaskgraph/pull/6))
- *(local-md)* read tasks and projects from a folder of Markdown files ([#5](https://github.com/nickderobertis/onetaskgraph/pull/5))
- *(engine)* fan queries out across sources with capability-aware pushdown and a visible plan ([#4](https://github.com/nickderobertis/onetaskgraph/pull/4))
- *(config)* layer file, environment and flag configuration over a named-source registry ([#3](https://github.com/nickderobertis/onetaskgraph/pull/3))
- establish the onetaskgraph workspace, its gate, its CI and its plugin contract ([#1](https://github.com/nickderobertis/onetaskgraph/pull/1))
