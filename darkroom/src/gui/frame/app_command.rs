//! [`AppCommand`]: what a surface asks of the shell, by group — the document
//! file, the worker, the preferences, and edits that need a dialog first.
//!
//! Data only. `App::run_command` is what each one does: the work bridges the
//! open document, the runtime, the preferences and the dialogs, so it belongs
//! to the owner of all of them rather than to the command.

use std::path::PathBuf;
use std::sync::Arc;

use lens::MlModelPaths;
use scenarium::{FsPathConfig, InputPort, NodeId};

/// A command a UI surface (the menu bar, the keyboard chords, the graph
/// toolbar, a node's header chips and context menu, an inline path-picker, the
/// Preferences tab, a viewer's toolbar) hands to `App`. The producing UI never
/// touches the document / `Theme` / runtime services directly.
///
/// Everything here needs `&mut App`, a blocking dialog, or both, so none of it
/// can run during the pass that raises it. A UI action that only rearranges
/// panes is not one of these — it is a
/// [`DockOperation`](palantir::DockOperation) on the frame's queue.
#[derive(Clone, Debug)]
pub(crate) enum AppCommand {
    /// Document file lifecycle.
    File(FileCommand),
    /// Graph execution + worker event loop.
    Run(RunCommand),
    /// Preferences edits.
    Prefs(PrefsCommand),
    /// Node edits raised via a dialog.
    Edit(EditCommand),
    /// Quit the app, through the unsaved-changes prompt.
    Quit,
}

/// Document file lifecycle: new / open / save / save-as.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileCommand {
    /// Replace the document with an empty one.
    New,
    /// Prompt for a file and open it.
    Open,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    Save,
    /// Always prompt for a destination.
    SaveAs,
}

/// Graph execution + the worker event loop. Commands only request work;
/// worker status reports drive the toolbar's execution and loop state.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Evaluate one node's upstream cone and deliver its outputs.
    Node(NodeId),
    /// Remove one authored node's compiled runtime-cache cone from RAM and disk.
    EvictCache(NodeId),
    /// Write one authored node's resident value to the disk store now — raised
    /// when its cache mode gains the disk bit.
    FlushCache(NodeId),
    /// Request cancellation of the in-flight run.
    Cancel,
    /// Start the worker's event loop (emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
}

/// Preferences UI actions.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PrefsCommand {
    /// A field of [`crate::core::io::preferences::Preferences`] was edited
    /// in place — by the Preferences tab (any checkbox / radio / path field)
    /// or the image viewer's toolbar (backdrop / sampling). `App` synchronizes
    /// derived state and persists it — one command for every field, so adding
    /// a preference needs no new command.
    Changed,
    PickMlModel(MlModelKind),
}

/// Which of the ML nodes' model files a Browse pick sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MlModelKind {
    Denoise,
    StarRemoval,
}

impl MlModelKind {
    /// The path this kind names in `paths`.
    pub(crate) const fn path_mut(self, paths: &mut MlModelPaths) -> &mut PathBuf {
        match self {
            Self::Denoise => &mut paths.denoise,
            Self::StarRemoval => &mut paths.star_removal,
        }
    }
}

/// Node edits that need a blocking dialog before applying. The dialog opens
/// after UI authoring, then the chosen paths land as an ordinary undoable
/// `SetInput` edit.
#[derive(Clone, Debug)]
pub(crate) enum EditCommand {
    /// Open a file dialog (filtered by the pick's picker config) for a
    /// node's `FsPath` const input, applying the chosen paths as a `SetInput`
    /// edit. Raised by the inline pick button, from the port row's value cell.
    PickInputPath(PathPick),
}

/// A click on an `FsPath` input's inline pick button: the port to set, and the
/// picker config to open the dialog with.
///
/// A named payload rather than the two values loose, because it travels as an
/// `AppCommand` through `App` and back — the dialog can only run once the
/// record pass is over, so the click and the effect are frames apart.
#[derive(Clone, Debug)]
pub(crate) struct PathPick {
    pub(crate) port: InputPort,
    /// Type-level metadata, taken from the port's `DataType` — the value
    /// carries only the selected path strings.
    pub(crate) config: Arc<FsPathConfig>,
}
