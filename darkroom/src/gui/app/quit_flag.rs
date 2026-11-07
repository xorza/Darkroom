//! [`QuitFlag`]: whether the editor quit, for a driver that outlives its window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Raised once, when the editor quits through File ▸ Quit or the shortcut, which closes its
/// window as the pass ends; a close by the title bar leaves it lowered. Shared, so a driver on
/// another thread tells a quit from a closed window after the app is gone. As Electron keeps a
/// quit (`before-quit`, `will-quit`) apart from a window's `close`
/// ([Electron `app`](https://www.electronjs.org/docs/latest/api/app#event-before-quit)).
#[derive(Clone, Debug, Default)]
pub(crate) struct QuitFlag(Arc<AtomicBool>);

impl QuitFlag {
    pub(crate) fn is_raised(&self) -> bool {
        // Relaxed: the flag publishes no other data, and a reader acts on its value alone.
        self.0.load(Ordering::Relaxed)
    }

    pub(super) fn raise(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
