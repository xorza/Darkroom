//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! (`gui::pane::graph::node::header`), which maps the returned
//! [`RenameEvent`] onto a `RenameNode` intent. Mirrors the per-widget split of
//! `gui::pane::graph::node::value_editor`; both share the blur-edge /
//! buffered-text core in [`crate::gui::widgets::buffered_edit`].

use palantir::{
    Align, Configure, HAlign, Justify, Panel, Sense, Sizing, Spacing, Text, TextEdit, Ui, VAlign,
    WidgetId,
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
///
/// Shaped like a palantir widget: the name is the only positional
/// argument, and identity ([`Self::id`]) and look ([`Self::style`]) are
/// optional overrides over a call-site id and the ambient theme.
#[derive(Debug)]
pub(crate) struct InlineRename<'a> {
    id: WidgetId,
    name: &'a str,
    style: Option<&'a InlineRenameTheme>,
    max_chars: usize,
    halign: HAlign,
}

impl<'a> InlineRename<'a> {
    /// A rename label for `name`, identified by its call site.
    ///
    /// Unlike a palantir widget's auto id, this one is *not* scoped to
    /// the enclosing node and *cannot* be disambiguated by occurrence:
    /// the widget reads its own state row before it opens a node, to
    /// decide whether to record a label or an editor at all, so there is
    /// no resolved parent id to mix in yet. Two rename labels built from
    /// one call site therefore share a draft — set [`Self::id`] from the
    /// domain item whenever this is built in a loop.
    #[track_caller]
    pub(crate) fn new(name: &'a str) -> Self {
        Self {
            id: WidgetId::auto_stable(),
            name,
            style: None,
            max_chars: DEFAULT_MAX_CHARS,
            halign: HAlign::Left,
        }
    }

    /// Use `id` verbatim instead of the call-site default. The label and
    /// the editor both record under it, which is what keeps the draft
    /// alive across the swap — so it has to be stable per underlying
    /// domain item (node id, port id, graph id, …).
    pub(crate) fn id(mut self, id: WidgetId) -> Self {
        self.id = id;
        self
    }

    /// Borrow a whole inline-rename theme override — all-or-nothing.
    /// `None` flattens ambient [`palantir::Theme::text_edit`] the same
    /// way the darkroom theme's own slot is built, so an unstyled rename
    /// still matches whatever text-edit palette is installed.
    ///
    /// Font, colour and leading ride along inside the bundle's per-state
    /// `text` slots, as they do for every palantir widget — to bold a
    /// title, hand over a bundle built with
    /// [`InlineRenameTheme::with_text`] rather than restyling here. A
    /// slot left `None` inherits ambient `palantir::Theme::text`.
    pub(crate) fn style(mut self, style: &'a InlineRenameTheme) -> Self {
        self.style = Some(style);
        self
    }

    /// Override the character cap applied to the active `TextEdit`.
    pub(crate) fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = n;
        self
    }

    /// Which edge the name hugs, in both the idle label and the active
    /// editor. Defaults to [`HAlign::Left`].
    ///
    /// The idle label mirrors `TextEdit`'s caret-room reservation on
    /// whichever side the glyphs *aren't* flush against, so the text
    /// doesn't shift by a pixel or two on the swap into edit mode — see
    /// the `idle_padding` derivation in [`Self::show`].
    ///
    /// `allow` rather than removal: `show` has always carried the
    /// non-`Left` geometry, and the trailing-edge case is what a
    /// right-aligned output-port rename needs. The unit test is the only
    /// caller until one exists.
    #[allow(dead_code)]
    pub(crate) fn halign(mut self, halign: HAlign) -> Self {
        self.halign = halign;
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> RenameEvent {
        let Self {
            id,
            name,
            style,
            max_chars,
            halign,
        } = self;
        // Bound outside the `match` so the flattened fallback outlives
        // the borrow, and built only when the caller supplied no bundle
        // — the styled path costs nothing for having this here.
        let ambient;
        let theme = match style {
            Some(theme) => theme,
            None => {
                ambient = InlineRenameTheme::flattened(&ui.theme().text_edit);
                &ambient
            }
        };
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
        // The label's text style: the bundle's resting slot, or ambient
        // when it declines to pin one — `TextEdit` resolves its own the
        // same way, so the two agree across the swap by construction.
        let text = theme.text_edit.looks.normal.text.as_ref();
        // Floor the height at one text line so an empty label still has
        // a clickable box (a `Hug` panel with no text would collapse to
        // zero height). Derived from the resolved text style so a bundle
        // that pins a font size also tightens the click target.
        let style_for_metrics = text.unwrap_or(&ui.theme().text);
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
                    // Derived from the widget's own id rather than left
                    // to a call-site auto id: the node header draws every
                    // node's title from one call site, so an auto id
                    // would separate two titles only by record order.
                    let mut t = Text::new(name).id(label_wid(id));
                    if let Some(s) = text {
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
                st.edit.text = name.to_owned();
                ui.request_focus(Some(id));
            }
            return RenameEvent {
                clicked,
                committed: None,
            };
        }

        let mut draft = std::mem::take(&mut ui.state_mut::<RenameState>(id).edit.text);
        // Both signals come off the editor, not off `ui`. A focused
        // `TextEdit` declares a `TEXT_FIELD` scope, which takes Enter
        // (`KeyClass::Text`) and Escape (`KeyClass::Escape`) — so polling
        // them here would see nothing, and the widget that consumed them
        // is the one that can report them anyway.
        let (submitted, cancelled) = {
            let edit = TextEdit::new(&mut draft)
                .id(id)
                .style(&theme.text_edit)
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
            committed: (commit && draft.as_str() != name).then_some(draft),
        }
    }
}

