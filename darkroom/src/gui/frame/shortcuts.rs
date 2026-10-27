//! Every keyboard chord the editor binds, in one place: the handlers read them to act, and the
//! menus read them to show the key beside the item.

use palantir::{Key, Shortcut};

pub(crate) const NEW: Shortcut = Shortcut::ctrl('N');
pub(crate) const OPEN: Shortcut = Shortcut::ctrl('O');
pub(crate) const SAVE: Shortcut = Shortcut::ctrl('S');
pub(crate) const SAVE_AS: Shortcut = Shortcut::ctrl_shift('S');
/// ⌘Q on macOS, Ctrl+Q elsewhere. Routes through `AppCommand::Quit` →
/// `App::guard_discard`, so it prompts to save when the document is dirty
/// — same path as File ▸ Quit. (palantir drops winit's default macOS menu
/// so ⌘Q reaches us instead of hard-terminating.)
pub(crate) const QUIT: Shortcut = Shortcut::ctrl('Q');
pub(crate) const UNDO: Shortcut = Shortcut::ctrl('Z');
pub(crate) const REDO: Shortcut = Shortcut::ctrl_shift('Z');
pub(crate) const DUPLICATE: Shortcut = Shortcut::ctrl('D');
/// Backspace removes the selection too; the menu shows Delete, the key a keyboard labels so.
pub(crate) const DELETE: Shortcut = Shortcut::key(Key::Delete);
pub(crate) const RUN: Shortcut = Shortcut::ctrl('R');
pub(crate) const RESET_VIEW: Shortcut = Shortcut::ctrl('0');
