//! The unsaved-changes guard: the transition that would replace or discard the open document,
//! held while its prompt is up, and the prompt itself, whose answer `App` applies right after the
//! dialog records.

use std::path::PathBuf;

use palantir::{Button, Checkbox, Configure, Modal, Panel, Sizing, Text, TextWrap, Ui, WidgetId};

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// The transition waiting on the unsaved-changes prompt, and so whether the prompt is up.
#[derive(Debug, Default)]
pub(super) struct DiscardGuard {
    pending: Option<PendingTransition>,
}

/// An answer to the prompt and the transition it answers, taken off the guard.
#[derive(Debug)]
pub(super) struct Answered {
    pub(super) transition: PendingTransition,
    pub(super) outcome: DiscardOutcome,
}

impl DiscardGuard {
    /// Raise the prompt for `transition`, in place of any it held.
    pub(super) fn hold(&mut self, transition: PendingTransition) {
        self.pending = Some(transition);
    }

    /// The prompt over this frame while it is up, naming the document `file_name` (`None` for
    /// one never saved), and the answer given this frame. The window stays open while it is up.
    pub(super) fn record(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        file_name: Option<&str>,
    ) -> Option<Answered> {
        let tail = self.pending.as_ref()?.prompt_tail();
        if ui.close_requested() {
            ui.keep_open();
        }
        let outcome = show(ui, theme, file_name, tail);
        self.answer(outcome)
    }

    /// The transition `outcome` answers, taken off the guard; `None` while no button is pressed.
    pub(super) fn answer(&mut self, outcome: DiscardOutcome) -> Option<Answered> {
        if outcome.choice == DiscardChoice::Stay {
            return None;
        }
        let transition = self.pending.take()?;
        Some(Answered {
            transition,
            outcome,
        })
    }
}

/// A transition that replaces or discards the open document. Held while
/// the unsaved-changes prompt is up, then carried out (or dropped) by the
/// answer.
///
/// The two open variants differ only in where the path comes from:
/// [`Self::OpenPicked`] runs the file dialog *after* the prompt clears, so a
/// cancelled prompt doesn't leave the user having chosen a file for nothing,
/// while [`Self::OpenAt`] already holds one handed in from outside the editor
/// (Finder, via [`crate::platform`]). Carrying that path is what
/// costs this `Copy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingTransition {
    Quit,
    New,
    OpenPicked,
    OpenAt(PathBuf),
}

impl PendingTransition {
    /// How the prompt finishes "Save changes to X before …?".
    const fn prompt_tail(&self) -> &'static str {
        match self {
            Self::Quit => "quitting",
            Self::New => "closing it",
            Self::OpenPicked | Self::OpenAt(_) => "opening another document",
        }
    }
}

/// The user's answer to the unsaved-changes prompt for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscardChoice {
    /// No button pressed yet — keep the dialog up.
    Stay,
    /// Keep editing (Cancel button, Esc, or backdrop click).
    Cancel,
    /// Go ahead without saving.
    Discard,
    /// Save first, then go ahead.
    Save,
}

/// What the exit dialog reported this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiscardOutcome {
    pub(super) choice: DiscardChoice,
    /// "Don't ask again" checkbox state. Honored only once the transition
    /// actually goes through — a `Cancel`, or a `Save` the user then
    /// cancelled, leaves the preference untouched.
    pub(super) dont_ask_again: bool,
}

/// What answering the prompt does, beyond dismissing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiscardResolution {
    /// Carry out the pending transition.
    pub(super) proceed: bool,
    /// Persist "Don't ask again".
    pub(super) silence_prompt: bool,
}

impl DiscardOutcome {
    /// Resolve the answer against whether the document is *still* dirty
    /// after any save the answer asked for.
    ///
    /// Both effects hang off the transition actually going through, and
    /// `silence_prompt` says so in one line. A `Save` the user then
    /// cancelled — or one that failed — leaves the document dirty, so it
    /// neither proceeds nor silences: silencing there would disable the
    /// guard on the strength of a save that never happened, and the next
    /// discard would go unprompted.
    pub(super) const fn resolve(self, still_dirty: bool) -> DiscardResolution {
        let proceed = match self.choice {
            DiscardChoice::Stay | DiscardChoice::Cancel => false,
            DiscardChoice::Discard => true,
            DiscardChoice::Save => !still_dirty,
        };
        DiscardResolution {
            proceed,
            silence_prompt: proceed && self.dont_ask_again,
        }
    }
}

