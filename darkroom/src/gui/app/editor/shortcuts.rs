//! Keyboard input → intent/command mapping. A child module of `editor`:
//! these read palantir's key state and translate chords into queued
//! `GraphIntent`s (canvas edits) or a `AppCommand` (file ops). Being a child
//! lets them drive the pipeline through `Editor`'s private fields without
//! widening visibility; they never touch the frame orchestration.

use palantir::{Shortcut, Ui};

use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::types::UndoStep;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::app::commands::shell::ShellCommand;
use crate::gui::app::editor::{Editor, StepSignals};

const UNDO_SHORTCUT: Shortcut = Shortcut::ctrl('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::ctrl_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::ctrl('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::ctrl('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::ctrl('S');
const SAVE_AS_SHORTCUT: Shortcut = Shortcut::ctrl_shift('S');
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
    /// (subscriptions clear each frame).
    ///
    /// No focus test: Ctrl+Z is `KeyClass::Edit`, so while a text field
    /// holds focus palantir grants it to that field's scope and this
    /// read answers `false` on its own.
    pub(super) fn apply_undo_redo(&mut self, ui: &mut Ui, open: &mut OpenDocument) {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        // Folded into a value first: the replay callback runs while
        // `action_stack` is mutably borrowed, so it can't touch `self`.
        let mut signals = StepSignals::default();
        let mut on_step = |step: &UndoStep| signals.fold(step);
        if undo {
            self.action_stack.undo(&mut open.document, &mut on_step);
        } else if redo {
            self.action_stack.redo(&mut open.document, &mut on_step);
        }
        self.absorb_signals(open, signals);
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
