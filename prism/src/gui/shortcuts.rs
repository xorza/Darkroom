//! Keyboard-shortcut → command mapping. Pure function over
//! `(InputSnapshot, autorun)` so the routing table is unit-testable
//! without an `egui::Context` or `Session`.

use eframe::egui;

use crate::gui::graph_ui::frame_output::{AppCommand, EditorCommand, FrameOutput, RunCommand};
use crate::input::InputSnapshot;

/// Commands a single frame's shortcuts can emit. Each field is at most
/// one — same precedence semantics the original sequential `if/else`
/// chain enforced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ShortcutCommands {
    pub run_cmd: Option<RunCommand>,
    pub app_cmd: Option<AppCommand>,
    pub editor_cmd: Option<EditorCommand>,
}

impl ShortcutCommands {
    pub(super) fn apply(self, output: &mut FrameOutput) {
        if let Some(cmd) = self.run_cmd {
            output.set_run_cmd(cmd);
        }
        if let Some(cmd) = self.app_cmd {
            output.set_app_cmd(cmd);
        }
        if let Some(cmd) = self.editor_cmd {
            output.set_editor_cmd(cmd);
        }
    }
}

/// Resolve which commands the current frame's input fires. `autorun`
/// flips Cmd+Shift+Space between Start/Stop. Within each command
/// category the first matching branch wins; shift-prefixed shortcuts
/// must be checked before their bare counterparts.
pub(super) fn shortcut_commands(input: &InputSnapshot, autorun: bool) -> ShortcutCommands {
    let editor_cmd = if input.cmd_only(egui::Key::Z) {
        Some(EditorCommand::Undo)
    } else if input.cmd_shift(egui::Key::Z) {
        Some(EditorCommand::Redo)
    } else {
        None
    };

    let app_cmd = if input.cmd(egui::Key::Q) {
        // Quit wins over Save/Open if both fire (it shouldn't — different keys).
        Some(AppCommand::Exit)
    } else if input.cmd_shift(egui::Key::S) {
        Some(AppCommand::SaveAs)
    } else if input.cmd_only(egui::Key::S) {
        Some(AppCommand::Save)
    } else if input.cmd(egui::Key::O) {
        Some(AppCommand::Open)
    } else {
        None
    };

    let run_cmd = if input.cmd_shift(egui::Key::Space) {
        Some(if autorun {
            RunCommand::StopAutorun
        } else {
            RunCommand::StartAutorun
        })
    } else if input.cmd_only(egui::Key::Space) {
        Some(RunCommand::RunOnce)
    } else {
        None
    };

    ShortcutCommands {
        run_cmd,
        app_cmd,
        editor_cmd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn input_with(modifiers: Modifiers, keys: &[Key]) -> InputSnapshot {
        InputSnapshot {
            modifiers,
            keys_pressed: keys.to_vec(),
            ..InputSnapshot::default()
        }
    }

    #[test]
    fn cmd_z_emits_undo() {
        let cmds = shortcut_commands(&input_with(Modifiers::COMMAND, &[Key::Z]), false);
        assert_eq!(cmds.editor_cmd, Some(EditorCommand::Undo));
        assert_eq!(cmds.app_cmd, None);
        assert_eq!(cmds.run_cmd, None);
    }

    #[test]
    fn cmd_shift_z_emits_redo() {
        let cmds = shortcut_commands(
            &input_with(Modifiers::COMMAND | Modifiers::SHIFT, &[Key::Z]),
            false,
        );
        assert_eq!(cmds.editor_cmd, Some(EditorCommand::Redo));
    }

    #[test]
    fn cmd_s_emits_save_and_cmd_shift_s_emits_save_as() {
        let save = shortcut_commands(&input_with(Modifiers::COMMAND, &[Key::S]), false);
        assert_eq!(save.app_cmd, Some(AppCommand::Save));

        let save_as = shortcut_commands(
            &input_with(Modifiers::COMMAND | Modifiers::SHIFT, &[Key::S]),
            false,
        );
        assert_eq!(save_as.app_cmd, Some(AppCommand::SaveAs));
    }

    #[test]
    fn cmd_o_emits_open() {
        let cmds = shortcut_commands(&input_with(Modifiers::COMMAND, &[Key::O]), false);
        assert_eq!(cmds.app_cmd, Some(AppCommand::Open));
    }

    #[test]
    fn cmd_q_emits_exit() {
        let cmds = shortcut_commands(&input_with(Modifiers::COMMAND, &[Key::Q]), false);
        assert_eq!(cmds.app_cmd, Some(AppCommand::Exit));
    }

    #[test]
    fn cmd_space_emits_run_once() {
        let cmds = shortcut_commands(&input_with(Modifiers::COMMAND, &[Key::Space]), false);
        assert_eq!(cmds.run_cmd, Some(RunCommand::RunOnce));
    }

    /// Cmd+Shift+Space is a toggle: idle → Start, autorun → Stop.
    /// Both directions must be testable from the pure mapping.
    #[test]
    fn cmd_shift_space_toggles_autorun() {
        let when_idle = shortcut_commands(
            &input_with(Modifiers::COMMAND | Modifiers::SHIFT, &[Key::Space]),
            false,
        );
        assert_eq!(when_idle.run_cmd, Some(RunCommand::StartAutorun));

        let when_running = shortcut_commands(
            &input_with(Modifiers::COMMAND | Modifiers::SHIFT, &[Key::Space]),
            true,
        );
        assert_eq!(when_running.run_cmd, Some(RunCommand::StopAutorun));
    }

    /// Without command modifier, none of the shortcuts fire.
    #[test]
    fn bare_keys_emit_nothing() {
        for key in [Key::Z, Key::S, Key::O, Key::Q, Key::Space] {
            let cmds = shortcut_commands(&input_with(Modifiers::default(), &[key]), false);
            assert_eq!(cmds, ShortcutCommands::default(), "bare {key:?}");
        }
    }

    /// Cmd+Shift+S must take the SaveAs branch, not the Save one.
    /// The original code's ordering bug-class: shift-prefixed branches
    /// have to be checked first.
    #[test]
    fn cmd_shift_s_does_not_also_emit_save() {
        let cmds = shortcut_commands(
            &input_with(Modifiers::COMMAND | Modifiers::SHIFT, &[Key::S]),
            false,
        );
        assert_eq!(cmds.app_cmd, Some(AppCommand::SaveAs));
    }

    /// Empty input produces no commands.
    #[test]
    fn no_keys_pressed_emits_nothing() {
        let cmds = shortcut_commands(&InputSnapshot::default(), false);
        assert_eq!(cmds, ShortcutCommands::default());
    }
}
