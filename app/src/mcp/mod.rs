//! The editor as an MCP server over stdio, for an agent to open projects in, click through and
//! look at. `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

mod darkroom_server;
mod params;
mod session_thread;

use std::path::PathBuf;
use std::process::ExitCode;

use common::thread as threads;
use darkroom::driver::{DisplayKind, Driver, SessionConfig};
use glam::UVec2;
use rmcp::ServiceExt as _;
use rmcp::transport::stdio;
use tokio::runtime;
use tokio::sync::oneshot;

use crate::mcp::darkroom_server::DarkroomServer;
use crate::mcp::session_thread::{Ended, SessionThread};

/// The largest window side a session takes, in physical pixels: wgpu's default
/// `max_texture_dimension_2d`, which desktop adapters reach. palantir asks the adapter for its
/// own limit, so an adapter below it panics on a larger window.
const MAX_WINDOW_SIDE: u32 = 8192;
/// The smallest scale a session takes: at a quarter, a 12 px label is 3 pixels high.
const MIN_SCALE: f32 = 0.25;

/// Serve the editor in an offscreen window of `size` physical pixels at `scale`, on `document`
/// or a new one, until the client leaves or the editor ends. The process is the session:
/// `darkroom/AGENTS.md`, "The editor harness", "The process is the session".
pub(crate) fn serve_headless(document: Option<PathBuf>, size: UVec2, scale: f32) -> ExitCode {
    let config = SessionConfig {
        display: DisplayKind::Gpu,
        size,
        scale,
    };
    if let Err(message) = check_config(config) {
        tracing::error!("darkroom: {message}");
        return ExitCode::FAILURE;
    }
    let started = SessionThread::start(move || session_thread::launch(config, document.as_deref()));
    let (session, ended) = match started {
        Ok(started) => started,
        Err(error) => {
            tracing::error!("darkroom: {error}");
            return ExitCode::FAILURE;
        }
    };
    match serve(session, ended) {
        Ok(None | Some(Ended::Quit)) => ExitCode::SUCCESS,
        Ok(Some(Ended::Panic(_))) => ExitCode::FAILURE,
        Err(error) => {
            tracing::error!("darkroom: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Run the editor's window on `document` on this, the main thread, and serve it from another
/// until the window closes. The server ends when the client leaves or the session ends; the
/// window stays for the person at it, whose edits a quit from the server would discard.
pub(crate) fn serve_window(document: Option<PathBuf>) -> ExitCode {
    let log = darkroom::init_driven_tracing();
    let run = darkroom::run_driven(document, move |link| {
        // Detached: the process ends with the window, whatever the server is doing.
        threads::spawn("darkroom-mcp", move || {
            let started = SessionThread::start(move || Driver::window(link, log));
            let ended = started
                .map_err(|error| error.to_string())
                .and_then(|(session, ended)| serve(session, ended));
            match ended {
                Ok(None) => tracing::info!("darkroom: the MCP client left; the window stays"),
                Ok(Some(ended)) => tracing::info!("darkroom: the MCP session ended: {ended:?}"),
                Err(error) => tracing::error!("darkroom: the MCP server ended: {error}"),
            }
        });
    });
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("darkroom: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Serve `session` over stdio until the client leaves or the session ends, and say which ended
/// it: `None` for the client, else how the session ended.
fn serve(session: SessionThread, ended: oneshot::Receiver<Ended>) -> Result<Option<Ended>, String> {
    let runtime = runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime builds");
    runtime.block_on(async move {
        let service = DarkroomServer::new(session)
            .serve(stdio())
            .await
            .map_err(|error| error.to_string())?;
        // The editor's end ends the server; rmcp sends the answers in flight before it stops.
        let token = service.cancellation_token();
        let watch = tokio::spawn(async move {
            let ended = ended.await.ok();
            token.cancel();
            ended
        });
        service.waiting().await.map_err(|error| error.to_string())?;
        watch.abort();
        Ok(watch.await.ok().flatten())
    })
}

/// Refuse a window no device can make or no one can read: a side of 0 or past
/// [`MAX_WINDOW_SIDE`], or a scale under [`MIN_SCALE`].
fn check_config(config: SessionConfig) -> Result<(), String> {
    let size = config.size;
    if size.min_element() == 0 || size.max_element() > MAX_WINDOW_SIDE {
        return Err(format!(
            "size is {}×{}; give each side from 1 to {MAX_WINDOW_SIDE} pixels",
            size.x, size.y
        ));
    }
    if !(config.scale.is_finite() && config.scale >= MIN_SCALE) {
        return Err(format!(
            "scale is {}; give the physical pixels per logical pixel, {MIN_SCALE} or more",
            config.scale
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
