# privateterms

The Rust SDK of the [private-terms specification](https://github.com/nickderobertis/privateterms/blob/main/spec/private-terms.md):
declare the private terms a repository, a directory or a host must never publish, then
detect, replace or reject them in text — and screen the identifiers a diagnostic would
print — exactly as the specification and its conformance suite define.

```toml
[dependencies]
privateterms = "0.1"
```

## Use

```rust
use privateterms::{resolve, Matcher, Replacement, TermMode, TermRule};

// The repository's own derived rules (derivation is the caller's), then the levels
// and the caller's replacement list.
let rules = vec![(
    TermRule { term: "quillmoss".into(), mode: TermMode::WholeWord, case_sensitive: false },
    "derived".to_owned(),
)];
let caller = [Replacement::Replace { term: "quillmoss".into(), with: "example".into() }];
let resolved = resolve(rules, &[], &caller).unwrap();

let matcher = Matcher::new(resolved, &[]).unwrap();
assert_eq!(matcher.hits("see Ｑuillmoss")[0].span, 4..15); // spans are of the original text
let replaced = matcher.replace("see quillmoss-core");
assert_eq!(replaced.text, "see example-core");
assert!(replaced.remaining.is_empty()); // non-empty would be a refusal
```

`Matcher::screen_locators` and `redact_all` give the locator outcomes of a check and of
an unavailable one; every error names the problem and never a term, and a
`DeclarationError` keeps its file in a separate `path` field that neither its `Display`
nor its `Debug` prints.

## Storage bindings (the crate's, not the specification's)

The specification defines a storage-independent model and no file format, file name or
directory layout. These bindings are **this crate's**, the ones onevcs and onetaskgraph
consume:

- **File names.** A declaration file is `private-terms.toml`, `private-terms.yaml` or
  `private-terms.yml` (`FILE_NAMES`). One directory holding two of them is refused as
  ambiguous (`DeclarationErrorKind::AmbiguousFiles`).
- **Encodings.** TOML and YAML are one encoding of the model each: `schema_version`,
  `terms`, `exceptions` and, from version 2, `replacements` (each `with = "…"` or
  `reject = true`) and `phrases`. These two documents parse to the same `Declaration`:

  ```rust
  use privateterms::{parse, Format, LevelKind};

  let toml = br#"
  schema_version = 2
  terms = ["quillmoss"]
  phrases = ["quillmoss-bridge"]

  [[replacements]]
  term = "quillmoss"
  with = "example-project"

  [[replacements]]
  term = "vantorel"
  reject = true
  "#;
  let yaml = br#"
  schema_version: 2
  terms: [quillmoss]
  phrases: [quillmoss-bridge]
  replacements:
    - term: quillmoss
      with: example-project
    - term: vantorel
      reject: true
  "#;
  assert_eq!(
      parse(toml, Format::Toml, LevelKind::Directory).unwrap(),
      parse(yaml, Format::Yaml, LevelKind::Directory).unwrap(),
  );
  ```

  `to_toml` and `to_yaml` write what `parse` reads back. YAML is read and written only
  through `serde-saphyr`, under one hardened reader: `yes`/`no`/`on`/`off` stay strings,
  `<<` is an ordinary key, a duplicate key or a custom tag is refused, errors carry no
  snippet of the document, and a budget bounds nesting depth, alias expansion and size
  (1 MiB, in either encoding). A refused document is refused whole.
- **Level discovery.** `discover_levels(checkout, home, fallback)` returns, nearest
  first: the repository's own file at the checkout's root; each ancestor directory of
  the checkout up to and including `home` (for a checkout outside `home`, up to but
  excluding the filesystem root); then the `fallback` directory a host names.
  `read_dir_level` reads one directory.

## Versioning

Pre-1.0: a breaking change to the public API is a minor release. `SPEC_VERSION` names the
specification version the crate implements.

## Benchmark

Run `cargo bench -p privateterms --bench clean_runs --locked` from the repository.
It reports median times and their ratio for visiting characters versus calling `hits`
on the same 75,084 clean printable runs (over 2.5 MiB, at least one third containing non-ASCII),
with 60 private terms. One run in twenty carries a case or normalization variant
of a term fragment, never a complete term. Matcher construction and an initial correctness/warmup pass
are outside the timings. Each sample averages 20 passes; the report takes the median
of 21 samples, alternating which side runs first to reduce host-load bias.
There is no wall-clock assertion.

Clean ASCII and Unicode text whose characters are inert under NFKC and unchanged by
lowercasing can use a conservative candidate search without normalization or span
mapping. Other Unicode text follows the full normalization path. The Unicode safety
lookup is shared across matchers and initialized once on its first use (about 1 MiB
of process memory); the benchmark's warmup includes that initialization. A reported
hit still uses the original normalization/span mapping, including its widening rules.
