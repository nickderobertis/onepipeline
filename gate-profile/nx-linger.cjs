// Preloaded into the Nx CLI process alone (nx-hook.sh adds it to NODE_OPTIONS for that process)
// while `just gate-profile` runs. The CPU a task used is exactly the jump in the CPU Nx counts for
// its reaped children, which sampler.mjs reads only while Nx is running; the last task's jump
// would otherwise be lost to Nx exiting within one sampling interval of it. So Nx waits, idle,
// for a few intervals before it exits. NODE_OPTIONS is put back first, so nothing Nx starts —
// no task, no plugin worker — loads this.
const before = process.env.ONEPIPELINE_GATE_PROFILE_NODE_OPTIONS;
if (before) process.env.NODE_OPTIONS = before;
else delete process.env.NODE_OPTIONS;
delete process.env.ONEPIPELINE_GATE_PROFILE_NODE_OPTIONS;
// The wait itself is proved by gate-profile.test.mjs, whose last real task is charged its
// grandchild's whole CPU only because Nx was sampled after reaping it; a test timing the wait
// would hold a figure to a fixed bound on a host whose load moves it.
const linger = Number(process.env.ONEPIPELINE_GATE_PROFILE_LINGER_MS);
if (Number.isInteger(linger) && linger > 0)
  process.on("exit", () => {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, linger);
  });
