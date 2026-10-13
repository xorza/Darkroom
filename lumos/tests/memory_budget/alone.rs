//! [`alone`]: each test's measurement in a process with nothing else to count.

use std::env;
use std::process::Command;
use std::thread;

/// Set in the child [`alone`] starts, so the body runs there rather than starting another.
const CHILD: &str = "LUMOS_MEMORY_BUDGET_ALONE";

/// Run `body` in a child process that runs this test alone.
///
/// The heap's count is every thread's but the harness's, and in one process other tests run
/// beside a measurement, and a finished test's thread frees what it held as it exits. In the
/// child, the only threads are the harness's, which [`LiveHeap`](crate::live_heap::LiveHeap)
/// leaves out, and the work's, so every byte counted is the measured work's, whichever of its
/// threads allocates it.
pub(crate) fn alone(body: impl FnOnce()) {
    if in_child() {
        body();
        return;
    }
    let name = thread::current()
        .name()
        .expect("the harness names each test's thread after the test")
        .to_owned();
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", &name, "--test-threads=1"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "{name} failed alone:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Whether this process is a child [`alone`] started, running one test and nothing beside it.
pub(crate) fn in_child() -> bool {
    env::var_os(CHILD).is_some()
}
