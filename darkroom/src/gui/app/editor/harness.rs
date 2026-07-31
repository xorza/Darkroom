//! Whole-editor test harness: drives [`Editor::frame`] through palantir's
//! [`UiHarness`], so a test can feed a real pointer event and assert on
//! what the editor did with it.
//!
//! Two levels, one type. [`EditorHarness::apply`] / [`EditorHarness::drain`]
//! reach the edit path directly and record nothing — enough for the tests
//! about what an intent does to a document. [`EditorHarness::frame`] drives a
//! real record pass, which is the only way to exercise what sits between a
//! pointer event and an intent: hit-testing, response routing, pane scoping.
//!
//! **The record closure runs once per record pass, not once per frame.**
//! `Editor::frame` is called from `App::record`, so on a frame with
//! pending action input it runs *twice*, exactly as in production. That
//! is deliberate — it is the behaviour under test. What a caller must
//! not do is accumulate across frames on the assumption of one call per
//! frame; [`Self::frame`] returns the first pass's command for the same
//! reason `UiHarness::frame_value` returns pass A.

use glam::UVec2;
use palantir::Ui;
use palantir::internals::UiHarness;
use scenarium::Library;

use crate::core::document::harness::DocFixture;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::types::GraphIntent;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::app::editor::Editor;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// Surface every editor test frames at unless it resizes. Wide enough
/// that the dock strip lays its chips out unwrapped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

#[derive(Debug)]
pub(crate) struct EditorHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read
    /// geometry through it directly — `h.ui.press_at(..)`, `h.ui.rect(..)`.
    pub(crate) ui: UiHarness,
    pub(crate) editor: Editor,
    pub(crate) open: OpenDocument,
    pub(crate) library: Library,
    pub(crate) theme: Theme,
    /// The run projections the frame reads — `App`'s in production, so a
    /// test that wants a node to look executed writes it here.
    pub(crate) run_state: RunState,
    pub(crate) preferences: Preferences,
    /// Footprint handed to the status bar. `0` — the default — is the
    /// no-reading path, so a test asserting on geometry isn't reading a
    /// figure that moves between runs; set it to pin the `MEM` clause.
    pub(crate) process_memory: u64,
}

impl EditorHarness {
    /// Real text shaping — the dock strip and node headers size to their
    /// labels, so mono metrics would put every chip in the wrong place.
    pub(crate) fn new(fixture: DocFixture) -> Self {
        Self {
            ui: UiHarness::with_text(SURFACE),
            editor: Editor::new(),
            open: OpenDocument {
                document: fixture.doc,
                path: None,
                dirty: false,
            },
            library: fixture.library,
            theme: Theme::default(),
            run_state: RunState::default(),
            preferences: Preferences::default(),
            process_memory: 0,
        }
    }

    /// Push one intent through the real edit path, as a widget's does.
    pub(crate) fn apply(&mut self, intent: GraphIntent) {
        self.editor.apply_edit(&mut self.open, intent);
    }

    /// Drain the queued intents into the document, as the frame's edit phase
    /// does.
    pub(crate) fn drain(&mut self) {
        self.editor.drain_intents(&mut self.open);
    }

    /// Take back the last undoable entry. Reports whether there was one.
    pub(crate) fn undo(&mut self) -> bool {
        self.editor
            .action_stack
            .undo(&mut self.open.document, &mut |_| {})
    }

    /// One editor frame. Returns the command the **first** record pass
    /// produced; a frame with pending action input records twice and the
    /// second pass no longer sees the one-frame edges that raise most
    /// commands.
    pub(crate) fn frame(&mut self) -> Option<AppCommand> {
        let Self {
            ui,
            editor,
            open,
            library,
            theme,
            run_state,
            preferences,
            process_memory,
        } = self;
        let ctx = AppCtx::new(
            theme,
            library,
            run_state,
            StatusInputs {
                error: None,
                process_memory: *process_memory,
            },
        );
        ui.frame_value(|recorder: &mut Ui| editor.frame(recorder, open, ctx, preferences))
    }

    /// `n` frames whose commands are discarded — the editor equivalent of
    /// `UiHarness::prime`. Two is the minimum before reading geometry:
    /// one to lay out, one for `response_for` to resolve against a
    /// settled cascade.
    pub(crate) fn prime(&mut self, n: u32) {
        for _ in 0..n {
            let _ = self.frame();
        }
    }
}

#[cfg(test)]
mod tests {

    use super::EditorHarness;

    use crate::core::document::harness::DocFixture;

    use crate::gui::window::status_bar::status_bar_id;

    /// The bar used to collapse when it had nothing to say; the process
    /// footprint gives it something on every frame, so it is recorded on
    /// an untouched document — and stays recorded when no reading is
    /// available, rather than reappearing as the figure lands.
    #[test]
    fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
        let mut h = EditorHarness::new(DocFixture::default());
        h.prime(2);
        let without =
            h.ui.rect(status_bar_id())
                .expect("status bar records with no reading and an empty cache");

        h.process_memory = 3 * 1024 * 1024;
        h.prime(2);
        let with = h.ui.rect(status_bar_id()).expect("status bar records");

        // The strip is a real row either way — a collapsed one would
        // arrange to zero height and read as "no bar".
        for (rect, what) in [(without, "no reading"), (with, "3 MB reading")] {
            assert!(rect.size.h > 0.0, "{what}: bar arranged to zero height");
            assert!(rect.size.w > 0.0, "{what}: bar arranged to zero width");
        }
        // With a reading the bar hugs a line of text; without one it is
        // padding alone, so it is strictly shorter. Both are still rows.
        assert!(
            with.size.h > without.size.h,
            "a reading adds its label's line to the bar: {} vs {}",
            with.size.h,
            without.size.h,
        );
    }
}
