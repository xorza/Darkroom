//! Keyboard input → intent/command mapping. Kept apart from the
//! per-frame pipeline in `app.rs`: these read palantir's key state and
//! translate chords into queued `Intent`s (canvas edits) or a
//! `MenuCommand` (file ops), never touching the frame orchestration.

use std::collections::BTreeSet;

use palantir::{Key, Shortcut, Ui};

use crate::app::App;
use crate::document::GraphRef;
use crate::edit::intent::{self, Intent, requires_reconcile, requires_relayout};
use crate::gui::menu_bar::MenuCommand;

const UNDO_SHORTCUT: Shortcut = Shortcut::ctrl('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::ctrl_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::ctrl('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::ctrl('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::ctrl('S');
const SAVE_AS_SHORTCUT: Shortcut = Shortcut::ctrl_shift('S');
const RESET_ZOOM_SHORTCUT: Shortcut = Shortcut::ctrl('0');
const DUPLICATE_SHORTCUT: Shortcut = Shortcut::ctrl('D');
const RUN_SHORTCUT: Shortcut = Shortcut::ctrl('R');

impl App {
    /// Ctrl+Z / Ctrl+Shift+Z. Replays undo/redo against the document
    /// (each entry carries its own graph target). Returns whether a
    /// relayout is needed.
    ///
    /// The chords are sampled via `key_pressed` *every frame,
    /// unconditionally* — that call both reads the press and keeps the
    /// chord subscribed, and palantir's keyboard wake-gate only delivers
    /// an off-focus press when its chord was subscribed last frame
    /// (subscriptions clear each frame). Focus only gates the *action*:
    /// while a widget holds focus, Ctrl+Z must undo that widget's text,
    /// so the graph-level handling stands down.
    pub(crate) fn apply_undo_redo(&mut self, ui: &mut Ui) -> bool {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        if ui.focused_id().is_some() {
            return false;
        }
        let mut relayout = false;
        let mut reconcile = false;
        let mut on_step = |step: &intent::UndoStep| {
            relayout |= requires_relayout(step);
            reconcile |= requires_reconcile(step);
        };
        if undo {
            self.action_stack.undo(&mut self.document, &mut on_step);
        } else if redo {
            self.action_stack.redo(&mut self.document, &mut on_step);
        }
        self.needs_reconcile |= reconcile;
        relayout
    }

    /// Esc-deselect and Ctrl+0 reset-zoom — both act on the active view,
    /// so they take the settled `target`. Routed through the intent stack
    /// (not a direct doc write) so they land in the undo history; the
    /// `is_noop` filter in `drain_intents` drops them when they'd change
    /// nothing. Chords are sampled unconditionally (see `apply_undo_redo`)
    /// and gated by focus.
    pub(crate) fn apply_canvas_shortcuts(&mut self, ui: &mut Ui, target: GraphRef) -> bool {
        let reset_zoom = ui.key_pressed(RESET_ZOOM_SHORTCUT);
        let escape = ui.escape_pressed();
        let duplicate = ui.key_pressed(DUPLICATE_SHORTCUT);
        // Sampled before the focus gate so the chords stay subscribed for
        // palantir's wake-gate even on a focused frame.
        let delete = ui.key_pressed(Shortcut::key(Key::Delete))
            || ui.key_pressed(Shortcut::key(Key::Backspace));
        if ui.focused_id().is_some() {
            return false;
        }
        let view = self.document.view(target).expect("active tab view exists");
        let has_selection = !view.selected_nodes.is_empty();
        let pan = view.pan;
        if escape && has_selection {
            self.intents.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        if reset_zoom {
            self.intents.push(Intent::SetViewport { pan, scale: 1.0 });
        }
        if duplicate && let Some(intent) = self.document.duplicate_intent(target) {
            self.intents.push(intent);
        }
        // Delete/Backspace removes the whole selection. One `RemoveNode`
        // per node; `drain_intents` batches a frame's intents into a single
        // undo entry, so it's one Cmd-Z (mirrors the breaker's multi-delete).
        if delete {
            let ids: Vec<_> = self
                .document
                .view(target)
                .expect("active tab view exists")
                .selected_nodes
                .iter()
                .copied()
                .collect();
            for node_id in ids {
                self.intents.push(Intent::RemoveNode { node_id });
            }
        }
        false
    }

    /// Map Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S / Ctrl+R to a `MenuCommand`.
    ///
    /// Document file ops are **global** — they fire regardless of
    /// focus, so Ctrl+S still saves while a node's value editor is
    /// focused (TextEdit doesn't bind S/O/N, so nothing is stolen).
    /// Every chord is sampled with `key_pressed` each frame so all
    /// stay subscribed for palantir's wake-gate (sampling all four up
    /// front, not short-circuited, so one chord firing doesn't drop
    /// the others' subscription that frame). Save-As (Ctrl+Shift+S) is
    /// checked before Save (Ctrl+S) so the shift variant wins its
    /// combo. Theme actions are menu-only — no shortcut.
    pub(crate) fn menu_shortcut(&self, ui: &mut Ui) -> Option<MenuCommand> {
        let new = ui.key_pressed(NEW_SHORTCUT);
        let open = ui.key_pressed(OPEN_SHORTCUT);
        let save_as = ui.key_pressed(SAVE_AS_SHORTCUT);
        let save = ui.key_pressed(SAVE_SHORTCUT);
        let run = ui.key_pressed(RUN_SHORTCUT);
        if new {
            Some(MenuCommand::NewDocument)
        } else if open {
            Some(MenuCommand::LoadDocument)
        } else if save_as {
            Some(MenuCommand::SaveDocumentAs)
        } else if save {
            Some(MenuCommand::SaveDocument)
        } else if run {
            Some(MenuCommand::Run)
        } else {
            None
        }
    }
}
