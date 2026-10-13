//! A compiled hook held before its first record or after its complete output.
//! The caller controls the step through a loopback handshake, not a delay.

use onepipeline_testfakes as fake;

fn main() {
    let dir = fake::script_dir();
    let startup = dir.join("dispatch-env.startup").exists();
    let gate = fake::node_script(&dir, "dispatch-env", "rendezvous")
        .unwrap_or_else(|| fake::fail("dispatch-env.rendezvous must name the ordering peer"));
    if startup {
        fake::meet(gate.trim());
    }
    fake::append(&dir.join("dispatch-env.entered"), "entered\n");
    println!("{{\"version\":1,\"env\":{{}}}}");
    if !startup {
        fake::meet(gate.trim());
    }
    // A killed hook can never reach this witness, even if it printed valid JSON.
    fake::append(&dir.join("dispatch-env.released"), "released\n");
}
