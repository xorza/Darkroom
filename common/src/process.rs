//! Other processes: the one route to starting one outside `common`, as `clippy.toml` holds.

use std::ffi::OsStr;
use std::io;
use std::process::{Command, Output};

use crate::platform;

/// Open `url` with the program the user's desktop opens it with, a browser for a web address,
/// and leave that program running.
pub fn open_url(url: &str) -> io::Result<()> {
    platform::open_url(url)
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
