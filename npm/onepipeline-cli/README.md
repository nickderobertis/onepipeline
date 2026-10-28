# onepipeline-cli

The `onepipeline` command, as a prebuilt binary.

```bash
npm install -g onepipeline-cli
onepipeline --help
```

The binary ships inside a per-platform package that npm selects by `os`/`cpu`, so
there is no compile step and no Rust toolchain to install.

On Linux and macOS the `onepipeline` command *becomes* the binary's process
(`process.execve`), so it needs Node.js 22.15+ or 23.11+ and refuses an older
one. On Windows, which has no exec, it runs the binary under a small keeper
process that passes its exit status and console interrupts through and never
leaves it running once the command has ended. The same binary is on
[PyPI](https://pypi.org/project/onepipeline-cli/) (`pip install
onepipeline-cli`) and [crates.io](https://crates.io/crates/onepipeline)
(`cargo install onepipeline`).

See [the repository](https://github.com/nickderobertis/onepipeline) for what it
does and the contract it implements.
