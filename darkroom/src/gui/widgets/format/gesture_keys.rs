//! [`GestureKeys`]: what a pointer gesture is reached by, written as the host writes a chord.

use std::fmt;
use std::fmt::{Display, Formatter};
use std::io::{Cursor, Write as _};
use std::str;

use palantir::{Key, Shortcut, ShortcutMods};

/// The modifiers a pointer gesture holds and the pointer's part of it, as `Ctrl+Shift+drag`, or
/// `⇧⌘ drag` where the host writes a chord in symbols. The reference and the status bar's hints
/// read it, so a gesture's keys never render two ways.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GestureKeys {
    pub(crate) mods: ShortcutMods,
    /// The pointer's part: a click or a drag, and with which button.
    pub(crate) pointer: &'static str,
}

/// A key whose label is its name on every host, which the modifiers' label is read off.
const PROBE: Key = Key::F1;
const PROBE_LABEL: &str = "F1";

impl Display for GestureKeys {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self { mods, pointer } = *self;
        if mods == ShortcutMods::NONE {
            return f.write_str(pointer);
        }
        // palantir labels a chord, not its modifiers alone, so the modifiers' label is a
        // chord's less its key: the command key reads as the host names it, with no platform
        // read here. A stack buffer, as a hint formats once a frame.
        let mut buffer = [0_u8; 48];
        let mut cursor = Cursor::new(&mut buffer[..]);
        write!(cursor, "{}", Shortcut::new(mods, PROBE)).map_err(|_| fmt::Error)?;
        let len = usize::try_from(cursor.position()).map_err(|_| fmt::Error)?;
        let label = str::from_utf8(&buffer[..len]).map_err(|_| fmt::Error)?;
        let held = label.strip_suffix(PROBE_LABEL).unwrap_or(label);
        f.write_str(held)?;
        // Names join with a `+` of their own; symbols take a space before the word.
        if !held.ends_with('+') {
            f.write_str(" ")?;
        }
        f.write_str(pointer)
    }
}

#[cfg(test)]
mod tests {
    use palantir::{Key, Shortcut, ShortcutMods};

    use crate::gui::widgets::format::gesture_keys::GestureKeys;

    /// No modifiers read as the pointer's part alone; with some, the keys are the host's label of
    /// a chord of them less its key, then the pointer's part, joined as the host joins a chord.
    #[test]
    fn the_keys_read_as_the_host_writes_a_chord() {
        let bare = GestureKeys {
            mods: ShortcutMods::NONE,
            pointer: "drag",
        };
        assert_eq!(bare.to_string(), "drag");

        let mods = ShortcutMods::CTRL.union(ShortcutMods::SHIFT);
        let chord = Shortcut::new(mods, Key::F1).to_string();
        let held = chord.strip_suffix("F1").expect("F1 is labeled by its name");
        let joined = if held.ends_with('+') {
            format!("{held}right-drag")
        } else {
            format!("{held} right-drag")
        };
        let keys = GestureKeys {
            mods,
            pointer: "right-drag",
        };
        assert_eq!(keys.to_string(), joined);
        assert!(!keys.to_string().contains("F1"));
    }
}
