//! The tools' inputs as an agent writes them, and their conversion to the driver's types. The
//! JSON shape is this crate's: the driver's types carry no schema.

use std::path::PathBuf;

use darkroom::DialogAnswer;
use darkroom::driver::OpenMode;
use darkroom::driver::action::{Action, Chord, DRAG_STEPS};
use darkroom::driver::error::DriverError;
use darkroom::driver::locator::{Locator, Side};
use darkroom::driver::shot::Region;
use darkroom::driver::state::Detail;
use glam::Vec2;
use palantir::{Modifiers, PointerButton, Rect};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

/// The longer side of a shot when the call names none: enough to read a node's labels in a
/// whole window, at about 1,300 tokens for a 16:10 picture.
pub(crate) const MAX_SIDE: u32 = 1280;
/// The widgets `darkroom_ui` lists when the call names no limit.
pub(crate) const UI_LIMIT: usize = 50;
/// Seconds `darkroom_wait_run` waits when the call names no timeout.
pub(crate) const RUN_TIMEOUT_S: f32 = 60.0;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct OpenParams {
    /// The `.darkroom` document to open, absolute or relative to the server's working directory;
    /// absent, a new unsaved document, which File ▸ Save As names.
    pub(crate) project: Option<PathBuf>,
    /// Copy the project's folder into the session's directory in RAM and open the copy, so
    /// nothing the session does reaches the original. By default the project opens where it is,
    /// as a person's Open does.
    #[serde(default)]
    pub(crate) copy: bool,
}

