//! Keyboard input → intent/command mapping. A child module of `editor`:
//! these read palantir's key state and translate chords into queued
//! `Intent`s (canvas edits) or a `AppCommand` (file ops). Being a child
//! lets them drive the pipeline through `Editor`'s private fields without
//! widening visibility; they never touch the frame orchestration.

use std::collections::BTreeSet;

use palantir::{Key, Shortcut, Ui};

use crate::core::document::Viewport;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::duplicate::{build_duplicate_intent, remove_selection_intents};
use crate::core::edit::intent::types::{Intent, UndoStep};
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::app::commands::shell::ShellCommand;
use crate::gui::app::editor::{Editor, StepSignals};
use crate::gui::dock;

const UNDO_SHORTCUT: Shortcut = Shortcut::ctrl('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::ctrl_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::ctrl('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::ctrl('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::ctrl('S');
const SAVE_AS_SHORTCUT: Shortcut = Shortcut::ctrl_shift('S');
const RESET_ZOOM_SHORTCUT: Shortcut = Shortcut::ctrl('0');
const DUPLICATE_SHORTCUT: Shortcut = Shortcut::ctrl('D');
const RUN_SHORTCUT: Shortcut = Shortcut::ctrl('R');
/// ⌘Q on macOS, Ctrl+Q elsewhere. Routes through `AppCommand::Shell(ShellCommand::Quit)` →
/// `App::request_quit`, so it prompts to save when the document is dirty
/// — same path as File ▸ Quit. (palantir drops winit's default macOS menu
/// so ⌘Q reaches us instead of hard-terminating.)
const QUIT_SHORTCUT: Shortcut = Shortcut::ctrl('Q');

impl Editor {
    /// Ctrl+Z / Ctrl+Shift+Z. Replays undo/redo against the document
    /// (each entry carries its own graph target). Returns whether a
    /// relayout is needed.
    ///
    /// The chords are sampled via `key_pressed` *every frame,
    /// unconditionally* — that call both reads the press and keeps the
    /// chord subscribed, and palantir's keyboard wake-gate only delivers
    /// an off-focus press when its chord was subscribed last frame
    /// (subscriptions clear each frame). Focus only gates the *action*:
    /// while a text widget holds focus, Ctrl+Z must undo that widget's
    /// text, so the graph-level handling stands down. A focused *pane*
    /// doesn't count — panes are focusable purely to route dock focus
    /// (`dock::typing_focus_held`).
    pub(super) fn apply_undo_redo(&mut self, ui: &mut Ui, open: &mut OpenDocument) {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        if dock::typing_focus_held(ui, &open.document) {
            return;
        }
        // Folded into a value first: the replay callback runs while
        // `action_stack` is mutably borrowed, so it can't touch `self`.
        let mut signals = StepSignals::default();
        let mut on_step = |step: &UndoStep| signals.fold(step);
        if undo {
            self.action_stack.undo(&mut open.document, &mut on_step);
        } else if redo {
            self.action_stack.redo(&mut open.document, &mut on_step);
        }
        self.absorb_signals(signals);
    }

    /// Esc-deselect, Ctrl+0 reset-zoom, Ctrl+D duplicate, and
    /// Delete/Backspace. A keyboard chord has no pane under a pointer to
    /// name, so it acts on the **focused** graph — the same rule every
    /// other off-canvas edit follows. Routed through the intent stack (not
    /// a direct doc write) so they land in the undo history; the `is_noop`
    /// filter in `drain_intents` drops them when they'd change nothing.
    /// Chords are sampled unconditionally (see `apply_undo_redo`) and
    /// gated by focus. Pushes intents only — their relayout is decided by
    /// the post-record drain, so this returns nothing.
    pub(super) fn apply_canvas_shortcuts(&mut self, ui: &mut Ui, open: &OpenDocument) {
        let reset_zoom = ui.key_pressed(RESET_ZOOM_SHORTCUT);
        let escape = ui.escape_pressed();
        let duplicate = ui.key_pressed(DUPLICATE_SHORTCUT);
        // Sampled before the focus gate so the chords stay subscribed for
        // palantir's wake-gate even on a focused frame.
        let delete = ui.key_pressed(Shortcut::key(Key::Delete))
            || ui.key_pressed(Shortcut::key(Key::Backspace));
        if dock::typing_focus_held(ui, &open.document) {
            return;
        }
        let Some(target) = open.document.focused_target() else {
            return;
        };
        let Some(view) = open.document.view(target) else {
            return;
        };
        let has_selection = !view.selected.is_empty();
        let pan = view.viewport.pan;
        let document = &open.document;
        let out = &mut self.intents;
        if escape && has_selection {
            out.push(
                target,
                Intent::SetSelection {
                    to: BTreeSet::new(),
                },
            );
        }
        if reset_zoom {
            out.push(
                target,
                Intent::SetViewport {
                    to: Viewport { pan, zoom: 1.0 },
                },
            );
        }
        if duplicate && let Some(intent) = build_duplicate_intent(document, target) {
            out.push(target, intent);
        }
        // Delete/Backspace removes the whole selection — one
        // `RemoveNode` each. `drain_intents` batches a frame's intents into a single
        // undo entry, so it's one Cmd-Z (mirrors the breaker's
        // multi-delete).
        if delete {
            out.extend(target, remove_selection_intents(&view.selected));
        }
    }

    /// Map Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S / Ctrl+R to a `AppCommand`.
    ///
    /// Document file ops are **global** — they fire regardless of
    /// focus, so Ctrl+S still saves while a node's value editor is
    /// focused (TextEdit doesn't bind S/O/N, so nothing is stolen).
    /// Every chord is sampled with `key_pressed` each frame so all
    /// stay subscribed for palantir's wake-gate (sampling them all up
    /// front, not short-circuited, so one chord firing doesn't drop
    /// the others' subscription that frame). Save-As (Ctrl+Shift+S) is
    /// checked before Save (Ctrl+S) so the shift variant wins its
    /// combo. Theme actions are menu-only — no shortcut.
    pub(super) fn menu_shortcut(&self, ui: &mut Ui) -> Option<AppCommand> {
        let new = ui.key_pressed(NEW_SHORTCUT);
        let open = ui.key_pressed(OPEN_SHORTCUT);
        let save_as = ui.key_pressed(SAVE_AS_SHORTCUT);
        let save = ui.key_pressed(SAVE_SHORTCUT);
        let run = ui.key_pressed(RUN_SHORTCUT);
        let quit = ui.key_pressed(QUIT_SHORTCUT);
        if new {
            Some(AppCommand::File(FileCommand::New))
        } else if open {
            Some(AppCommand::File(FileCommand::Load))
        } else if save_as {
            Some(AppCommand::File(FileCommand::SaveAs))
        } else if save {
            Some(AppCommand::File(FileCommand::Save))
        } else if run {
            Some(AppCommand::Run(RunCommand::Once))
        } else if quit {
            Some(AppCommand::Shell(ShellCommand::Quit))
        } else {
            None
        }
    }
}
