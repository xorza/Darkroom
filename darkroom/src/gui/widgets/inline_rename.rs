//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! ([`crate::gui::node::header`]) and the subgraph boundary-port names
//! ([`crate::gui::node::port_rename`]); each maps the returned
//! [`RenameEvent`] onto its own intent. Mirrors the per-widget split of
//! [`crate::gui::node::value_editor`].

use palantir::{
    Brush, Configure, InternedStr, Key, Panel, Sense, Shortcut, Sizing, Spacing, Stroke, Text,
    TextEdit, TextEditTheme, TextStyle, Ui, WidgetId,
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
    pub(crate) clicked: bool,
    pub(crate) committed: Option<String>,
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
    style: Option<TextStyle>,
) -> RenameEvent {
    // Floor the height at one text line so an empty label still has a
    // clickable box (a `Hug` panel with no text would collapse to zero
    // height). Derived from the (possibly overridden) text style so
    // overriding the font size also tightens the click target.
    let style_for_metrics = style.unwrap_or(ui.theme.text);
    let line_h = style_for_metrics.line_height_for(style_for_metrics.font_size_px);
    if !ui.state_mut::<RenameState>(id).active {
        // `DRAG` as well as `CLICK`: the label captures the press (so it
        // can register clicks / double-click-to-edit), but a press that
        // turns into a drag must still be available to an ancestor that
        // uses the label as a move handle — e.g. the node header dragging
        // its node. Without `DRAG` the press latches as a click-only
        // capture and the drag is swallowed. The active editor is a
        // `TextEdit` (no `DRAG`), so this only applies while idle.
        let resp = Panel::hstack()
            .id(id)
            .size((Sizing::Hug, Sizing::Hug))
            .min_size((MIN_EDIT_WIDTH, line_h))
            .sense(Sense::CLICK | Sense::DRAG)
            .show(ui, |ui| {
                // `InternedStr::clone` is allocation-free for the `Owned`
                // names darkroom builds, so this is cheap per frame.
                let mut t = Text::new(name.clone());
                if let Some(s) = style {
                    t = t.style(s);
                }
                t.show(ui);
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
        .style(flat_edit_style(ui, style))
        .max_chars(max_chars)
        .size((Sizing::Hug, Sizing::Hug))
        .min_size((MIN_EDIT_WIDTH, line_h))
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
/// fixed-width field still reads as editable. When `text` is `Some`,
/// the same style is applied to every WidgetLook so the edit field
/// renders at the same size/colour as the idle label.
fn flat_edit_style(ui: &Ui, text: Option<TextStyle>) -> TextEditTheme {
    let mut style = ui.theme.text_edit.clone();
    style.padding = Spacing::ZERO;
    style.margin = Spacing::ZERO;
    for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
        if let Some(bg) = look.background.as_mut() {
            bg.stroke = Stroke::ZERO;
            bg.fill = Brush::TRANSPARENT;
        }
        if text.is_some() {
            look.text = text;
        }
    }
    style
}
