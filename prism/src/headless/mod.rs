//! Headless frontend: no GUI, no terminal UI. Boots Session, worker,
//! and script transports, then waits for either a script `shutdown()`
//! or Ctrl-C. Useful for driving prism from a remote script client
//! (`examples/script_client`) without a desktop window.
//!
//! Loop shape: wake → `session.frame(...)` (drains worker and script
//! inbounds, forwards intents to the worker) → sleep until the next
//! `request_redraw` or `close_app` from the host or until Ctrl-C.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::Notify;

use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::headless::ui_host::HeadlessUiHost;
use crate::launch_config::LaunchConfig;
use crate::session::Session;

mod ui_host;

pub async fn run(launch_config: LaunchConfig) -> Result<()> {
    let wake = Arc::new(Notify::new());
    let shutdown = Arc::new(AtomicBool::new(false));
    let host = HeadlessUiHost::new(wake.clone(), shutdown.clone());
    let mut session = Session::new(host, launch_config);

    println!("prism headless: running. Send `shutdown()` over the script TCP, or Ctrl-C.");

    let mut output = FrameOutput::default();
    loop {
        // Drain inbound queues and dispatch any pending worker messages
        // (queued runs, autorun toggles, intents committed by scripts).
        // No render closure — there's no UI to paint.
        session.frame(&mut output, |_, _| {});

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = wake.notified() => {}
        }
    }

    session.exit();
    println!("prism headless: shut down");
    Ok(())
}
