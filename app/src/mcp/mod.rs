//! The editor as an MCP server over stdio, for an agent to open projects in, click through and
//! look at. `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

mod darkroom_server;
mod params;
mod session_thread;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};

use common::thread as threads;
use darkroom::driver::log_ring::LogRing;
use darkroom::driver::{DisplayKind, Driver, SessionConfig};
use glam::UVec2;
use rmcp::ServiceExt as _;
use rmcp::transport::stdio;
use tokio::runtime;
use tokio::sync::oneshot;

use crate::mcp::darkroom_server::DarkroomServer;
use crate::mcp::session_thread::{Ended, SessionThread};

/// Serve the editor in an offscreen window of `size` physical pixels at `scale`, on `document`
/// or a new one, until the client leaves or the editor ends, its logs kept in `log`. The process
/// is the session: `darkroom/AGENTS.md`, "The editor harness", "The process is the session".
pub(crate) fn serve_headless(
    document: Option<PathBuf>,
    size: UVec2,
    scale: f32,
    log: LogRing,
) -> ExitCode {
    let config = SessionConfig {
        display: DisplayKind::Gpu,
        size,
        scale,
    };
    let started =
        SessionThread::start(move || session_thread::launch(config, log, document.as_deref()));
    let (session, ended) = match started {
        Ok(started) => started,
        Err(error) => {
            tracing::error!("darkroom: {error}");
            return ExitCode::FAILURE;
        }
    };
    match serve(Arc::new(session), ended) {
        Ok(None | Some(Ended::Quit | Ended::Closed)) => ExitCode::SUCCESS,
        Ok(Some(Ended::Panic(_))) => ExitCode::FAILURE,
        Err(error) => {
            tracing::error!("darkroom: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Run the editor's window on `document` on this, the main thread, and serve it from another
/// until the window closes, its logs kept in `log`. The server ends when the client leaves or the
/// session ends; the window stays for the person at it, whose edits a quit from the server would
/// discard.
pub(crate) fn serve_window(document: Option<PathBuf>, log: LogRing) -> ExitCode {
    let (sessions, session) = mpsc::channel();
    let run = darkroom::run_driven(document, move |link| {
        // Detached: the server reads stdin, which nothing interrupts, so the process ends with
        // the window whatever the server is doing. The session comes back here to end first.
        threads::spawn("darkroom-mcp", move || {
            let ended = SessionThread::start(move || Driver::window(link, log))
                .map_err(|error| error.to_string())
                .and_then(|(session, ended)| {
                    let session = Arc::new(session);
                    sessions.send(Arc::clone(&session)).ok();
                    serve(session, ended)
                });
            match ended {
                Ok(None) => tracing::info!("darkroom: the MCP client left; the window stays"),
                Ok(Some(ended)) => tracing::info!("darkroom: the MCP session ended: {ended:?}"),
                Err(error) => tracing::error!("darkroom: the MCP server ended: {error}"),
            }
        });
    });
    // The window is gone. A session that started ends before the process, so its directory goes
    // with it; one that did not start dropped the sender.
    if let Ok(session) = session.recv() {
        session.close();
    }
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
fn serve(
    session: Arc<SessionThread>,
    ended: oneshot::Receiver<Ended>,
) -> Result<Option<Ended>, String> {
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

#[cfg(test)]
mod tests;
