use crate::intent::Intent;

/// Per-frame outbox: every mutation a widget proposed during the
/// just-recorded frame. `App::frame` clears it, lets `view::build`
/// populate it via `push`, then drains and applies each entry through
/// `intent::build_step` / `intent::apply_step` (pushing the resulting
/// `UndoStep` onto the action stack).
///
/// Renderer-side code emits `Intent`s; this struct lets the App
/// orchestrate apply + undo-stack push from one place, instead of
/// scattering `ViewGraph` mutations across widget bodies.
#[derive(Default)]
pub struct FrameResult {
    pub intents: Vec<Intent>,
}

impl FrameResult {
    pub fn clear(&mut self) {
        self.intents.clear();
    }

    pub fn push(&mut self, intent: Intent) {
        self.intents.push(intent);
    }

    pub fn drain(&mut self) -> std::vec::Drain<'_, Intent> {
        self.intents.drain(..)
    }
}
