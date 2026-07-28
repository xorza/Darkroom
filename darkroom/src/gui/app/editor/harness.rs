//! Whole-editor test harness: drives [`Editor::frame`] through palantir's
//! [`UiHarness`], so a test can feed a real pointer event and assert on
//! what the editor did with it.
//!
//! This is the level above `TestEditor`, which calls `apply_edit` /
//! `drain_intents` directly and never records. Everything between a
//! pointer event and an intent — hit-testing, response routing, pane
//! scoping — is only exercised by driving frames, and that is what this
//! type is for.
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

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::preferences::Preferences;
use crate::gui::app::StatusInputs;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::editor::Editor;
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
    pub(crate) preferences: Preferences,
    /// Footprint handed to the status bar. `0` — the default — is the
    /// no-reading path, so a test asserting on geometry isn't reading a
    /// figure that moves between runs; set it to pin the `MEM` clause.
    pub(crate) process_memory: u64,
}

impl EditorHarness {
    /// Real text shaping — the dock strip and node headers size to their
    /// labels, so mono metrics would put every chip in the wrong place.
    pub(crate) fn new(document: Document) -> Self {
        Self {
            ui: UiHarness::with_text(SURFACE),
            editor: Editor::new(),
            open: OpenDocument {
                document,
                path: None,
            },
            library: Library::default(),
            theme: Theme::default(),
            preferences: Preferences::default(),
            process_memory: 0,
        }
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
            preferences,
            process_memory,
        } = self;
        let theme = &*theme;
        let process_memory = *process_memory;
        ui.frame_value(|recorder: &mut Ui| {
            editor.frame(
                open,
                recorder,
                library,
                theme,
                preferences,
                StatusInputs {
                    error: None,
                    process_memory,
                },
            )
        })
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

