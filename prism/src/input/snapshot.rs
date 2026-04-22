use egui::{Context, Event, Key, Modifiers, MouseWheelUnit, Pos2, Vec2};

/// Snapshot of everything the prism view/interaction layer needs to know
/// about input for one frame, captured once at frame start.
#[derive(Debug, Clone, Default)]
pub struct InputSnapshot {
    pub pointer_pos: Option<Pos2>,
    pub interact_pos: Option<Pos2>,

    pub primary_pressed: bool,
    pub primary_down: bool,
    pub secondary_pressed: bool,
    /// Any pointer button was newly pressed this frame.
    pub any_pointer_pressed: bool,

    pub modifiers: Modifiers,
    /// Keys whose `pressed` event fired this frame.
    pub keys_pressed: Vec<Key>,

    /// Accumulated smooth scroll delta plus any `MouseWheelUnit::Point` events.
    pub scroll_delta: Vec2,
    /// Magnitude of any `MouseWheelUnit::Line`/`Page` events (y component of
    /// the last such event in the frame; matches the prior behaviour of
    /// `collect_scroll_mouse_wheel_deltas`).
    pub wheel_lines: f32,

    /// Raw egui zoom delta (pinch / scroll-zoom gesture) — 1.0 if no zoom.
    pub zoom_delta: f32,
}

impl InputSnapshot {
    /// Samples the context's current input state once.
    pub fn capture(ctx: &Context) -> Self {
        ctx.input(|i| {
            let mut scroll_delta = i.smooth_scroll_delta;
            let mut wheel_lines = 0.0_f32;
            let mut keys_pressed = Vec::new();

            for event in &i.events {
                match event {
                    Event::MouseWheel { unit, delta, .. } => match unit {
                        MouseWheelUnit::Point => scroll_delta += *delta,
                        MouseWheelUnit::Line | MouseWheelUnit::Page => {
                            wheel_lines = delta.y;
                        }
                    },
                    Event::Key {
                        key, pressed: true, ..
                    } => {
                        keys_pressed.push(*key);
                    }
                    _ => {}
                }
            }

            Self {
                pointer_pos: i.pointer.hover_pos(),
                interact_pos: i.pointer.interact_pos(),
                primary_pressed: i.pointer.primary_pressed(),
                primary_down: i.pointer.primary_down(),
                secondary_pressed: i.pointer.secondary_pressed(),
                any_pointer_pressed: i.pointer.any_pressed(),
                modifiers: i.modifiers,
                keys_pressed,
                scroll_delta,
                wheel_lines,
                zoom_delta: i.zoom_delta(),
            }
        })
    }

    pub fn key_pressed(&self, key: Key) -> bool {
        self.keys_pressed.contains(&key)
    }

    pub fn escape_pressed(&self) -> bool {
        self.key_pressed(Key::Escape)
    }

    /// `command+key` with shift not required and not forbidden.
    pub fn cmd(&self, key: Key) -> bool {
        self.key_pressed(key) && self.modifiers.command
    }

    /// `command+key`, shift must be up.
    pub fn cmd_only(&self, key: Key) -> bool {
        self.cmd(key) && !self.modifiers.shift
    }

    /// `command+shift+key`.
    pub fn cmd_shift(&self, key: Key) -> bool {
        self.cmd(key) && self.modifiers.shift
    }

    /// Whether interaction should be cancelled this frame (Escape or
    /// secondary click).
    pub fn cancel_requested(&self) -> bool {
        self.escape_pressed() || self.secondary_pressed
    }

    /// Egui's pinch/scroll-zoom factor, but with a `command` held down
    /// meaning "cmd-scroll is for panning" — returns 1.0 in that case so
    /// scroll_delta takes effect instead.
    pub fn zoom_delta_unless_cmd(&self) -> f32 {
        if self.modifiers.command {
            1.0
        } else {
            self.zoom_delta
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn snapshot_with_keys(keys: &[Key], modifiers: Modifiers) -> InputSnapshot {
        InputSnapshot {
            keys_pressed: keys.to_vec(),
            modifiers,
            ..InputSnapshot::default()
        }
    }

    #[test]
    fn cmd_only_requires_command_without_shift() {
        let s = snapshot_with_keys(
            &[Key::S],
            Modifiers {
                command: true,
                shift: false,
                ..Modifiers::default()
            },
        );
        assert!(s.cmd_only(Key::S));
        assert!(!s.cmd_shift(Key::S));
        assert!(s.cmd(Key::S));
    }

    #[test]
    fn cmd_shift_requires_both() {
        let s = snapshot_with_keys(
            &[Key::Z],
            Modifiers {
                command: true,
                shift: true,
                ..Modifiers::default()
            },
        );
        assert!(s.cmd_shift(Key::Z));
        assert!(!s.cmd_only(Key::Z));
        assert!(s.cmd(Key::Z));
    }

    #[test]
    fn cmd_family_rejects_missing_command() {
        let s = snapshot_with_keys(&[Key::S], Modifiers::default());
        assert!(!s.cmd(Key::S));
        assert!(!s.cmd_only(Key::S));
        assert!(!s.cmd_shift(Key::S));
    }

    #[test]
    fn escape_and_cancel() {
        let s = snapshot_with_keys(&[Key::Escape], Modifiers::default());
        assert!(s.escape_pressed());
        assert!(s.cancel_requested());

        let s = InputSnapshot {
            secondary_pressed: true,
            ..InputSnapshot::default()
        };
        assert!(s.cancel_requested());
    }

    #[test]
    fn zoom_delta_unless_cmd() {
        let mut s = InputSnapshot {
            zoom_delta: 1.25,
            ..InputSnapshot::default()
        };
        assert!((s.zoom_delta_unless_cmd() - 1.25).abs() < 1e-6);
        s.modifiers.command = true;
        assert!((s.zoom_delta_unless_cmd() - 1.0).abs() < 1e-6);
    }
}
