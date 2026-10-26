//! Why a harness call failed, each case with what an agent needs to correct the call.

use std::io;
use std::path::PathBuf;

/// A harness call that could not do what it was asked.
#[expect(
    clippy::module_name_repetitions,
    reason = "an error carries its domain's name, as `ExecError` does"
)]
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("{locator} matches nothing; the nearest are: {}", .nearest.join(", "))]
    NotFound {
        locator: String,
        nearest: Vec<String>,
    },
    #[error("{locator} matches {} widgets, name one of: {}", .matches.len(), .matches.join(", "))]
    Ambiguous {
        locator: String,
        matches: Vec<String>,
    },
    #[error("{locator} is in the document but not on screen; pan or zoom the canvas to it")]
    OffScreen { locator: String },
    #[error("the editor did not settle within {frames} frames after {step}")]
    Unsettled { step: String, frames: u32 },
    #[error("{region} covers none of the window; pick a region inside it")]
    EmptyRegion { region: String },
    #[error("a shot needs pixels, and this session has none: start it with the GPU display")]
    NoGpu,
    #[error("the run did not finish within {seconds} s")]
    RunTimeout { seconds: f32 },
    #[error(
        "'{chord}' is not a key chord: write modifiers and one key joined by '+', as ctrl+shift+z, \
         enter, escape, f2, delete or a single character"
    )]
    BadChord { chord: String },
    #[error(
        "the project folder {} holds {bytes} bytes, over the {limit}-byte copy limit; open it \
         in place instead",
        .folder.display()
    )]
    FolderTooLarge {
        folder: PathBuf,
        bytes: u64,
        limit: u64,
    },
    #[error(
        "{} links to a folder, which a copy cannot follow safely; open the project in place",
        .path.display()
    )]
    LinkedFolder { path: PathBuf },
    #[error("{} has no parent folder to copy", .path.display())]
    NoFolder { path: PathBuf },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