/// Render the modal over the current frame. `file_name` names the document
/// in the prompt (`None` for a never-saved one) and `tail` finishes the
/// sentence for the transition being guarded ("quitting", "closing it", …).
/// Returns the choice the user made this frame plus the "Don't ask again"
/// state.
///
/// The buttons sit at the right in one order on every platform, Save last in the accent, and
/// Save takes the focus as the dialog opens, so Return presses it, as a dialog's default button
/// does.
fn show(ui: &mut Ui, theme: &Theme, file_name: Option<&str>, tail: &str) -> DiscardOutcome {
    let title = match file_name {
        Some(name) => ui.fmt(format_args!("Save changes to “{name}” before {tail}?")),
        None => ui.fmt(format_args!("Save changes before {tail}?")),
    };

    // The checkbox's own id keys its state, so the row lives exactly as long
    // as the dialog is up: once it closes nothing records the checkbox, the
    // row is swept, and the next open starts unchecked.
    let dont_ask_id = dont_ask_wid();
    ui.with_state::<DontAskAgain, _>(dont_ask_id, |ui, DontAskAgain(dont_ask_again)| {
        let mut choice = DiscardChoice::Stay;
        let resp = Modal::new()
            .id_salt(("discard_dialog", "modal"))
            .show(ui, |ui, _| {
                Panel::vstack()
                    .id(body_wid())
                    .max_size((title_measure(theme), f32::INFINITY))
                    .gap(theme.space.lg)
                    .show(ui, |ui| {
                        Text::new(title)
                            .id(title_wid())
                            .style(&support::title_text(ui, theme))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                        Checkbox::new(dont_ask_again)
                            .id(dont_ask_id)
                            .label("Don't ask again")
                            .show(ui);
                        Panel::hstack()
                            .id_salt(("discard_dialog", "row"))
                            .size((Sizing::FILL, Sizing::HUG))
                            .gap(theme.space.sm)
                            .show(ui, |ui| {
                                support::hspacer(ui, "discard_dialog_spacer");
                                if Button::new()
                                    .id(discard_wid())
                                    .label("Don't Save")
                                    .show(ui)
                                    .left
                                    .clicked()
                                {
                                    choice = DiscardChoice::Discard;
                                }
                                if Button::new()
                                    .id(cancel_wid())
                                    .label("Cancel")
                                    .show(ui)
                                    .left
                                    .clicked()
                                {
                                    choice = DiscardChoice::Cancel;
                                }
                                if Button::new()
                                    .id(save_wid())
                                    .label("Save")
                                    .style(&theme.primary_button)
                                    .show(ui)
                                    .left
                                    .clicked()
                                {
                                    choice = DiscardChoice::Save;
                                }
                            });
                    });
            });
        // Save holds the focus as the dialog opens, so Return presses it.
        if resp.opened {
            ui.set_focus(save_wid());
        }
        // Esc / backdrop click dismisses the modal — treat as Cancel.
        if resp.dismissed {
            choice = DiscardChoice::Cancel;
        }
        DiscardOutcome {
            choice,
            dont_ask_again: *dont_ask_again,
        }
    })
}

/// The checkbox's row: a type of its own, so no other row a widget keeps at
/// the checkbox's id can share it.
#[derive(Default, Debug)]
struct DontAskAgain(bool);

/// The title's measure: about 60 characters at the title size, 30 of its ems, inside the 45 to
/// 75 a line of reading text holds (Bringhurst, *The Elements of Typographic Style*, 2.1.2), so
/// a long file name wraps rather than widening the dialog.
fn title_measure(theme: &Theme) -> f32 {
    30.0 * theme.font.title
}

fn dont_ask_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.dont_ask_again")
}

fn body_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.body")
}

fn title_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.title")
}

fn save_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.save")
}

fn discard_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.discard")
}

