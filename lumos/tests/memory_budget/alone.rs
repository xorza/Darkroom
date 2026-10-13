//! [`alone`]: each test's measurement in a process with nothing else to count.

use std::env;
use std::process::Command;
use std::thread;

/// Set in the child [`alone`] starts, so the body runs there rather than starting another.
const CHILD: &str = "LUMOS_MEMORY_BUDGET_ALONE";

/// Run `body` in a child process that runs this test alone.
///
/// The heap's count is the process's, and in one process the harness's own threads allocate
/// beside a measurement: the main thread recording a result and starting the next test, a
/// finished test's thread freeing what it held as it exits. In the child, the harness's main
/// thread waits on the one test it started, so every byte counted is the measured work's,
/// whichever of its threads allocates it.
pub(crate) fn alone(body: impl FnOnce()) {
    if env::var_os(CHILD).is_some() {
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
