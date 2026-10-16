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
    if dir.join("dispatch-env.record-invocation").exists() {
        use std::io::Write;
        let record = std::env::var_os("ONEPIPELINE_E2E_HOOK_RECORD")
            .unwrap_or_else(|| fake::fail("the hook record directory is required"));
        let run = std::env::var("ONEPIPELINE_RUN_ID")
            .unwrap_or_else(|_| fake::fail("the run id is required"));
        let node = std::env::var("ONEPIPELINE_NODE_ID")
            .unwrap_or_else(|_| fake::fail("the node id is required"));
        let root = std::path::PathBuf::from(record).join(run);
        std::fs::create_dir_all(&root).expect("the run record directory");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("invocations"))
            .expect("the invocation record opens");
        file.write_all(format!("{node}\n").as_bytes())
            .expect("the invocation is recorded");
        file.sync_all().expect("the invocation is durable");
        drop(file);
    }
    fake::append(&dir.join("dispatch-env.entered"), "entered\n");
    println!("{{\"version\":1,\"env\":{{}}}}");
    if !startup {
        fake::meet(gate.trim());
    }
    // A killed hook can never reach this witness, even if it printed valid JSON.
    fake::append(&dir.join("dispatch-env.released"), "released\n");
}
