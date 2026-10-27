use std::path::PathBuf;
use std::sync::Arc;

use common::{TempDir, fs, task};
use darkroom::driver::log_ring::LogRing;
use darkroom::driver::{DisplayKind, SessionConfig};
use darkroom::internals::fixture;
use glam::UVec2;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::serde_json::{self, Value, json};
use rmcp::service::{RoleClient, RunningService};
use rmcp::{ServiceError, ServiceExt as _};
use tokio::io;
use tokio::sync::oneshot;

use crate::mcp::darkroom_server::DarkroomServer;
use crate::mcp::session_thread::{self, Ended, SessionThread, ToolError};

const BRIGHTNESS: &str = "Brightness / Contrast";

/// A connected server and client, the session it serves, and the receiver told when the session
/// ends.
struct Connected {
    server: DarkroomServer,
    session: Arc<SessionThread>,
    client: RunningService<RoleClient, ()>,
    ended: oneshot::Receiver<Ended>,
}

/// A server on `display` and `document`, and a client talking to it in process.
async fn connect(display: DisplayKind, document: Option<PathBuf>) -> Connected {
    let config = SessionConfig {
        display,
        size: UVec2::new(800, 600),
        scale: 1.0,
    };
    let (session, ended) = SessionThread::start(move || {
        session_thread::launch(config, LogRing::default(), document.as_deref())
    })
    .unwrap();
    let session = Arc::new(session);
    let server = DarkroomServer::new(Arc::clone(&session));
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
    Connected {
        server,
        session,
        client,
        ended,
    }
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
    let Connected { client, .. } = connect(DisplayKind::Cpu, None).await;
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
    assert_eq!(properties("darkroom_open"), ["copy", "project"]);
    assert_eq!(properties("darkroom_act"), ["actions", "shot"]);
    assert_eq!(properties("darkroom_state"), ["detail", "nodes"]);
    assert_eq!(properties("darkroom_ui"), ["filter", "limit"]);
    assert_eq!(properties("darkroom_shot"), ["max_side", "region"]);
    assert_eq!(properties("darkroom_wait_run"), ["timeout_s"]);
}

/// A server started on no document is on a new one. An opened fixture reports the build and its
/// nodes, and `darkroom_state` lists them; a click on a name two nodes share is a tool error
/// that names the action it stopped at and lists both nodes, and `nth` picks one. A bad chord
/// refuses its batch before any of it plays. The CPU display has no shots, and an act whose shot
/// fails still reports the actions it played. An open with no project is a new unsaved document; a project that does
/// not open leaves the open document as it was.
#[tokio::test]
async fn an_opened_project_is_driven_through_the_tools() {
    let Connected { client, .. } = connect(DisplayKind::Cpu, None).await;
    let early = call(&client, "darkroom_state", json!({})).await.unwrap();
    let early: Value = serde_json::from_str(text(&early, 0)).unwrap();
    assert_eq!(
        (&early["document"], &early["nodes"]),
        (&Value::Null, &json!([])),
        "the session starts on a new document"
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

    // A bad chord refuses the batch before any of it plays: the click on the empty corner,
    // which would clear the selection, does not run.
    let refused = call(
        &client,
        "darkroom_act",
        json!({ "actions": [{ "click": { "at": { "screen": [1.0, 1.0] } } }, { "key": "ctrl+banana" }] }),
    )
    .await
    .unwrap();
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused, 0).starts_with("'ctrl+banana' is not a key chord"),
        "{}",
        text(&refused, 0)
    );
    let kept = call(&client, "darkroom_state", json!({})).await.unwrap();
    let kept: Value = serde_json::from_str(text(&kept, 0)).unwrap();
    assert_eq!(kept["selection"], json!([BRIGHTNESS]));

    let shot = call(&client, "darkroom_shot", json!({})).await.unwrap();
    assert_eq!(shot.is_error, Some(true));
    assert!(text(&shot, 0).contains("GPU display"), "{}", text(&shot, 0));
    // A shot that fails after the actions keeps their report: they ran.
    let acted = call(
        &client,
        "darkroom_act",
        json!({ "actions": [{ "move": { "screen": [10.0, 10.0] } }], "shot": {} }),
    )
    .await
    .unwrap();
    assert_eq!(acted.is_error, Some(false), "{}", text(&acted, 0));
    let report: Value = serde_json::from_str(text(&acted, 0)).unwrap();
    assert_eq!(report["selection"], json!([BRIGHTNESS]));
    assert_eq!(
        text(&acted, 1),
        format!(
            "The actions ran; the shot after them failed: {}",
            text(&shot, 0)
        )
    );

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
    assert!(
        text(&missing, 0).starts_with("the editor did not open "),
        "{}",
        text(&missing, 0)
    );
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    let after: Value = serde_json::from_str(text(&after, 0)).unwrap();
    assert_eq!(
        after["document"],
        Value::Null,
        "a project that does not open leaves the new document open"
    );
}