/// Id of the idle label inside the rename widget `id`, derived from its
/// parent so a title keeps its identity when the node's paint order
/// changes. Only the idle path opens it — in edit mode the `TextEdit`
/// takes over and records under `id` itself.
fn label_wid(id: WidgetId) -> WidgetId {
    id.with("inline_rename.label")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::theme::Theme;
    use glam::UVec2;
    use palantir::Key;
    use palantir::internals::UiHarness;

    /// `halign` puts the glyphs against the requested edge, and every
    /// setting leaves the label the same width.
    ///
    /// The width half is the anti-twitch contract `idle_padding` exists
    /// for: the idle label reserves `2 * caret_width` of horizontal slack
    /// either way, split to match where `TextEdit` will actually put the
    /// glyphs, so opening the editor can't reflow the row or slide the
    /// text sideways. Asserted on the panel⇄label pair rather than by eye
    /// because the offsets are 1.5 px.
    ///
    /// `pixel_snap(false)` keeps the arithmetic exact — snapping would
    /// round the 1.5 px caret slack to a whole pixel and hide which side
    /// it landed on. Text width never enters the math (only the distance
    /// from the label's edge to the panel's does), so the mono fallback
    /// `UiHarness::new` gives is fine.
    #[test]
    fn halign_places_the_name_without_changing_the_labels_width() {
        let theme = Theme::default();
        // Padding is derived from this, so a zero would make all three
        // cases identical and the assertions vacuous.
        let caret = theme.inline_rename.text_edit.caret_width;
        assert_eq!(caret, 1.5, "the padding math below is written for 1.5");

        // Left  → padding (0, 0, 2*caret, 0), Justify::Start  → flush left.
        // Right → padding (caret, 0, caret, 0), Justify::End  → right edge
        //         sits `caret` inside the panel.
        // Center→ padding (caret, 0, caret, 0), Justify::Center → centered,
        //         since the padding is symmetric.
        let mut widths = Vec::new();
        for (i, halign) in [HAlign::Left, HAlign::Right, HAlign::Center]
            .into_iter()
            .enumerate()
        {
            let id = WidgetId::from_hash(("rename-halign", i));
            let mut h = UiHarness::new(UVec2::new(300, 100)).pixel_snap(false);
            h.frame(|ui| {
                InlineRename::new("Ab")
                    .id(id)
                    .style(&theme.inline_rename)
                    .halign(halign)
                    .show(ui);
            });

            let panel = h.rect(id).expect("label panel arranged");
            let label = h.rect(label_wid(id)).expect("name text arranged");
            match halign {
                HAlign::Left => assert_eq!(
                    label.min.x, panel.min.x,
                    "left-aligned name starts at the panel's leading edge",
                ),
                HAlign::Right => assert_eq!(
                    label.max().x,
                    panel.max().x - caret,
                    "right-aligned name stops one caret-width short of the \
                     trailing edge, where TextEdit will draw it",
                ),
                HAlign::Center => assert_eq!(
                    label.center().x,
                    panel.center().x,
                    "centred name sits on the panel's midline",
                ),
                _ => unreachable!(),
            }
            widths.push(panel.size.w);
        }

        // The three cases differ in *where* the text sits, never in how
        // much room the label takes — that is what keeps a node header
        // from reflowing when its title's alignment changes.
        assert_eq!(
            widths,
            vec![widths[0]; 3],
            "every alignment reserves the same 2 * caret_width of slack",
        );
    }

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
            InlineRename::new("Alpha")
                .id(id)
                .style(&theme.inline_rename)
                .show(ui)
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
