//! Headless frontend: no GUI, no terminal UI. Boots Session, worker,
//! and script transports, then waits for either a script `shutdown()`
//! or Ctrl-C. Useful for driving darkroom-egui from a remote script client
//! (`examples/script_client`) without a desktop window.
//!
//! Loop shape: wake → `session.tick(...)` (drains worker and script
//! inbounds, forwards intents to the worker) → sleep until the next
//! `request_redraw` or `close_app` from the host or until Ctrl-C.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::Notify;

use crate::headless::ui_host::HeadlessUiHost;
use crate::launch_config::LaunchConfig;
use crate::session::Session;
use crate::session::output::FrameOutput;

mod ui_host;

pub async fn run(launch_config: LaunchConfig) -> Result<()> {
    let wake = Arc::new(Notify::new());
    let shutdown = Arc::new(AtomicBool::new(false));
    let host = HeadlessUiHost::new(wake.clone(), shutdown.clone());
    let mut session = Session::new(host, launch_config);

    tracing::info!(
        "darkroom-egui headless: running. Send `shutdown()` over the script TCP, or Ctrl-C."
    );

    let mut output = FrameOutput::default();
    loop {
        // Drain inbound queues and dispatch any pending worker messages
        // (queued runs, autorun toggles, intents committed by scripts).
        // No render — there's no UI to paint.
        session.tick(&mut output);

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = wake.notified() => {}
        }
    }

    session.exit();
    tracing::info!("darkroom-egui headless: shut down");
    Ok(())
}
