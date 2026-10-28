//! [`DarkroomServer`]: the editor's tools, each a job on the editor's thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use darkroom::driver::Driver;
use darkroom::driver::action::Action;
use darkroom::driver::error::{ActFailure, DriverError};
use darkroom::driver::shot::{LegendNode, Region};
use darkroom::driver::state::{Detail, NODE_LIMIT, State};
use glam::UVec2;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, PromptMessage, Role, ServerCapabilities,
    ServerConfig,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::mcp::params::{
    ActParams, MAX_SIDE, OpenParams, RUN_TIMEOUT_S, ShotParams, StateParams, UI_LIMIT, UiParams,
    WaitRunParams,
};
use crate::mcp::session_thread::{Session, SessionThread, ToolError};

/// What the server tells an agent before its first call.
const INSTRUCTIONS: &str = "Drives the darkroom node editor, offscreen or in its window. Open a \
project with darkroom_open, play clicks, drags and keys with darkroom_act, read the editor as text with \
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
    size: UVec2,
    nodes: &'a [LegendNode],
}

#[tool_router]
impl DarkroomServer {
    /// A server of the session `session` holds.
    pub(crate) fn new(session: Arc<SessionThread>) -> Self {
        Self {
            session,
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
        description = "Open a .darkroom or .darkroomz project in the editor, in place of the open document; \
        with no project, a new unsaved document, which a Save As names. A project that does not \
        open leaves the open document as it was. The project opens where \
        it is, so saves reach the original; copy opens a copy of its folder in the session's own \
        directory in RAM instead, so nothing the session does reaches the original. \
        Answers with the build the server runs, the session directory, and the editor's state. \
        Example: {\"project\": \"darkroom/sample/sample.darkroom\"}"
    )]
    async fn darkroom_open(&self, Parameters(params): Parameters<OpenParams>) -> CallToolResult {
        self.run(move |session| {
            let driver = session.open(params.project.as_deref(), params.mode())?;
            json(&Opened {
                build: env!("DARKROOM_BUILD"),
                session_dir: driver.session_dir().to_path_buf(),
                state: driver.state(Detail::Concise, None)?,
            })
        })
        .await
    }

    #[tool(
        description = "Play input into the editor: clicks, double clicks, drags, scrolls, \
        pinches, key chords, typed text, and the answers file dialogs take. The editor settles \
        after each action; the first that fails stops the rest, and one that quits the editor \
        ends the session and the server. Name each place with a locator: \
        node, header, port, text, world, screen, offset or nth. Answers with the selection, the \
        dirty flag and the focus, and with a shot when one is asked. Example: {\"actions\": \
        [{\"click\": {\"at\": {\"node\": \"Blur\"}}}, {\"drag\": {\"from\": {\"port\": {\"node\": \
        \"Load Image\", \"side\": \"out\", \"port\": \"Image\"}}, \"to\": {\"port\": {\"node\": \
        \"Blur\", \"side\": \"in\", \"port\": \"Image\"}}}}, {\"key\": \"ctrl+s\"}], \"shot\": \
        {\"region\": {\"node\": \"Blur\"}}}"
    )]
    async fn darkroom_act(&self, Parameters(params): Parameters<ActParams>) -> CallToolResult {
        self.run(move |session| {
            let actions = params
                .actions
                .into_iter()
                .map(Action::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            let count = actions.len();
            let report = match session.driver()?.act(&actions) {
                Ok(report) => report,
                Err(ActFailure {
                    ran,
                    error: DriverError::Quit,
                }) => {
                    session.quit();
                    return Ok(vec![ContentBlock::text(format!(
                        "The editor quit at action {} of {count}, as the action asked, and the \
                         session is closed; the actions after it did not run, and the server \
                         ends.",
                        (ran + 1).min(count)
                    ))]);
                }
                Err(ActFailure { ran, error }) if ran < count => {
                    return Err(ToolError(format!(
                        "action {} of {count} failed, and the ones before it ran: {error}",
                        ran + 1
                    )));
                }
                Err(ActFailure { error, .. }) => {
                    return Err(ToolError(format!(
                        "the {count} actions ran, and reading the editor after them failed: \
                         {error}"
                    )));
                }
            };
            let mut content = json(&report)?;
            if let Some(shot) = params.shot {
                // The actions ran whatever the shot does: their report stays, so the agent does
                // not play them again.
                match take_shot(session.driver()?, shot) {
                    Ok(shot) => content.extend(shot),
                    Err(ToolError(message)) => content.push(ContentBlock::text(format!(
                        "The actions ran; the shot after them failed: {message}"
                    ))),
                }
            }
            Ok(content)
        })
        .await
    }

    #[tool(
        description = format!(
            "The editor as text: the open document, whether it has unsaved edits, each node by \
             name with its rect on screen, the selection, the focus, the status line, and the \
             warnings and errors logged since the last call. detailed adds each node's id, \
             ports, bindings and run status, the wires, the camera and whether an undo is \
             possible. nodes keeps the nodes whose name contains it and the wires with one at \
             either end; the list stops at {NODE_LIMIT} nodes, and the answer says how many it \
             left out. Example: {{\"detail\": \"detailed\", \"nodes\": \"Blur\"}}"
        )
    )]
    async fn darkroom_state(&self, Parameters(params): Parameters<StateParams>) -> CallToolResult {
        self.run(move |session| {
            let driver = session.driver()?;
            json(&driver.state(params.detail.into(), params.nodes.as_deref())?)
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
            let driver = session.driver()?;
            json(&driver.ui(params.filter.as_deref(), params.limit.unwrap_or(UI_LIMIT))?)
        })
        .await
    }

    #[tool(
        description = "A picture of the editor as a user's screenshot gives it: the window, a \
        pane by its tab's name, a node by name with a margin, or a rect of the canvas. It is \
        scaled so that its longer side is at most max_side pixels, 1280 by default, and comes with \
        a legend of each visible node and port at its rect in the picture's pixels, so a point \
        seen in it becomes a locator. Read darkroom_state first: text \
        costs fewer tokens. Example: {\"region\": {\"node\": \"Blur\"}, \"max_side\": 800}"
    )]
    async fn darkroom_shot(&self, Parameters(params): Parameters<ShotParams>) -> CallToolResult {
        self.run(move |session| take_shot(session.driver()?, params))
            .await
    }

    #[tool(
        description = "Wait until a run the last darkroom_act started on the editor's worker \
        finishes and the editor settles, for at most timeout_s seconds, 60 by default. Start the \
        run in that darkroom_act, as with the toolbar's run button, a node's run chip or ctrl+r. \
        Example: {\"timeout_s\": 120}"
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
            let driver = session.driver()?;
            driver.wait_run(timeout)?;
            json(&driver.state(Detail::Concise, None)?)
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
fn take_shot(driver: &mut Driver, params: ShotParams) -> Result<Vec<ContentBlock>, ToolError> {
    let max_side = params.max_side.unwrap_or(MAX_SIDE);
    if max_side == 0 {
        return Err(ToolError(
            "max_side is 0; give the picture's longer side in pixels".to_string(),
        ));
    }
    let shot = driver.shot(&Region::from(params.region), max_side)?;
    // rmcp's own base64 encoder, which its prompt messages reach.
    let image = PromptMessage::new_image(Role::Assistant, &shot.png, "image/png", None, None);
    let mut content = vec![image.content];
    content.extend(json(&ShotLegend {
        path: &shot.path,
        size: shot.size,
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
