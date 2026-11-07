//! Other processes: the one route to starting one outside `common`, as `clippy.toml` holds.

use std::ffi::OsStr;
use std::io;
use std::process::{Command, Output};

use crate::platform;

/// Open `url` with the program the user's desktop opens it with, a browser for a web address,
/// and leave that program running. `on_failure` hears a failure, now or from another thread when
/// the program ends, as no system answers at once for all of them. One route for every failure on
/// every system. Refused: logging here, as this crate has no logger.
pub fn open_url(url: &str, on_failure: impl FnOnce(io::Error) + Send + 'static) {
    platform::open_url(url, on_failure);
}

/// Reach the terminal that started this binary, which a Windows GUI binary would not. Call it
/// first in `main`, ahead of any output.
pub fn attach_parent_console() {
    platform::attach_parent_console();
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
