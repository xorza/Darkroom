//! Headless mode: no UI at all. Drives a [`TerminalSession`] (worker + script TCP
//! host), idling until a script `shutdown()` or Ctrl-C.
//!
//! Loop shape: `tick` (drain worker + script inbounds, apply edits, run)
//! → break on `shutdown()` → otherwise sleep until the next wake (a script
//! side-effect) or Ctrl-C. The `TerminalSession` is owned by `main` (built +
//! dropped in sync context, since dropping the worker/script tokio
//! runtimes inside this async loop would panic); here we only borrow it.

use anyhow::Result;
use tokio::sync::Notify;

use crate::core::terminal_session::TerminalSession;

pub(crate) async fn run(session: &mut TerminalSession, notify: &Notify) -> Result<()> {
    tracing::info!("darkroom headless: running — send `shutdown()` over the script TCP, or Ctrl-C");

    loop {
        session.tick();
        if session.quit {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            // Woken by a script side-effect; loop back to drain it.
            _ = notify.notified() => {}
        }
    }

    tracing::info!("darkroom headless: shut down");
    Ok(())
}
