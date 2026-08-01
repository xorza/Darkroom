//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! (`gui::pane::graph::node::header`), which maps the returned
//! [`RenameEvent`] onto a `RenameNode` intent. Mirrors the per-widget split of
//! `gui::pane::graph::node::value_editor`; both share the blur-edge /
//! buffered-text core in [`crate::gui::widgets::buffered_edit`].

use palantir::{
    Align, Configure, HAlign, InternedStr, Justify, Panel, Sense, Sizing, Spacing, Text, TextEdit,
    TextEditTheme, TextStyle, Ui, VAlign, WidgetId,
};

use crate::gui::theme::InlineRenameTheme;
use crate::gui::widgets::buffered_edit::EditBuffer;

/// Cross-frame state for one inline-rename editor, held in palantir's
/// `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone, Debug)]
struct RenameState {
    active: bool,
    /// The in-progress draft plus blur-edge tracking, shared with
    /// `gui::pane::graph::node::value_editor`'s buffered fields — see
    /// [`EditBuffer`] for why the latch needs to survive the
    /// `request_focus` → focus-landing gap this widget opens.
    edit: EditBuffer,
}

/// What one frame of [`InlineRename`] surfaced. `clicked` (idle label
/// clicked, including the double-click frame) and `committed` (a changed
/// value was accepted) never co-occur — the first only fires while idle,
/// the second only while editing — but a single struct keeps the caller's
/// match flat.
#[derive(Debug)]
pub(crate) struct RenameEvent {
    pub(crate) clicked: bool,
    pub(crate) committed: Option<String>,
}

/// Minimum width of both the idle label and the editor, so a short name
/// still presents an easy double-click target and the field doesn't
/// collapse to a caret sliver when the draft is emptied.
const MIN_EDIT_WIDTH: f32 = 40.0;

/// Default character cap. Caller's `.max_chars(n)` overrides.
const DEFAULT_MAX_CHARS: usize = 64;

/// Inline-renamable label builder. Idle = click-sensing `Text`;
/// double-click swaps in a `max_chars`-capped `TextEdit` that hugs its
/// text width (grows as you type). Enter or blur commits, Esc cancels.
/// The same `id` is recorded every frame across the label⇄editor swap so
/// palantir keeps the state row alive — pick something stable per
/// underlying domain item (node id, port id, graph id, …).
///
#[derive(Debug)]
pub(crate) struct InlineRename<'a> {
    id: WidgetId,
    name: InternedStr,
    theme: &'a InlineRenameTheme,
    max_chars: usize,
    style: Option<&'a TextStyle>,
    halign: HAlign,
}

impl<'a> InlineRename<'a> {
    pub(crate) fn new(id: WidgetId, name: InternedStr, theme: &'a InlineRenameTheme) -> Self {
        Self {
            id,
            theme,
            name,
            max_chars: DEFAULT_MAX_CHARS,
            style: None,
            halign: HAlign::Left,
        }
    }

