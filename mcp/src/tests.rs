use common::TempDir;
use darkroom::internals::harness::fixture;
use darkroom::internals::harness::{DisplayKind, SessionConfig};
use glam::UVec2;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::serde_json::{self, Value, json};
use rmcp::service::{RoleClient, RunningService};
use rmcp::{ServiceError, ServiceExt as _};
use tokio::io;

use crate::WindowSize;
use crate::darkroom_server::DarkroomServer;
use crate::session_thread::ToolError;

const BRIGHTNESS: &str = "Brightness / Contrast";

/// A server on `display`, and a client talking to it in process.
async fn connect(display: DisplayKind) -> (DarkroomServer, RunningService<RoleClient, ()>) {
    let server = DarkroomServer::new(SessionConfig {
        display,
        size: UVec2::new(800, 600),
        scale: 1.0,
    });
    let (server_end, client_end) = io::duplex(1 << 16);
    let serving = server.clone();
    tokio::spawn(async move {
        serving
            .serve(server_end)
            .await
            .expect("the server starts")
            .waiting()
            .await
            .ok();
    });
    let client = ().serve(client_end).await.expect("the client connects");
    (server, client)
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> Result<CallToolResult, ServiceError> {
    let Value::Object(arguments) = arguments else {
        panic!("a tool's arguments are an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
}

/// The text of `result`'s block at `idx`.
fn text(result: &CallToolResult, idx: usize) -> &str {
    &result.content[idx].as_text().expect("a text block").text
}

/// The six tools are listed, each with the schema of its input: an object whose properties are
/// the fields an agent writes.
#[tokio::test]
async fn the_server_lists_its_six_tools_with_their_schemas() {
    let (_server, client) = connect(DisplayKind::Cpu).await;
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "darkroom_act",
            "darkroom_open",
            "darkroom_shot",
            "darkroom_state",
            "darkroom_ui",
            "darkroom_wait_run"
        ]
    );
    let properties = |name: &str| -> Vec<String> {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(
            tool.description
                .as_ref()
                .is_some_and(|text| text.contains("Example:"))
        );
        let mut keys: Vec<String> = tool.input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(
        properties("darkroom_open"),
        ["in_place", "project", "scale", "size"]
    );
    assert_eq!(properties("darkroom_act"), ["actions", "shot"]);
    assert_eq!(properties("darkroom_state"), ["detail", "nodes"]);
    assert_eq!(properties("darkroom_ui"), ["filter", "limit"]);
    assert_eq!(properties("darkroom_shot"), ["max_side", "region"]);
    assert_eq!(properties("darkroom_wait_run"), ["timeout_s"]);
}

/// A call before any open says to open first. An opened fixture reports the build and its
/// nodes, and `darkroom_state` lists them; a click on a name two nodes share is a tool error
/// that names the action it stopped at and lists both nodes, and `nth` picks one. The CPU
/// display has no shots, and a window of no pixels is refused before a session starts. An open
/// with no project is a new unsaved document; a project that does not open closes the session.
#[tokio::test]
async fn an_opened_project_is_driven_through_the_tools() {
    let (_server, client) = connect(DisplayKind::Cpu).await;
    let early = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(early.is_error, Some(true));
    assert_eq!(
        text(&early, 0),
        "no project is open; call darkroom_open first"
    );

    let dir = TempDir::new("darkroom-mcp-open");
    let project = fixture::project(dir.path(), &[BRIGHTNESS, BRIGHTNESS]);
    let opened = call(&client, "darkroom_open", json!({ "project": project }))
        .await
        .unwrap();
    assert_eq!(opened.is_error, Some(false), "{}", text(&opened, 0));
    let opened: Value = serde_json::from_str(text(&opened, 0)).unwrap();
    assert_eq!(opened["build"], env!("DARKROOM_BUILD"));
    assert_eq!(opened["state"]["nodes"].as_array().unwrap().len(), 2);

    let state = call(&client, "darkroom_state", json!({ "detail": "detailed" }))
        .await
        .unwrap();
    let state: Value = serde_json::from_str(text(&state, 0)).unwrap();
    let names: Vec<&str> = state["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, [BRIGHTNESS, BRIGHTNESS]);
    assert!(state["details"]["wires"].as_array().unwrap().is_empty());

    let click = |at: Value| json!({ "actions": [{ "click": { "at": at } }] });
    let ambiguous = call(
        &client,
        "darkroom_act",
        json!({ "actions": [
            { "move": { "screen": [10.0, 10.0] } },
            { "click": { "at": { "node": BRIGHTNESS } } },
        ] }),
    )
    .await
    .unwrap();
    assert_eq!(ambiguous.is_error, Some(true));
    let message = text(&ambiguous, 0);
    assert!(
        message.starts_with("action 2 of 2 failed, and the ones before it ran: "),
        "{message}"
    );
    for node in state["nodes"].as_array().unwrap() {
        let id = node["id"].as_str().unwrap();
        assert!(
            message.contains(&format!("{BRIGHTNESS} ({id})")),
            "{message}"
        );
    }
    let picked = call(
        &client,
        "darkroom_act",
        click(json!({ "nth": { "of": { "node": BRIGHTNESS }, "index": 1 } })),
    )
    .await
    .unwrap();
    let report: Value = serde_json::from_str(text(&picked, 0)).unwrap();
    assert_eq!(report["selection"], json!([BRIGHTNESS]));

    let shot = call(&client, "darkroom_shot", json!({})).await.unwrap();
    assert_eq!(shot.is_error, Some(true));
    assert!(text(&shot, 0).contains("GPU display"), "{}", text(&shot, 0));

    let empty = call(
        &client,
        "darkroom_open",
        json!({ "project": project, "size": [0, 600] }),
    )
    .await
    .unwrap();
    assert_eq!(
        text(&empty, 0),
        "size is 0×600; give each side from 1 to 8192 pixels"
    );
    let still = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(still.is_error, Some(false), "the open session stays");

    let new = call(&client, "darkroom_open", json!({})).await.unwrap();
    let new: Value = serde_json::from_str(text(&new, 0)).unwrap();
    assert_eq!(
        (&new["state"]["document"], &new["state"]["nodes"]),
        (&Value::Null, &json!([])),
        "no project is a new unsaved document"
    );

    let missing = call(
        &client,
        "darkroom_open",
        json!({ "project": dir.path().join("missing").join("scene.darkroom") }),
    )
    .await
    .unwrap();
    assert_eq!(missing.is_error, Some(true));
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(
        text(&after, 0),
        "no project is open; call darkroom_open first",
        "a project that does not open leaves no session"
    );
}

/// A panic on the editor's thread answers its call with the message and where it was raised,
/// closes the session, and leaves the server serving: the next call says the session ended and
/// how, and an open starts a new one. A quit, which an action asks, closes the session too, and
/// says so in place of the actions after it.
#[tokio::test]
async fn a_panic_ends_the_session_and_not_the_server() {
    let (server, client) = connect(DisplayKind::Cpu).await;
    let dir = TempDir::new("darkroom-mcp-panic");
    let project = fixture::project(dir.path(), &[BRIGHTNESS]);
    call(&client, "darkroom_open", json!({ "project": project }))
        .await
        .unwrap();

    let line = line!() + 1;
    let panicked = server.run(|_| -> Result<Vec<ContentBlock>, ToolError> { panic!("boom") });
    let panicked = panicked.await;
    assert_eq!(panicked.is_error, Some(true));
    let message = text(&panicked, 0);
    assert!(
        message.contains(&format!("boom at mcp/src/tests.rs:{line}")),
        "{message}"
    );

    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert!(
        text(&after, 0).starts_with(
            "no project is open: the last session ended when the editor panicked: boom"
        ),
        "{}",
        text(&after, 0)
    );
    let reopened = call(&client, "darkroom_open", json!({ "project": project }))
        .await
        .unwrap();
    assert_eq!(reopened.is_error, Some(false), "{}", text(&reopened, 0));

    let quit = call(
        &client,
        "darkroom_act",
        json!({ "actions": [{ "key": "ctrl+q" }, { "key": "escape" }] }),
    )
    .await
    .unwrap();
    assert_eq!(quit.is_error, Some(false));
    assert!(
        text(&quit, 0).starts_with("The editor quit at action 1 of 2"),
        "{}",
        text(&quit, 0)
    );
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(
        text(&after, 0),
        "no project is open: the last session ended when an action quit the editor. Call \
         darkroom_open to start a new one."
    );
}

/// On the GPU display a shot comes back as a PNG image block and its legend, which names the
/// node; an action can end in one.
#[tokio::test]
async fn a_shot_is_an_image_and_its_legend() {
    let (_server, client) = connect(DisplayKind::Gpu).await;
    let dir = TempDir::new("darkroom-mcp-shot");
    let project = fixture::project(dir.path(), &[BRIGHTNESS]);
    call(&client, "darkroom_open", json!({ "project": project }))
        .await
        .unwrap();
    let acted = call(
        &client,
        "darkroom_act",
        json!({
            "actions": [{ "click": { "at": { "node": BRIGHTNESS } } }],
            "shot": { "region": { "node": BRIGHTNESS }, "max_side": 200 },
        }),
    )
    .await
    .unwrap();
    assert_eq!(acted.is_error, Some(false), "{}", text(&acted, 0));
    let ContentBlock::Image(image) = &acted.content[1] else {
        panic!("{:?}", acted.content[1]);
    };
    assert_eq!(image.mime_type, "image/png");
    let legend: Value = serde_json::from_str(text(&acted, 2)).unwrap();
    assert_eq!(legend["nodes"][0]["name"], BRIGHTNESS);
    let (w, h) = (legend["size"][0].as_u64(), legend["size"][1].as_u64());
    assert_eq!(
        w.max(h),
        Some(200),
        "the node with its margin is wider than 200 px, so its longer side scales to 200"
    );
}

/// A size is `WxH` in whole pixels; anything else is refused with the form to write.
#[test]
fn a_window_size_is_width_x_height() {
    assert_eq!(
        "1600x1000".parse::<WindowSize>().unwrap().0,
        UVec2::new(1600, 1000)
    );
    for bad in ["1600", "1600x", "x1000", "16o0x1000", "-1x1"] {
        assert_eq!(
            bad.parse::<WindowSize>().unwrap_err(),
            format!("'{bad}' is not a size; write it as 1600x1000")
        );
    }
}
