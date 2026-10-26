//! [`DarkroomServer`]: the editor's tools, each a job on the editor's thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use darkroom::internals::harness::action::Action;
use darkroom::internals::harness::shot::{LegendNode, Region};
use darkroom::internals::harness::state::{Detail, State};
use darkroom::internals::harness::{ActReport, Harness, SessionConfig};
use glam::UVec2;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, PromptMessage, Role, ServerCapabilities,
    ServerConfig,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::params::{
    ActParams, MAX_SIDE, OpenParams, RUN_TIMEOUT_S, ShotParams, StateParams, UI_LIMIT, UiParams,
    WaitRunParams,
};
use crate::session_thread::{Session, SessionThread, ToolError};

/// The largest window side a session takes, in physical pixels: wgpu's default
/// `max_texture_dimension_2d`, which desktop adapters reach. palantir asks the adapter for its
/// own limit, so an adapter below it panics on a larger window, and the session ends.
const MAX_WINDOW_SIDE: u32 = 8192;
/// The smallest scale a session takes: at a quarter, a 12 px label is 3 pixels high.
const MIN_SCALE: f32 = 0.25;

/// What the server tells an agent before its first call.
const INSTRUCTIONS: &str = "Drives the darkroom node editor with no desktop. Open a project with \
darkroom_open, play clicks, drags and keys with darkroom_act, read the editor as text with \
darkroom_state and darkroom_ui, and look at it with darkroom_shot. Name things by what the editor \
shows: a node by its name, a port by node, side and name, a widget by its text; use screen \
points only for what has no name. Read state before a shot: text costs less.";

/// The MCP server: its tools reach the one editor session through its thread.
#[derive(Debug, Clone)]
pub(crate) struct DarkroomServer {
    session: Arc<SessionThread>,
    tool_router: ToolRouter<Self>,
}

/// What `darkroom_open` answers.
#[derive(Debug, Serialize)]
struct Opened {
    /// The commit the server was built from, and whether the tree differed: a server older than
    /// the source says so here.
    build: &'static str,
    /// Where this session's copy, saves and shots live.
    session_dir: PathBuf,
    state: State,
}

/// What a shot answers beside its picture.
#[derive(Debug, Serialize)]
struct ShotLegend<'a> {
    path: &'a PathBuf,
    /// `[width, height]` of the picture.
    size: [u32; 2],
    nodes: &'a [LegendNode],
}

#[tool_router]
impl DarkroomServer {
    /// A server whose sessions are made with `defaults` where `darkroom_open` names nothing else.
    pub(crate) fn new(defaults: SessionConfig) -> Self {
        Self {
            session: Arc::new(SessionThread::new(defaults)),
            tool_router: Self::tool_router(),
        }
    }

    /// Run `job` on the editor's thread.
    pub(crate) async fn run(
        &self,
        job: impl FnOnce(&mut Session) -> Result<Vec<ContentBlock>, ToolError> + Send + 'static,
    ) -> CallToolResult {
        self.session.run(job).await
    }