    /// Override the character cap applied to the active `TextEdit`.
    pub(crate) fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = n;
        self
    }

    /// Override the text style of both the idle label and the active
    /// editor (font size / colour / family). Defaults to ambient
    /// `ui.theme.text`.
    pub(crate) fn style(mut self, style: &'a TextStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> RenameEvent {
        let Self {
            id,
            name,
            theme,
            max_chars,
            style,
            halign,
        } = self;
        // The label sits inside a `MIN_EDIT_WIDTH` panel so short names
        // still present a clickable target; the parent's main-axis
        // distribution (`justify`) decides which side the text hugs.
        let justify = match halign {
            HAlign::Right => Justify::End,
            HAlign::Center => Justify::Center,
            _ => Justify::Start,
        };
        // Pin both axes explicitly — TextEdit's single-line default
        // (`Align::LEFT` = HAlign::Left + VAlign::Center) is sticky in
        // edit mode, but we also need vertical centering in idle so
        // the swap doesn't snap glyphs vertically.
        let text_align = Align::new(halign, VAlign::Center);
        // Floor the height at one text line so an empty label still has
        // a clickable box (a `Hug` panel with no text would collapse to
        // zero height). Derived from the (possibly overridden) text
        // style so overriding the font size also tightens the click
        // target.
        let style_for_metrics = style.unwrap_or(&ui.theme.text);
        let line_h = style_for_metrics.line_height_for(style_for_metrics.font_size_px);
        // Resolve the editor theme up front so the idle path can
        // mirror the active TextEdit's trailing caret-room — without
        // this, the panel grows by `caret_width` (and right-aligned
        // glyphs shift left by the same amount) on the swap to edit
        // mode, twitching the label one or two pixels.
        let caret_room = theme.text_edit.caret_width.max(0.0);
        // TextEdit's Hug single-line floor sets `min_size.w = text +
        // padding_horiz + 2 * caret_room` (see palantir
        // `text_edit/mod.rs::show`), reserving caret slack on *both*
        // sides so the end-of-line caret never clips on horizontal
        // scroll. We mirror the same total width on the idle Panel,
        // but the side that holds the slack has to match where
        // TextEdit's `align_offset` actually places the glyphs — i.e.
        // *opposite* the text's leading edge:
        //   - Left  halign: TE puts text flush at `TE.left + 0`, with
        //     2·caret_room slack on the right → idle: padding on right.
        //   - Right halign: TE puts text flush at `TE.right - caret_room`,
        //     so slack is split caret_room/caret_room → idle: symmetric.
        // Same total width either way, so the surrounding row doesn't
        // reshape; the glyph baseline stays put across the swap.
        let idle_padding = match halign {
            HAlign::Right | HAlign::Center => Spacing::xy(caret_room, 0.0),
            _ => Spacing::new(0.0, 0.0, 2.0 * caret_room, 0.0),
        };
        if !ui.state_mut::<RenameState>(id).active {
            // `DRAG` as well as `CLICK`: the label captures the press
            // (so it can register clicks / double-click-to-edit), but
            // a press that turns into a drag must still be available
            // to an ancestor that uses the label as a move handle —
            // e.g. the node header dragging its node. Without `DRAG`
            // the press latches as a click-only capture and the drag
            // is swallowed. The active editor is a `TextEdit` (no
            // `DRAG`), so this only applies while idle.
            let resp = Panel::hstack()
                .id(id)
                .size((Sizing::HUG, Sizing::HUG))
                .min_size((MIN_EDIT_WIDTH, line_h))
                .padding(idle_padding)
                .justify(justify)
                // Match TextEdit's single-line vertical centering so
                // the swap to edit mode doesn't shift the glyph row.
                .child_align(Align::v(VAlign::Center))
                .sense(Sense::CLICK | Sense::DRAG)
                .show(ui, |ui| {
                    let mut t = Text::new(name.clone());
                    if let Some(s) = style {
                        t = t.style(s);
                    }
                    t.show(ui);
                })
                .response;
            let clicked = resp.left.clicked();
            let double_clicked = resp.left.double_clicked();
            if double_clicked {
                let st = ui.state_mut::<RenameState>(id);
                st.active = true;
                st.edit.reset_latch();
                st.edit.text = name.borrow_str().to_owned();
                ui.request_focus(Some(id));
            }
            return RenameEvent {
                clicked,
                committed: None,
            };
        }

        let mut draft = std::mem::take(&mut ui.state_mut::<RenameState>(id).edit.text);
        let edit_style = edit_style(theme, style);
        // Both signals come off the editor, not off `ui`. A focused
        // `TextEdit` declares a `TEXT_FIELD` scope, which takes Enter
        // (`KeyClass::Text`) and Escape (`KeyClass::Escape`) — so polling
        // them here would see nothing, and the widget that consumed them
        // is the one that can report them anyway.
        let (submitted, cancelled) = {
            let edit = TextEdit::new(&mut draft)
                .id(id)
                .style(&edit_style)
                .max_chars(max_chars)
                // Renaming replaces a name far more often than it edits
                // one, so the draft arrives selected and the first
                // keystroke wipes it. Safe against the double-click that
                // opened the session: `double_clicked` fires on the
                // second *release*, so the button is already up by the
                // frame the editor first records — and the select-all is
                // gated on no press being held, which is what keeps a
                // click *into* an open editor placing the caret instead.
                .select_all_on_focus()
                // Renaming replaces a name far more often than it edits
                // one, so the draft arrives selected and the first
                // keystroke wipes it. Safe against the double-click that
                // opened the session: `double_clicked` fires on the
                // second *release*, so the button is already up by the
                // frame the editor first records — and the select-all is
                // gated on no press being held, which is what keeps a
                // click *into* an open editor placing the caret instead.
                .size((Sizing::HUG, Sizing::HUG))
                .min_size((MIN_EDIT_WIDTH, line_h))
                .text_align(text_align)
                .show(ui);
            (edit.submitted, edit.cancelled)
        };
        let focused = ui.focused_id() == Some(id);
        let commit = {
            let st = ui.state_mut::<RenameState>(id);
            st.edit.text = draft.clone();
            let blurred = st.edit.blur_edge(focused);
            // Commit on Enter or on blur; Esc wins as a cancel. Escape
            // blurs too, so `cancelled` has to be tested first.
            !cancelled && (submitted || blurred)
        };
        if !(commit || cancelled) {
            return RenameEvent {
                clicked: false,
                committed: None,
            };
        }
        let st = ui.state_mut::<RenameState>(id);
        st.active = false;
        st.edit.reset_latch();
        ui.request_focus(None);
        RenameEvent {
            clicked: false,
            committed: (commit && draft.as_str() != &*name.borrow_str()).then_some(draft),
        }
    }
}

