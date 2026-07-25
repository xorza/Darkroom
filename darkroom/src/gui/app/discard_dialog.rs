//! The unsaved-changes confirmation dialog. Rendered by
//! [`App::record_discard_prompt`](crate::gui::app::App::record_discard_prompt)
//! whenever a transition that would replace or discard the open document is
//! requested while it has edits; the returned [`DiscardOutcome`] is applied
//! immediately after the dialog finishes authoring.

use aperture::{Button, Checkbox, Configure, Modal, Panel, Text, Ui, WidgetId};

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
    pub(super) fn resolve(self, still_dirty: bool) -> DiscardResolution {
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
pub(super) fn show(ui: &mut Ui, file_name: Option<&str>, tail: &str) -> DiscardOutcome {
    let title = match file_name {
        Some(name) => ui.fmt(format_args!("Save changes to “{name}” before {tail}?")),
        None => ui.fmt(format_args!("Save changes before {tail}?")),
    };

    // Checkbox state lives across the frames the dialog is up; the id isn't
    // recorded once the dialog closes, so the row is swept and the next
    // open starts unchecked.
    let dont_ask_id = WidgetId::from_hash("discard_dialog::dont_ask_again");
    let mut dont_ask_again = *ui.state_mut::<bool>(dont_ask_id);

    let mut choice = DiscardChoice::Stay;
    let resp = Modal::new()
        .id_salt(("discard_dialog", "modal"))
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt(("discard_dialog", "body"))
                .gap(16.0)
                .padding(8.0)
                .show(ui, |ui| {
                    Text::new(title)
                        .id_salt(("discard_dialog", "title"))
                        .show(ui);
                    Checkbox::new(&mut dont_ask_again)
                        .id_salt(("discard_dialog", "dont_ask"))
                        .label("Don't ask again")
                        .show(ui);
                    Panel::hstack()
                        .id_salt(("discard_dialog", "row"))
                        .gap(8.0)
                        .show(ui, |ui| {
                            if Button::new()
                                .id_salt(("discard_dialog", "save"))
                                .label("Save")
                                .show(ui)
                                .left
                                .clicked()
                            {
                                choice = DiscardChoice::Save;
                            }
                            if Button::new()
                                .id_salt(("discard_dialog", "discard"))
                                .label("Don't Save")
                                .show(ui)
                                .left
                                .clicked()
                            {
                                choice = DiscardChoice::Discard;
                            }
                            if Button::new()
                                .id_salt(("discard_dialog", "cancel"))
                                .label("Cancel")
                                .show(ui)
                                .left
                                .clicked()
                            {
                                choice = DiscardChoice::Cancel;
                            }
                        });
                });
        });
    // Esc / backdrop click dismisses the modal — treat as Cancel.
    if resp.dismissed {
        choice = DiscardChoice::Cancel;
    }

    *ui.state_mut::<bool>(dont_ask_id) = dont_ask_again;
    DiscardOutcome {
        choice,
        dont_ask_again,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The regression this pins: "Don't ask again" used to persist
        // *before* the save ran, so cancelling a Save As left the document
        // open and dirty with the guard permanently off — and the next
        // discard took the work with it, silently.
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