    pub(crate) fn document(&self) -> &Document {
        &self.open.document
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::PointerButton;

    use super::EditorHarness;
    use crate::core::document::dock::{DockDrop, DockOp, SplitSide};
    use crate::core::document::{Document, GraphRef, TabRef};
    use crate::gui::canvas::outer_canvas_widget_id;
    use crate::gui::dock::strip;
    use crate::gui::status_bar::status_bar_id;

    /// Two graph panes side by side, both settled — the fixture every
    /// pane-scoping case needs.
    fn two_panes() -> (EditorHarness, GraphRef, GraphRef) {
        let mut h = EditorHarness::new(Document::default());
        let local = h
            .open
            .document
            .create_graph(GraphRef::Main)
            .expect("a local graph");
        let right = GraphRef::Local(local);
        let primary = h.open.document.layout.primary().id;
        h.open
            .document
            .layout
            .find_or_insert(TabRef::Graph(right), primary);
        h.open.document.layout.apply(DockOp::MoveTab {
            tab: TabRef::Graph(right),
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        });
        h.prime(2);
        (h, GraphRef::Main, right)
    }

    /// A focused text field takes the chords it edits with, and only
    /// those — the whole point of `KeyFilter::TEXT_FIELD` omitting
    /// `ACCEL`, and what replaced `dock::typing_focus_held`.
    ///
    /// Driven through a real inline rename rather than by poking focus:
    /// the scope is declared by `TextEdit` while focused, so nothing is
    /// under test unless a real editor is really up.
    #[test]
    fn a_focused_field_takes_edit_chords_and_lets_accelerators_through() {
        use palantir::{Key, Modifiers};

        use crate::gui::app::commands::AppCommand;
        use crate::gui::app::commands::run::RunCommand;

        // Both tabs in one group, so activating the other is a real,
        // recorded `DockOp::ActivateTab` — the undo history the `Edit`
        // leg below needs in order to prove anything.
        let mut h = EditorHarness::new(Document::default());
        let local = h
            .open
            .document
            .create_graph(GraphRef::Main)
            .expect("a local graph");
        let primary = h.open.document.layout.primary().id;
        h.open
            .document
            .layout
            .find_or_insert(TabRef::Graph(GraphRef::Local(local)), primary);
        h.prime(2);
        h.ui.click_on(strip::tab_chip_wid(TabRef::Graph(GraphRef::Main)));
        h.frame();
        h.frame();

        // Double-click the local tab's label to open its inline rename,
        // which focuses a `TextEdit` and so declares the scope.
        let label = strip::tab_rename_wid(local);
        h.ui.click_on(label);
        h.frame();
        h.ui.click_on(label);
        h.frame();
        assert_eq!(
            h.ui.ui().focused_id(),
            Some(label),
            "the double-click must open the rename editor, or nothing is scoped",
        );
        let before = format!("{:?}", h.document().layout);

        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        // `Accel` — not in `TEXT_FIELD`, so it walks past the editor to
        // the app root. This is the case an exclusive capture breaks.
        h.ui.set_modifiers(ctrl);
        h.ui.key(Key::Char('r'));
        assert!(
            matches!(h.frame(), Some(AppCommand::Run(RunCommand::Once))),
            "Ctrl+R must still run the graph while a field is focused",
        );

        // `Edit` — the field takes it, so the document is untouched.
        h.ui.set_modifiers(ctrl);
        h.ui.key(Key::Char('z'));
        h.frame();
        assert_eq!(
            format!("{:?}", h.document().layout),
            before,
            "Ctrl+Z belongs to the focused field, not to the document",
        );

        // Control leg, and the non-vacuity proof: blur the field and the
        // same chord walks the same stack it just refused to.
        h.ui.ui().request_focus(None);
        h.frame();
        h.ui.set_modifiers(ctrl);
        h.ui.key(Key::Char('z'));
        h.frame();
        assert_ne!(
            format!("{:?}", h.document().layout),
            before,
            "with nothing focused Ctrl+Z must undo — otherwise the case above \
             proves nothing",
        );
    }

    /// The bar used to collapse when it had nothing to say; the process
    /// footprint gives it something on every frame, so it is recorded on
    /// an untouched document — and stays recorded when no reading is
    /// available, rather than reappearing as the figure lands.
    #[test]
    fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
        let mut h = EditorHarness::new(Document::default());
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

    /// The whole pipeline in one case: the editor records a real dock
    /// strip, the chip lands where the hit-test says it does, and a
    /// press on it routes through `DockUi::scan` into an undoable
    /// `DocIntent::Dock` that switches the active tab.
    ///
    /// Every step here is what `TestEditor` cannot reach — it calls
    /// `apply_edit` directly and never records, so nothing between the
    /// pointer and the intent is exercised.
    #[test]
    fn clicking_a_tab_chip_activates_it_through_the_real_hit_test() {
        let mut h = EditorHarness::new(Document::default());
        let local = h
            .open
            .document
            .create_graph(GraphRef::Main)
            .expect("a local graph");
        let subgraph = TabRef::Graph(GraphRef::Local(local));
        let primary = h.open.document.layout.primary().id;
        h.open.document.layout.find_or_insert(subgraph, primary);

        // Two frames: one to lay the strip out, one for `response_for`
        // to resolve against a settled cascade.
        h.prime(2);
        assert_eq!(
            h.document().layout.primary().active_tab(),
            TabRef::Graph(GraphRef::Main),
            "inserting a tab does not focus it — Main stays active",
        );

        let chip = strip::tab_chip_wid(subgraph);
        let target = h.ui.center_of(chip);
        // Not `Some(chip)`: a Local tab draws its label as an
        // `InlineRename` whose idle panel senses DRAG and sits over the
        // chip, so the topmost hit is the label. The click still has to
        // reach the chip — that is what the activation below pins, and
        // it is the regression `dock::tests` guards from the other side.
        assert!(
            h.ui.hit_at(target).is_some(),
            "nothing senses input at the chip center {target:?}; collisions: {:?}",
            h.ui.collisions(),
        );

        h.ui.click_at(target);
        let _ = h.frame();

        assert_eq!(
            h.document().layout.primary().active_tab(),
            subgraph,
            "clicking a chip activates its tab",
        );
    }

    /// A pan moves the pane the pointer is over and leaves its
    /// neighbour alone.
    ///
    /// This pins the *outcome*, not either mechanism. Two independent
    /// guards produce it — `emit_pan_zoom` reads each pane's own
    /// `outer_canvas_widget_id` response, and `PanAnchor::apply` refuses
    /// a delta whose key is not the pane that latched. Mutating either
    /// one alone still passes, because the other covers it; mutating
    /// both fails here. That redundancy is worth knowing about: a future
    /// simplification that removes one guard will not be caught by this
    /// test, and `pan_zoom`'s own tests cover `PanAnchor::apply` with a
    /// hand-passed key rather than through a real pointer.
    #[test]
    fn a_pan_moves_only_the_pane_under_the_pointer() {
        let (mut h, left, right) = two_panes();

        let pan_of = |h: &EditorHarness, g: GraphRef| {
            h.document().view(g).expect("an open view").viewport.pan
        };
        let (left_before, right_before) = (pan_of(&h, left), pan_of(&h, right));

        // Guard against the fixture going inert: if the split did not
        // produce two live panes, "the neighbour did not move" passes
        // while pinning nothing.
        assert!(
            h.ui.rect(outer_canvas_widget_id(left)).is_some()
                && h.ui.rect(outer_canvas_widget_id(right)).is_some(),
            "both panes must have arranged canvases",
        );

        // Middle-drag is `CanvasGesture::Pan`; aim at the left pane's
        // own outer canvas so the hit-test decides which pane wins.
        let origin = h.ui.center_of(outer_canvas_widget_id(left));
        h.ui.press_button_at(PointerButton::Middle, origin);
        h.ui.drag_to(origin + Vec2::new(40.0, 25.0));
        let _ = h.frame();

        assert_ne!(
            pan_of(&h, left),
            left_before,
            "the pane under the pointer must pan",
        );
        assert_eq!(
            pan_of(&h, right),
            right_before,
            "the neighbouring pane must not be dragged along",
        );
    }
}