/// Build the active editor's theme: start from the bundle's flat
/// `text_edit` (no padding/margin/border, transparent fill) and, when
/// a label style is supplied, clone it into every WidgetLook's text
/// slot so the field renders at the same font as the idle label.
fn edit_style(theme: &InlineRenameTheme, text: Option<&TextStyle>) -> TextEditTheme {
    let mut style = theme.text_edit.clone();
    if let Some(text) = text {
        for look in [
            &mut style.looks.normal,
            &mut style.looks.hovered,
            &mut style.looks.active,
            &mut style.looks.disabled,
        ] {
            look.text = Some(text.clone());
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::theme::Theme;
    use glam::UVec2;
    use palantir::Key;
    use palantir::internals::UiHarness;

    /// Entering rename selects the whole draft, so the first keystroke
    /// replaces the name instead of appending to it.
    ///
    /// Asserted through the committed value rather than the editor's
    /// selection range — that's the behaviour the caller sees, and the
    /// range is palantir-internal anyway. Without `select_all_on_focus`
    /// the caret sits where the double-click landed and this commits
    /// some splice of the old name and the new character.
    #[test]
    fn entering_edit_mode_selects_the_whole_name() {
        let theme = Theme::default();
        let id = WidgetId::from_hash("rename-select-all");
        let mut h = UiHarness::new(UVec2::new(300, 100));

        fn render(ui: &mut Ui, id: WidgetId, theme: &Theme) -> RenameEvent {
            let name = ui.intern("Alpha");
            InlineRename::new(id, name, &theme.inline_rename).show(ui)
        }

        // Lay the label out, then double-click it to open the editor.
        h.frame(|ui| {
            render(ui, id, &theme);
        });
        let hit = h.rect(id).expect("label arranged").center();
        h.click_at(hit);
        h.frame(|ui| {
            render(ui, id, &theme);
        });
        h.click_at(hit);
        h.frame(|ui| {
            render(ui, id, &theme);
        });

        // The editor's first frame: focus lands and the draft selects.
        h.frame(|ui| {
            render(ui, id, &theme);
        });

        // One character replaces the selection outright.
        h.key(Key::Char('X'));
        h.frame(|ui| {
            render(ui, id, &theme);
        });

        h.key(Key::Enter);
        let committed = h.frame_value(|ui| render(ui, id, &theme).committed);
        assert_eq!(
            committed.as_deref(),
            Some("X"),
            "the first keystroke must replace the whole name, not splice into it",
        );
    }
}
