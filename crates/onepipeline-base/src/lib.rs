//! Option (c)'s prototype crate onepipeline-base: written by scripts/crate-split/prototype-c.mjs.
#![allow(missing_docs, unreachable_pub, dead_code, unused, clippy::all)]
pub const VERSION: &str = "0.110.1";
#[path = "../../../src"]
mod src {
    pub mod provenance;
    pub mod hostphases;
    pub mod rendercost;
}
pub use src::provenance;
pub use src::hostphases;
pub use src::rendercost;