fn cancel_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.discard_dialog.cancel")
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::gui::app::discard_dialog::DiscardGuard;

    impl DiscardGuard {
        /// Whether the prompt is up, holding a transition.
        pub(crate) const fn is_up(&self) -> bool {
            self.pending.is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::Harness;
    use palantir::Key;

    use super::*;

    /// The checkbox keeps its tick while the dialog is up, and the row goes
    /// with the dialog: once a frame passes without it, the next open starts
    /// unchecked rather than inheriting the last prompt's answer.
    #[test]
    fn the_checkbox_holds_while_the_dialog_is_up_and_resets_once_it_closes() {
        let theme = Theme::default();
        let mut h = Harness::new(UVec2::new(800, 600));
        let open =
            |h: &mut Harness| h.frame_value(|ui| show(ui, &theme, None, "quitting").dont_ask_again);
        h.prime(2, |ui| {
            show(ui, &theme, None, "quitting");
        });
        assert!(!open(&mut h), "a fresh dialog starts unchecked");

        h.click(dont_ask_wid());
        assert!(open(&mut h), "the click ticks the box");
        assert!(open(&mut h), "the tick holds on the next frame");

        h.frame(|ui| {
            assert_eq!(
                ui.state::<DontAskAgain>(dont_ask_wid()).map(|row| row.0),
                Some(true),
                "still held this frame"
            );
        });
        h.frame(|ui| {
            assert_eq!(
                ui.state::<DontAskAgain>(dont_ask_wid()).map(|row| row.0),
                None,
                "a frame without the dialog swept its row"
            );
        });
        assert!(!open(&mut h), "the next open starts unchecked");
    }

    /// The buttons sit at the body's right in one order, Don't Save, Cancel, then Save, which
    /// wears the accent; a long file name wraps the title inside its measure; Return picks Save.
    #[test]
    fn save_ends_the_button_row_in_the_accent_and_return_picks_it() {
        let theme = Theme::default();
        let mut h = Harness::new(UVec2::new(1200, 800));
        let name = "a file name long enough to run well past the title's measure.darkroom";
        let choice =
            |h: &mut Harness| h.frame_value(|ui| show(ui, &theme, Some(name), "quitting").choice);
        assert_eq!(choice(&mut h), DiscardChoice::Stay);
        assert_eq!(choice(&mut h), DiscardChoice::Stay);
        let rect = |wid: WidgetId| h.rect(wid).expect("recorded");
        let [body, title, discard, cancel, save] = [
            body_wid(),
            title_wid(),
            discard_wid(),
            cancel_wid(),
            save_wid(),
        ]
        .map(rect);
        assert!(discard.max().x < cancel.min.x && cancel.max().x < save.min.x);
        assert_eq!(
            save.max().x,
            body.max().x,
            "the row ends at the body's right edge"
        );
        assert!(body.size.w <= title_measure(&theme), "{}", body.size.w);
        let line = theme
            .palantir
            .text
            .with_font_size(theme.font.title)
            .line_height();
        assert!(
            title.size.h >= 2.0 * line,
            "the title wraps: {}",
            title.size.h
        );
        assert_eq!(
            theme.primary_button.looks.normal.chrome.fill,
            theme.colors.accent.into()
        );
        h.key(Key::Enter);
        assert_eq!(choice(&mut h), DiscardChoice::Save);
    }

    /// A document Finder hands over is guarded exactly like one the user
    /// picked: same question, same wording. The two differ only in where the
    /// path came from, and the prompt has no business exposing that.
    #[test]
    fn an_externally_opened_document_prompts_like_a_picked_one() {
        let picked = PendingTransition::OpenPicked;
        let handed = PendingTransition::OpenAt(PathBuf::from("/tmp/scene.darkroom"));
        assert_eq!(picked.prompt_tail(), "opening another document");
        assert_eq!(handed.prompt_tail(), picked.prompt_tail());

        // The other two stay distinct — a shared tail would misdescribe what
        // the user is about to lose the document to.
        assert_eq!(PendingTransition::Quit.prompt_tail(), "quitting");
        assert_eq!(PendingTransition::New.prompt_tail(), "closing it");
    }

    fn answer(choice: DiscardChoice, dont_ask_again: bool) -> DiscardOutcome {
        DiscardOutcome {
            choice,
            dont_ask_again,
        }
    }

    fn resolution(proceed: bool, silence_prompt: bool) -> DiscardResolution {
        DiscardResolution {
            proceed,
            silence_prompt,
        }
    }

    #[test]
    fn only_a_transition_that_goes_through_can_silence_the_prompt() {
        // "Don't ask again" persists only once the transition goes through.
        // Persisted before the save ran, a cancelled Save As would leave the
        // document open and dirty with the guard permanently off — and the
        // next discard would take the work with it, silently.
        assert_eq!(
            answer(DiscardChoice::Save, true).resolve(true),
            resolution(false, false),
            "a cancelled or failed save abandons the transition and keeps the guard"
        );
        assert_eq!(
            answer(DiscardChoice::Save, true).resolve(false),
            resolution(true, true),
            "a save that stuck proceeds and honors the checkbox"
        );

        // Discard never saves, so dirtiness can't change its answer.
        for still_dirty in [true, false] {
            assert_eq!(
                answer(DiscardChoice::Discard, true).resolve(still_dirty),
                resolution(true, true),
                "discard proceeds regardless of dirtiness ({still_dirty})"
            );
        }

        // Neither non-answer proceeds, and neither touches the preference
        // however the box is left.
        for choice in [DiscardChoice::Cancel, DiscardChoice::Stay] {
            for dont_ask_again in [true, false] {
                assert_eq!(
                    answer(choice, dont_ask_again).resolve(true),
                    resolution(false, false),
                    "{choice:?} with dont_ask_again={dont_ask_again} changes nothing"
                );
            }
        }

        // An unchecked box proceeds without disabling anything — the two
        // effects are independent.
        assert_eq!(
            answer(DiscardChoice::Discard, false).resolve(false),
            resolution(true, false)
        );
    }
}
