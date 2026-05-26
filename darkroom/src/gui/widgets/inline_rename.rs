//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! ([`crate::gui::node::header`]) and the subgraph boundary-port names
//! ([`crate::gui::node::port_rename`]); each maps the returned
//! [`RenameEvent`] onto its own intent. Mirrors the per-widget split of
//! [`crate::gui::node::value_editor`].

use palantir::{
    Configure, InternedStr, Key, Panel, Sense, Shortcut, Sizing, Spacing, Stroke, Text, TextEdit,
    TextEditTheme, Ui, WidgetId,
};

/// Cross-frame state for one inline-rename editor, held in palantir's
/// `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone)]
struct RenameState {
    active: bool,
    /// Latches once the editor actually holds focus, so the frames
    /// between `request_focus` and focus landing don't read as a blur
    /// and commit early.
    focused_once: bool,
    draft: String,
}

/// What one frame of [`inline_rename`] surfaced. `clicked` (idle label
/// clicked, including the double-click frame) and `committed` (a changed
/// value was accepted) never co-occur — the first only fires while idle,
/// the second only while editing — but a single struct keeps the caller's
/// match flat.
pub(crate) struct RenameEvent {
    pub clicked: bool,
    pub committed: Option<String>,
}

/// Minimum width of both the idle label and the editor, so a short name
/// still presents an easy double-click target and the field doesn't
/// collapse to a caret sliver when the draft is emptied.
const MIN_EDIT_WIDTH: f32 = 40.0;

/// Draw an inline-renamable label under `id`. While idle it's a
/// click-sensing `Text`; double-click swaps it for a `max_chars`-capped
/// `TextEdit` that hugs its text width (grows as you type). Enter or blur
/// commits (→ `committed` when the value changed), Esc cancels. The same
/// `id` is recorded every frame across the label⇄editor swap so palantir
/// keeps the state row alive.
pub(crate) fn inline_rename(
    ui: &mut Ui,
    id: WidgetId,
    name: InternedStr,
    max_chars: usize,
) -> RenameEvent {
    if !ui.state_mut::<RenameState>(id).active {
        let resp = Panel::hstack()
            .id(id)
            .size((Sizing::Hug, Sizing::Hug))
            .min_size((MIN_EDIT_WIDTH, 0.0))
            .sense(Sense::CLICK)
            .show(ui, |ui| {
                // `InternedStr::clone` is allocation-free for the `Owned`
                // names darkroom builds, so this is cheap per frame.
                Text::new(name.clone()).show(ui);
            })
            .response;
        // Read flags off the response before any `ui.state_mut` — the
        // response borrows `ui`.
        let clicked = resp.clicked();
        let double_clicked = resp.double_clicked();
        if double_clicked {
            let st = ui.state_mut::<RenameState>(id);
            st.active = true;
            st.focused_once = false;
            st.draft = name.as_str("").to_owned();
            ui.request_focus(Some(id));
        }
        return RenameEvent {
            clicked,
            committed: None,
        };
    }

    let mut draft = std::mem::take(&mut ui.state_mut::<RenameState>(id).draft);
    TextEdit::new(&mut draft)
        .id(id)
        .style(flat_edit_style(ui))
        .max_chars(max_chars)
        .size((Sizing::Hug, Sizing::Hug))
        .min_size((MIN_EDIT_WIDTH, 0.0))
        .show(ui);
    let focused = ui.focused_id() == Some(id);
    let escape = ui.escape_pressed();
    let enter = ui.key_pressed(Shortcut::key(Key::Enter));
    let commit = {
        let st = ui.state_mut::<RenameState>(id);
        st.draft = draft.clone();
        st.focused_once |= focused;
        // Commit on Enter or on blur (once focus had landed); Esc wins
        // as a cancel.
        !escape && (enter || (st.focused_once && !focused))
    };
    if !(commit || escape) {
        return RenameEvent {
            clicked: false,
            committed: None,
        };
    }
    let st = ui.state_mut::<RenameState>(id);
    st.active = false;
    st.focused_once = false;
    ui.request_focus(None);
    RenameEvent {
        clicked: false,
        committed: (commit && draft != name.as_str("")).then_some(draft),
    }
}

/// The ambient text-edit theme flattened for an inline rename field:
/// zero padding/margin and no border, so the editor's `Hug` height
/// equals the plain `Text` label's line height — the node body doesn't
/// grow when a label enters/exits edit mode. The fill stays, so the
/// fixed-width field still reads as editable.
fn flat_edit_style(ui: &Ui) -> TextEditTheme {
    let mut style = ui.theme.text_edit.clone();
    style.padding = Spacing::ZERO;
    style.margin = Spacing::ZERO;
    for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
        if let Some(bg) = look.background.as_mut() {
            bg.stroke = Stroke::ZERO;
        }
    }
    style
}