    #[tool(
        description = "Open a .darkroom project in a new editor session, closing any open one; \
        with no project, a new unsaved document, which a Save As names. By default the project's \
        folder is copied into the session's own directory in RAM and the copy opens, so nothing \
        the session does reaches the original; in_place opens the original. \
        Answers with the build the server runs, the session directory, and the editor's state. \
        Example: {\"project\": \"darkroom/sample/sample.darkroom\"}"
    )]
    async fn darkroom_open(&self, Parameters(params): Parameters<OpenParams>) -> CallToolResult {
        self.run(move |session| {
            let mut config = session.defaults;
            if let Some([w, h]) = params.size {
                config.size = UVec2::new(w, h);
            }
            if let Some(scale) = params.scale {
                config.scale = scale;
            }
            check_config(config)?;
            let harness = session.start(config, params.project.as_deref(), params.in_place)?;
            json(&Opened {
                build: env!("DARKROOM_BUILD"),
                session_dir: harness.session_dir().to_path_buf(),
                state: harness.state(Detail::Concise, None),
            })
        })
        .await
    }

    #[tool(
        description = "Play input into the editor: clicks, double clicks, drags, scrolls, \
        pinches, key chords, typed text, and the answers file dialogs take. The editor settles \
        after each action; the first that fails stops the rest. Name each place with a locator: \
        node, header, port, text, world, screen, offset or nth. Answers with the selection, the \
        dirty flag and the focus, and with a shot when one is asked. Example: {\"actions\": \
        [{\"click\": {\"at\": {\"node\": \"Blur\"}}}, {\"drag\": {\"from\": {\"port\": {\"node\": \
        \"Load Image\", \"side\": \"out\", \"port\": \"Image\"}}, \"to\": {\"port\": {\"node\": \
        \"Blur\", \"side\": \"in\", \"port\": \"Image\"}}}}, {\"key\": \"ctrl+s\"}], \"shot\": \
        {\"region\": {\"node\": \"Blur\"}}}"
    )]
    async fn darkroom_act(&self, Parameters(params): Parameters<ActParams>) -> CallToolResult {
        self.run(move |session| {
            let harness = session.harness()?;
            let count = params.actions.len();
            // One at a time, so a failure names the action it stopped at.
            let mut report = harness.act(&[])?;
            for (idx, action) in params.actions.into_iter().enumerate() {
                let step = harness.act(&[Action::from(action)]).map_err(|error| {
                    ToolError(format!(
                        "action {} of {count} failed, and the ones before it ran: {error}",
                        idx + 1
                    ))
                })?;
                report = ActReport {
                    frames: report.frames + step.frames,
                    ..step
                };
            }
            let mut content = json(&report)?;
            if let Some(shot) = params.shot {
                content.extend(take_shot(harness, shot)?);
            }
            Ok(content)
        })
        .await
    }

    #[tool(
        description = "The editor as text: the open document, whether it has unsaved edits, \
        each node by name with its rect on screen, the selection, the focus, the status line, and \
        the warnings and errors logged since the last call. detailed adds each node's id, ports, \
        bindings and run status, the wires, the camera and whether an undo is possible; nodes \
        keeps the nodes whose name contains it. Example: {\"detail\": \"detailed\", \"nodes\": \
        \"Blur\"}"
    )]
    async fn darkroom_state(&self, Parameters(params): Parameters<StateParams>) -> CallToolResult {
        self.run(move |session| {
            let harness = session.harness()?;
            json(&harness.state(params.detail.into(), params.nodes.as_deref()))
        })
        .await
    }

    #[tool(
        description = "The visible widgets that take input, in paint order: each one's text, \
        its rect in logical pixels, and whether it clicks, drags, takes the focus or is disabled. \
        Use it to find the text a text locator names. filter keeps the widgets whose text contains \
        it, ignoring case; limit caps the list at 50 by default and the answer says how many it \
        left out. Example: {\"filter\": \"save\"}"
    )]
    async fn darkroom_ui(&self, Parameters(params): Parameters<UiParams>) -> CallToolResult {
        self.run(move |session| {
            let harness = session.harness()?;
            json(&harness.ui(params.filter.as_deref(), params.limit.unwrap_or(UI_LIMIT)))
        })
        .await
    }

    #[tool(
        description = "A picture of the editor as a user's screenshot gives it: the window, a \
        pane by its tab's name, a node by name with a margin, or a rect of the canvas. It is \
        scaled so that its longer side is at most max_side pixels, 1280 by default, and comes with \
        a legend of each visible node and port at its rect in the picture's pixels, so a point \
        seen in it becomes a locator. Needs the GPU display. Read darkroom_state first: text \
        costs fewer tokens. Example: {\"region\": {\"node\": \"Blur\"}, \"max_side\": 800}"
    )]
    async fn darkroom_shot(&self, Parameters(params): Parameters<ShotParams>) -> CallToolResult {
        self.run(move |session| take_shot(session.harness()?, params))
            .await
    }

    #[tool(
        description = "Wait until the graph's run on the editor's worker finishes and the \
        editor settles, for at most timeout_s seconds, 60 by default. Start a run first, as with \
        the toolbar's run button or a node's run chip. Example: {\"timeout_s\": 120}"
    )]
    async fn darkroom_wait_run(
        &self,
        Parameters(params): Parameters<WaitRunParams>,
    ) -> CallToolResult {
        self.run(move |session| {
            let seconds = params.timeout_s.unwrap_or(RUN_TIMEOUT_S);
            let timeout = Duration::try_from_secs_f32(seconds).map_err(|_| {
                ToolError(format!(
                    "timeout_s is {seconds}; give a number of seconds from 0"
                ))
            })?;
            let harness = session.harness()?;
            harness.wait_run(timeout)?;
            json(&harness.state(Detail::Concise, None))
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
#[expect(
    clippy::unused_async_trait_impl,
    reason = "the async trait fns are rmcp's `tool_handler`'s, which await the router alone"
)]
impl ServerHandler for DarkroomServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("darkroom", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// A shot of `params`' region, as an image and its legend.
fn take_shot(harness: &mut Harness, params: ShotParams) -> Result<Vec<ContentBlock>, ToolError> {
    let max_side = params.max_side.unwrap_or(MAX_SIDE);
    if max_side == 0 {
        return Err(ToolError(
            "max_side is 0; give the picture's longer side in pixels".to_string(),
        ));
    }
    let shot = harness.shot(&Region::from(params.region), max_side)?;
    // rmcp's own base64 encoder, which its prompt messages reach.
    let image = PromptMessage::new_image(Role::Assistant, &shot.png, "image/png", None, None);
    let mut content = vec![image.content];
    content.extend(json(&ShotLegend {
        path: &shot.path,
        size: shot.size.to_array(),
        nodes: &shot.legend,
    })?);
    Ok(content)
}

/// `value` as one text block of JSON.
fn json(value: &impl Serialize) -> Result<Vec<ContentBlock>, ToolError> {
    ContentBlock::json(value)
        .map(|block| vec![block])
        .map_err(|error| ToolError(format!("the answer did not serialize: {error}")))
}

/// Refuse a window no device can make or no one can read: a side of 0 or past
/// [`MAX_WINDOW_SIDE`], or a scale under [`MIN_SCALE`].
fn check_config(config: SessionConfig) -> Result<(), ToolError> {
    let size = config.size;
    if size.min_element() == 0 || size.max_element() > MAX_WINDOW_SIDE {
        return Err(ToolError(format!(
            "size is {}×{}; give each side from 1 to {MAX_WINDOW_SIDE} pixels",
            size.x, size.y
        )));
    }
    if !(config.scale.is_finite() && config.scale >= MIN_SCALE) {
        return Err(ToolError(format!(
            "scale is {}; give the physical pixels per logical pixel, {MIN_SCALE} or more",
            config.scale
        )));
    }
    Ok(())
}
