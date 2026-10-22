//! Other processes: the one route to starting one outside `common`, as `clippy.toml` holds.

use std::ffi::OsStr;
use std::io;
use std::process::{Command, Output};

/// Start `program` with `args` and leave it running: nothing waits for it or reads its output.
pub fn spawn(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> io::Result<()> {
    Command::new(program).args(args).spawn().map(drop)
}

/// Run `program` with `args` and the environment it inherits plus `envs`, wait for it to exit,
/// and hand back its status and what it wrote, as `Command::output`.
pub fn output(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    envs: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
) -> io::Result<Output> {
    Command::new(program).args(args).envs(envs).output()
}
