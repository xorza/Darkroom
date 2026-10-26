//! [`Action`]: one step of input the harness plays into the window, and the key chords it names.

use std::path::PathBuf;

use glam::Vec2;
use palantir::{Key, Modifiers, PointerButton};

use crate::internals::harness::error::HarnessError;
use crate::internals::harness::locator::Locator;

/// Pointer moves a drag takes from its start to its end when the call names none, enough to pass
/// any drag threshold in small steps.
pub const DRAG_STEPS: u32 = 8;

/// One step of input. The harness runs frames until the editor settles after each.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Move the pointer there, as a hover.
    Move(Locator),
    /// Press and release `button` there, holding `mods`.
    Click {
        at: Locator,
        button: PointerButton,
        mods: Modifiers,
    },
    /// Two left clicks there, close enough in time to count as one double click.
    DoubleClick(Locator),
    /// Press `button` at `from`, move to `to` in `steps` moves, and release there.
    Drag {
        from: Locator,
        to: Locator,
        button: PointerButton,
        mods: Modifiers,
        steps: u32,
    },
    /// Scroll there by `delta` logical pixels; positive `y` scrolls content down.
    Scroll { at: Locator, delta: Vec2 },
    /// Pinch there by `factor`: above 1 zooms in.
    Pinch { at: Locator, factor: f32 },
    /// Press one key chord, as `ctrl+z`, `shift+delete`, `enter` or `a`.
    Key(String),
    /// Type text into whatever holds the keyboard focus.
    Type(String),
    /// Queue the answer the next file dialog gets.
    Answer(DialogAnswer),
}

/// One scripted answer to a file dialog, which the next dialog the app opens takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogAnswer {
    /// One path, for every ask but the multi-file pick.
    Path(PathBuf),
    /// The multi-file pick's paths.
    Paths(Vec<PathBuf>),
    /// The user closed the dialog.
    Cancel,
}

/// A key and the modifiers held for it, parsed from a chord such as `ctrl+shift+z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chord {
    pub(crate) mods: Modifiers,
    pub(crate) key: Key,
}

impl Chord {
    /// `chord`: modifiers and one key joined by `+`, without regard to case. The modifiers are
    /// `ctrl` (Cmd on macOS, as palantir reads it), `shift`, `alt` and `meta`; the key is a named
    /// key or one character.
    pub(crate) fn parse(chord: &str) -> Result<Self, HarnessError> {
        let bad = || HarnessError::BadChord {
            chord: chord.to_string(),
        };
        let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
        // `ctrl++` names the plus key: the split leaves two empty parts at the end.
        if chord.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let (key, mods_named) = parts.split_last().ok_or_else(bad)?;
        let mut mods = Modifiers::default();
        for named in mods_named {
            match named.to_lowercase().as_str() {
                "ctrl" | "cmd" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" => mods.alt = true,
                "meta" | "super" => mods.meta = true,
                _ => return Err(bad()),
            }
        }
        let key = match key.to_lowercase().as_str() {
            "enter" | "return" => Key::Enter,
            "escape" | "esc" => Key::Escape,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "left" => Key::ArrowLeft,
            "right" => Key::ArrowRight,
            "up" => Key::ArrowUp,
            "down" => Key::ArrowDown,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "space" => Key::Char(' '),
            "f1" => Key::F1,
            "f2" => Key::F2,
            "f3" => Key::F3,
            "f4" => Key::F4,
            "f5" => Key::F5,
            "f6" => Key::F6,
            "f7" => Key::F7,
            "f8" => Key::F8,
            "f9" => Key::F9,
            "f10" => Key::F10,
            "f11" => Key::F11,
            "f12" => Key::F12,
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                    _ => return Err(bad()),
                }
            }
        };
        Ok(Self { mods, key })
    }

    /// Whether the chord types its character: with no command modifier held, as a layout does.
    pub(crate) const fn types(self) -> bool {
        !(self.mods.ctrl || self.mods.alt || self.mods.meta)
    }
}

/// `from` to `to` in `steps` even moves, `to` last.
pub(crate) fn path(from: Vec2, to: Vec2, steps: u32) -> impl Iterator<Item = Vec2> {
    let steps = steps.max(1);
    (1..=steps).map(move |step| from.lerp(to, step as f32 / steps as f32))
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::{Key, Modifiers};

    use crate::internals::harness::action::{Chord, path};

    /// Each modifier, named keys, one character in either case, and the plus key; a word that is
    /// no key and an unknown modifier are refused.
    #[test]
    fn chords_parse_to_their_key_and_modifiers() {
        let chord = |mods: Modifiers, key: Key| Chord { mods, key };
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(Chord::parse("ctrl+z").unwrap(), chord(ctrl, Key::Char('z')));
        assert_eq!(
            Chord::parse("Ctrl+Shift+Z").unwrap(),
            chord(
                Modifiers {
                    shift: true,
                    ..ctrl
                },
                Key::Char('z')
            )
        );
        assert_eq!(
            Chord::parse("alt+meta+f2").unwrap(),
            chord(
                Modifiers {
                    alt: true,
                    meta: true,
                    ..Modifiers::default()
                },
                Key::F2
            )
        );
        assert_eq!(Chord::parse("escape").unwrap().key, Key::Escape);
        assert_eq!(Chord::parse("ctrl++").unwrap(), chord(ctrl, Key::Char('+')));
        assert!(Chord::parse("a").unwrap().types());
        assert!(!Chord::parse("ctrl+a").unwrap().types());
        assert!(Chord::parse("ctrl+banana").is_err());
        assert!(Chord::parse("hyper+a").is_err());
        assert!(Chord::parse("").is_err());
    }

    /// Four steps from (0, 0) to (8, 4) land a quarter of the way at a time, ending on the target.
    #[test]
    fn a_drag_path_ends_on_its_target() {
        let points: Vec<Vec2> = path(Vec2::ZERO, Vec2::new(8.0, 4.0), 4).collect();
        assert_eq!(
            points,
            [
                Vec2::new(2.0, 1.0),
                Vec2::new(4.0, 2.0),
                Vec2::new(6.0, 3.0),
                Vec2::new(8.0, 4.0)
            ]
        );
    }
}