/// A panic on the editor's thread answers its call with the message and where it was raised,
/// and ends the session, which the server is told of; a call after it says how the session
/// ended. A quit, which an action asks, ends the session too, and says so in place of the
/// actions after it. A close, which the window's end asks, ends the session and removes its
/// directory before it returns, and one after a quit keeps the quit. A server starts on the
/// document it is given.
#[tokio::test]
async fn a_panic_or_a_quit_ends_the_session() {
    let dir = TempDir::new("darkroom-mcp-panic");
    let project = fixture::project(dir.path(), &[BRIGHTNESS]);
    let Connected {
        server,
        client,
        ended,
        ..
    } = connect(DisplayKind::Cpu, Some(project.clone())).await;
    let started = call(&client, "darkroom_state", json!({})).await.unwrap();
    let started: Value = serde_json::from_str(text(&started, 0)).unwrap();
    assert_eq!(started["document"], project.to_str().unwrap());

    let line = line!() + 1;
    let panicked = server.run(|_| -> Result<Vec<ContentBlock>, ToolError> { panic!("boom") });
    let panicked = panicked.await;
    assert_eq!(panicked.is_error, Some(true));
    let message = text(&panicked, 0);
    assert!(
        message.contains(&format!("boom at {}:{line}", file!())),
        "{message}"
    );
    assert!(
        matches!(ended.await, Ok(Ended::Panic(panic)) if panic.starts_with("boom at ")),
        "the server is told"
    );
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert!(
        text(&after, 0).starts_with("the session ended when the editor panicked: boom"),
        "{}",
        text(&after, 0)
    );

    let Connected {
        session,
        client,
        ended,
        ..
    } = connect(DisplayKind::Cpu, Some(project)).await;
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
    assert!(matches!(ended.await, Ok(Ended::Quit)));
    // A close after the quit, as the window's end sends, keeps the reason it ended for.
    task::run_blocking(move || session.close()).await.unwrap();
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(
        text(&after, 0),
        "the session ended when an action quit the editor; the server is ending"
    );

    let Connected {
        server,
        session,
        client,
        ended,
    } = connect(DisplayKind::Cpu, None).await;
    let session_dir = server
        .run(|session| {
            let dir = session.driver()?.session_dir().display().to_string();
            Ok(vec![ContentBlock::text(dir)])
        })
        .await;
    let session_dir = PathBuf::from(text(&session_dir, 0));
    assert!(fs::is_dir(&session_dir));
    task::run_blocking(move || session.close()).await.unwrap();
    assert!(
        !fs::exists(&session_dir),
        "the close waits for the driver's drop"
    );
    assert!(matches!(ended.await, Ok(Ended::Closed)));
    let after = call(&client, "darkroom_state", json!({})).await.unwrap();
    assert_eq!(
        text(&after, 0),
        "the session ended when its window closed; the server is ending"
    );
}

/// On the GPU display a shot comes back as a PNG image block and its legend, which names the
/// node; an action can end in one.
#[tokio::test]
async fn a_shot_is_an_image_and_its_legend() {
    let dir = TempDir::new("darkroom-mcp-shot");
    let project = fixture::project(dir.path(), &[BRIGHTNESS]);
    let Connected { client, .. } = connect(DisplayKind::Gpu, Some(project)).await;
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

/// A document that does not open keeps the session from starting.
#[test]
fn a_document_that_does_not_open_is_refused() {
    let config = SessionConfig {
        display: DisplayKind::Cpu,
        size: UVec2::new(800, 600),
        scale: 1.0,
    };
    let dir = TempDir::new("darkroom-mcp-start");
    let missing = dir.join("missing.darkroom");
    let refused = SessionThread::start(move || {
        session_thread::launch(config, LogRing::default(), Some(&missing))
    })
    .unwrap_err();
    assert!(
        refused.to_string().starts_with("the editor did not open "),
        "{refused}"
    );
}
