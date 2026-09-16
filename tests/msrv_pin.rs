//! The minimum supported Rust version is declared in one file and configured in
//! another, and they agree.
//!
//! `Cargo.toml`'s `workspace.package.rust-version` is the declaration: what a
//! consumer of the published crate is promised, what `just msrv` reads to pick a
//! toolchain, and what CI's `msrv` job builds under. `clippy.toml`'s `msrv` is
//! what keeps clippy from *suggesting* an API newer than that promise. Nothing
//! derives one from the other — clippy reads its own file, cargo reads the
//! manifest — so a floor raised in one and left behind in the other leaves the
//! deterministic gate green while clippy advises rewrites the declared floor
//! cannot compile, or refuses ones it can.
//!
//! This is the drift gate for that pair, in the shape `patch_pin.rs` holds the
//! other one in: both real files, parsed as the TOML they are, and held equal.

use std::path::Path;

fn read(name: &str) -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{name} reads: {error}"));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{name} parses: {error}"))
}

/// The declared floor, refused rather than defaulted when it is absent: a
/// manifest that stopped declaring one promises nothing, and comparing against
/// "no floor" would pass on exactly that.
fn declared() -> String {
    read("Cargo.toml")["workspace"]["package"]["rust-version"]
        .as_str()
        .expect("Cargo.toml's workspace.package declares rust-version")
        .to_owned()
}

fn configured() -> String {
    read("clippy.toml")["msrv"]
        .as_str()
        .expect("clippy.toml sets msrv")
        .to_owned()
}

#[test]
fn clippy_is_configured_at_the_rust_version_the_manifest_declares() {
    let declared = declared();
    let configured = configured();
    assert_eq!(
        declared, configured,
        "`Cargo.toml` declares rust-version {declared} and `clippy.toml` sets msrv \
         {configured}; raise both in one change"
    );
}
