//! [`RunMode`]: when a run starts unasked.

use serde::{Deserialize, Serialize};

/// When a run starts unasked: one setting of the editor, not of a document, so it follows a
/// person from one document to the next.
///
/// - **In On Change an open runs the previews**, as Houdini cooks the display node of a file it
///   loads ([cooking](https://www.sidefx.com/docs/houdini/basics/cooking.html)): a document shows
///   what it holds without an edit or a Run first.
/// - **On Change starts.** A change shows by itself, as Houdini's Auto Update, Dynamo's
///   Automatic ([interface](https://primer2.dynamobim.org/3_user_interface)) and Blender's
///   compositor run; an editor commits once a gesture, so a scrub runs on its release, as
///   Houdini's On Mouse Up does ([cooking](https://www.sidefx.com/docs/houdini/basics/cooking.html)).
///   Refused: Manual as the start, ComfyUI's and PixInsight's; the mode in the document,
///   Dynamo's.
/// - **A change runs the previews' cones alone.** A sink that is not a preview, a Save Image,
///   runs on Run alone, so moving a field writes no file. Refused: every sink, as Run does.
/// - **Only the previews the change reaches run.** A preview is a pure sink, so one whose inputs
///   fold to the digest it last showed is served as it stands and pulls nothing upstream, as
///   TouchDesigner cooks a requested node only for a reason
///   ([cook](https://derivative.ca/UserGuide/Cook)). Refused: darkroom working out what an edit
///   reaches, a second record of change beside scenarium's digests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunMode {
    /// Run alone starts one.
    Manual,
    /// An edit that changes what a run reads starts a run of the previews' cones.
    #[default]
    OnChange,
}

impl RunMode {
    /// The other mode: what the Run on Change toggle switches to.
    pub(crate) const fn toggled(self) -> Self {
        match self {
            Self::Manual => Self::OnChange,
            Self::OnChange => Self::Manual,
        }
    }
}