impl OpenParams {
    pub(crate) const fn mode(&self) -> OpenMode {
        if self.copy {
            OpenMode::Copy
        } else {
            OpenMode::InPlace
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ActParams {
    /// The steps, played in order; the editor settles after each. The first that fails stops the
    /// rest.
    pub(crate) actions: Vec<ActionParam>,
    /// A shot to take after the last action.
    pub(crate) shot: Option<ShotParams>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct StateParams {
    /// `concise` (the default): nodes, selection, focus, status. `detailed` adds each node's id,
    /// ports, bindings and run status, the wires, the camera and the undo.
    #[serde(default)]
    pub(crate) detail: DetailParam,
    /// Only the nodes whose name contains this, and the wires with one at either end.
    pub(crate) nodes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct UiParams {
    /// Only the widgets whose text contains this, ignoring case.
    pub(crate) filter: Option<String>,
    /// The most widgets to list, 50 by default; the reply says how many it left out.
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ShotParams {
    /// What to picture; the whole window by default.
    #[serde(default)]
    pub(crate) region: RegionParam,
    /// The picture's longer side at most, in pixels; 1280 by default.
    pub(crate) max_side: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct WaitRunParams {
    /// Seconds to wait for the run, 60 by default.
    pub(crate) timeout_s: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetailParam {
    #[default]
    Concise,
    Detailed,
}

/// One step of input.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionParam {
    /// Move the pointer there, as a hover.
    Move(LocatorParam),
    /// Press and release a button there.
    Click {
        at: LocatorParam,
        #[serde(default)]
        button: ButtonParam,
        /// Modifiers held through the click.
        #[serde(default)]
        mods: Vec<ModParam>,
    },
    /// Two left clicks there, as one double click.
    DoubleClick(LocatorParam),
    /// Press at `from`, move to `to`, release there.
    Drag {
        from: LocatorParam,
        to: LocatorParam,
        #[serde(default)]
        button: ButtonParam,
        #[serde(default)]
        mods: Vec<ModParam>,
        /// Pointer moves on the way, 8 by default.
        steps: Option<u32>,
    },
    /// Scroll there by `delta` logical pixels; a positive `y` scrolls content down.
    Scroll { at: LocatorParam, delta: [f32; 2] },
    /// Pinch there by `factor`; above 1 zooms in.
    Pinch { at: LocatorParam, factor: f32 },
    /// One key chord: modifiers and one key joined by `+`, as `ctrl+z`, `shift+delete`, `enter`,
    /// `escape`, `f2` or `a`. `ctrl` is Cmd on macOS.
    Key(String),
    /// Type text into the focused field.
    Type(String),
    /// The answer the next file dialog gets. Queue it before the action that opens the dialog.
    Answer(AnswerParam),
}

/// A place in the window: by what the editor shows first, by text next, by position last.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocatorParam {
    /// A node's body, by its name or its id.
    Node(String),
    /// A node's title bar, by its name or id: drag it to move the node, double-click to rename.
    Header(String),
    /// A port circle: the node by name or id, `in` or `out`, the port by name or index.
    Port {
        node: String,
        side: SideParam,
        port: String,
    },
    /// A point of the graph canvas, `[x, y]` in the coordinates nodes are placed in.
    World([f32; 2]),
    /// The one visible interactive widget whose text is exactly this: a button, a menu item, a
    /// tab, a field.
    Text(String),
    /// A point of the window, `[x, y]` in logical pixels.
    Screen([f32; 2]),
    /// Another locator's point, moved by `[dx, dy]` logical pixels.
    Offset { of: Box<Self>, by: [f32; 2] },
    /// The match at `index`, from 0, of a `node` or `text` locator that matches several, in the
    /// order an ambiguity error lists them.
    Nth { of: Box<Self>, index: usize },
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum SideParam {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum ButtonParam {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModParam {
    /// Cmd on macOS.
    Ctrl,
    Shift,
    Alt,
    Meta,
}

/// A file dialog's answer.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum AnswerParam {
    /// One path, for every dialog but a pick of several files.
    Path(PathBuf),
    /// The paths of a pick of several files.
    Paths(Vec<PathBuf>),
    /// The user closed the dialog.
    Cancel,
}

/// What a shot pictures.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegionParam {
    #[default]
    Window,
    /// The pane whose visible tab has this name, with its tab strip.
    Pane(String),
    /// A node by name or id, with a 24 px margin.
    Node(String),
    /// A rect of the canvas, `[x, y, w, h]` in the coordinates nodes are placed in.
    World([f32; 4]),
}

impl From<DetailParam> for Detail {
    fn from(detail: DetailParam) -> Self {
        match detail {
            DetailParam::Concise => Self::Concise,
            DetailParam::Detailed => Self::Detailed,
        }
    }
}

/// A chord is parsed here, so a batch with a bad one is refused before any of it plays.
impl TryFrom<ActionParam> for Action {
    type Error = DriverError;

    fn try_from(action: ActionParam) -> Result<Self, DriverError> {
        Ok(match action {
            ActionParam::Move(at) => Self::Move(at.into()),
            ActionParam::Click { at, button, mods } => Self::Click {
                at: at.into(),
                button: button.into(),
                mods: modifiers(&mods),
            },
            ActionParam::DoubleClick(at) => Self::DoubleClick(at.into()),
            ActionParam::Drag {
                from,
                to,
                button,
                mods,
                steps,
            } => Self::Drag {
                from: from.into(),
                to: to.into(),
                button: button.into(),
                mods: modifiers(&mods),
                steps: steps.unwrap_or(DRAG_STEPS),
            },
            ActionParam::Scroll { at, delta } => Self::Scroll {
                at: at.into(),
                delta: Vec2::from(delta),
            },
            ActionParam::Pinch { at, factor } => Self::Pinch {
                at: at.into(),
                factor,
            },
            ActionParam::Key(chord) => Self::Key(Chord::parse(&chord)?),
            ActionParam::Type(text) => Self::Type(text),
            ActionParam::Answer(answer) => Self::Answer(answer.into()),
        })
    }
}

impl From<LocatorParam> for Locator {
    fn from(locator: LocatorParam) -> Self {
        match locator {
            LocatorParam::Node(node) => Self::Node(node),
            LocatorParam::Header(node) => Self::NodeHeader(node),
            LocatorParam::Port { node, side, port } => Self::Port {
                node,
                side: match side {
                    SideParam::In => Side::In,
                    SideParam::Out => Side::Out,
                },
                port,
            },
            LocatorParam::World(point) => Self::World(Vec2::from(point)),
            LocatorParam::Text(text) => Self::Text(text),
            LocatorParam::Screen(point) => Self::Screen(Vec2::from(point)),
            LocatorParam::Offset { of, by } => Self::Offset(Box::new((*of).into()), Vec2::from(by)),
            LocatorParam::Nth { of, index } => Self::Nth(Box::new((*of).into()), index),
        }
    }
}

impl From<ButtonParam> for PointerButton {
    fn from(button: ButtonParam) -> Self {
        match button {
            ButtonParam::Left => Self::Left,
            ButtonParam::Right => Self::Right,
            ButtonParam::Middle => Self::Middle,
        }
    }
}

impl From<AnswerParam> for DialogAnswer {
    fn from(answer: AnswerParam) -> Self {
        match answer {
            AnswerParam::Path(path) => Self::Path(path),
            AnswerParam::Paths(paths) => Self::Paths(paths),
            AnswerParam::Cancel => Self::Cancel,
        }
    }
}

impl From<RegionParam> for Region {
    fn from(region: RegionParam) -> Self {
        match region {
            RegionParam::Window => Self::Window,
            RegionParam::Pane(name) => Self::Pane(name),
            RegionParam::Node(node) => Self::Node(node),
            RegionParam::World([x, y, w, h]) => Self::World(Rect::new(x, y, w, h)),
        }
    }
}

/// The modifiers `mods` names held at once.
fn modifiers(mods: &[ModParam]) -> Modifiers {
    let mut held = Modifiers::default();
    for modifier in mods {
        match modifier {
            ModParam::Ctrl => held.ctrl = true,
            ModParam::Shift => held.shift = true,
            ModParam::Alt => held.alt = true,
            ModParam::Meta => held.meta = true,
        }
    }
    held
}
