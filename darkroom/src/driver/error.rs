//! Why a driver call failed, each case with what an agent needs to correct the call.

use std::io;
use std::path::PathBuf;

use palantir::GpuRequestError;

/// A driver call that could not do what it was asked.
#[expect(
    clippy::module_name_repetitions,
    reason = "an error carries its domain's name, as `ExecError` does"
)]
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
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
    #[error(
        "the editor quit, as an action asked (File ▸ Quit or Ctrl+Q), and the session ended; open \
         a project to go on"
    )]
    Quit,
    #[error("the editor did not settle within {frames} frames after {step}")]
    Unsettled { step: String, frames: u32 },
    #[error("the editor did not settle within {seconds} s after {step}")]
    UnsettledInTime { step: String, seconds: f32 },
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
    #[error("a pinch factor of {factor} zooms nowhere; give a positive one, above 1 to zoom in")]
    BadPinch { factor: f32 },
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
    #[error(
        "{} holds paths that climb out of its folder ({}), which a copy of the folder alone would \
         point at other files; open it in place",
        .document.display(),
        .paths.join(", ")
    )]
    PathsLeaveFolder {
        document: PathBuf,
        paths: Vec<String>,
    },
    #[error(
        "the editor did not open {}: {}",
        .path.display(),
        .status.as_deref().unwrap_or("it said nothing")
    )]
    NotOpened {
        path: PathBuf,
        /// The editor's status line after the open, which names why: the load's error, as the
        /// editor reports it to a person.
        status: Option<String>,
    },
    #[error(
        "{} has unsaved edits, which an open would discard; save them or undo them first",
        .document
            .as_ref()
            .map_or_else(|| "the new document".into(), |path| path.display().to_string())
    )]
    Unsaved { document: Option<PathBuf> },
    #[error(
        "the editor waits on a native file dialog, which only the person at the window answers"
    )]
    NativeDialog,
    #[error("the editor did not answer within {seconds} s")]
    NoAnswer { seconds: f32 },
    #[error("the GPU display needs an adapter, and none is usable: {0}")]
    NoAdapter(#[from] GpuRequestError),
    #[error("{} has no parent folder to copy", .path.display())]
    NoFolder { path: PathBuf },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
