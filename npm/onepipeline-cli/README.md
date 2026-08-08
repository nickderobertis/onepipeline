# onepipeline-cli

The `onepipeline` command, as a prebuilt binary.

```bash
npm install -g onepipeline-cli
onepipeline --help
```

The binary ships inside a per-platform package that npm selects by `os`/`cpu`, so
there is no compile step and no Rust toolchain to install. The same binary is on
[PyPI](https://pypi.org/project/onepipeline-cli/) (`pip install
onepipeline-cli`) and [crates.io](https://crates.io/crates/onepipeline)
(`cargo install onepipeline`).

See [the repository](https://github.com/nickderobertis/onepipeline) for what it
does and the contract it implements.
