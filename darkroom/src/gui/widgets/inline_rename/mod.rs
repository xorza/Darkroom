//! Reusable inline-rename label. Renders plain text that swaps to a `TextEdit` hugging its text
//! on double-click; Enter or focus loss commits the edited string, Esc cancels. The node header's
//! title is its caller, which maps the returned [`RenameEvent`] onto a `RenameNode` intent.

use palantir::{
    Align, Configure, EditEnd, HAlign, Panel, Sense, Sizing, Spacing, Text, TextEdit,
    TextEditTheme, Ui, VAlign, WidgetId,
};

/// Cross-frame state for one inline-rename editor, held in palantir's
/// `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone, Debug)]
struct RenameState {
    active: bool,
    /// The in-progress draft. Commit and cancel come from the editor's own
    /// `committed` and `canceled` edges, which only report a blur once focus
    /// has landed, so the `set_focus` → focus-landing gap this widget opens
    /// never reads as one.
    draft: String,
}

/// What one frame of [`InlineRename`] surfaced: the idle label clicked, the double-click frame
/// included, or a changed name accepted. Never both: a click fires only while idle, a commit
/// only while editing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RenameEvent {
    Idle,
    Clicked,
    Committed(String),
}

/// Minimum width of both the idle label and the editor, so a short name
/// still presents an easy double-click target and the field doesn't
/// collapse to a caret sliver when the draft is emptied.
const MIN_EDIT_WIDTH: f32 = 40.0;

/// Inline-renamable label builder. Idle = click-sensing `Text`;
/// double-click swaps in a `TextEdit` that hugs its text width (grows as you type), capped as
/// [`Self::max_chars`] says. Enter or blur commits, Esc cancels.
#[derive(Debug)]
pub(crate) struct InlineRename<'a> {
    wid: WidgetId,
    name: &'a str,
    style: &'a TextEditTheme,
    /// The editor's character cap; none, as a `TextEdit` has, unless set.
    max_chars: Option<usize>,
    editing: Editing,
}

/// Whether a double-click opens the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Editing {
    Allowed,
    /// The label still clicks and drags, but never becomes an editor: for a
    /// name no edit can change.
    Refused,
}

impl<'a> InlineRename<'a> {
    /// A rename label for `name` in the look `style`, under `wid`. The label and the editor both
    /// record under it, which is what keeps the draft alive across the swap — so it has to be
    /// stable per domain item (node id, port id, …).
    ///
    /// Font, color and leading ride along inside the bundle's per-state `text` slots, as they do
    /// for every palantir widget: a bold title hands over the theme's `flat_text_edit.bold`. An
    /// axis a slot leaves unset inherits ambient `palantir::Theme::text`.
    pub(crate) const fn new(wid: WidgetId, name: &'a str, style: &'a TextEditTheme) -> Self {
        Self {
            wid,
            name,
            style,
            max_chars: None,
            editing: Editing::Allowed,
        }
    }

    /// Show the label without the editor: a double-click opens nothing.
    pub(crate) const fn read_only(mut self) -> Self {
        self.editing = Editing::Refused;
        self
    }

    /// Cap the editor at `n` characters.
    pub(crate) const fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = Some(n);
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> RenameEvent {
        let Self {
            wid,
            name,
            style: theme,
            max_chars,
            editing,
        } = self;
        // The label sits inside a `MIN_EDIT_WIDTH` panel so short names
        // still present a clickable target, with the text flush left. Both
        // axes are pinned — TextEdit's single-line default (`Align::LEFT`)
        // is sticky in edit mode, but idle needs vertical centering too so
        // the swap doesn't snap glyphs vertically.
        let text_align = Align::new(HAlign::Left, VAlign::Center);
        // The label's text style: the bundle's resting slot over ambient —
        // `TextEdit` resolves its own the same way, so the two agree across
        // the swap by construction.
        let text = theme.looks.normal.text.apply(&ui.theme().text);
        // Floor the height at one text line so an empty label still has
        // a clickable box (a `Hug` panel with no text would collapse to
        // zero height). Derived from the resolved text style so a bundle
        // that pins a font size also tightens the click target.
        let line_h = text.line_height_for(text.font_size);
        // Resolve the editor theme up front so the idle path can
        // mirror the active TextEdit's trailing caret-room — without
        // this, the panel grows by `caret_width` (and right-aligned
        // glyphs shift left by the same amount) on the swap to edit
        // mode, twitching the label one or two pixels.
        let caret_room = theme.caret_width.max(0.0);
        // TextEdit's Hug single-line floor sets `min_size.w = text +
        // padding_horiz + 2 * caret_room` (see palantir
        // `text_edit/mod.rs::show`) and puts left-aligned text flush at its
        // left edge, so all of that slack sits on the right. The idle panel
        // reserves the same, on the same side, so the row keeps its width and
        // the glyphs stay put across the swap.
        let idle_padding = Spacing::new(0.0, 0.0, 2.0 * caret_room, 0.0);
        ui.with_state::<RenameState, _>(wid, |ui, st| {
            if !st.active {
                // `DRAG` as well as `CLICK`: the label captures the press
                // (so it can register clicks / double-click-to-edit), but
                // a press that turns into a drag must still be available
                // to an ancestor that uses the label as a move handle —
                // e.g. the node header dragging its node. Without `DRAG`
                // the press latches as a click-only capture and the drag
                // is swallowed. The active editor is a `TextEdit` (no
                // `DRAG`), so this only applies while idle.
                let resp = Panel::hstack()
                    .id(wid)
                    .size((Sizing::HUG, Sizing::HUG))
                    .min_size((MIN_EDIT_WIDTH, line_h))
                    .padding(idle_padding)
                    // Match TextEdit's single-line vertical centering so
                    // the swap to edit mode doesn't shift the glyph row.
                    .child_align(Align::v(VAlign::Center))
                    .sense(Sense::CLICK | Sense::DRAG)
                    .show(ui, |ui| {
                        // Derived from the widget's own id rather than left
                        // to a call-site auto id: the node header draws every
                        // node's title from one call site, so an auto id
                        // would separate two titles only by record order.
                        Text::new(name).id(label_wid(wid)).style(&text).show(ui);
                    })
                    .response;
                let clicked = resp.left.clicked();
                if resp.left.double_clicked() && editing == Editing::Allowed {
                    st.active = true;
                    // Refilled, not replaced — the row's buffer keeps whatever
                    // capacity the last rename grew it to.
                    st.draft.clear();
                    st.draft.push_str(name);
                    ui.set_focus(wid);
                }
                return if clicked {
                    RenameEvent::Clicked
                } else {
                    RenameEvent::Idle
                };
            }

            // Both signals come off the editor, not off `ui`. A focused
            // `TextEdit` declares a `TEXT_FIELD` scope, which takes Enter
            // (`KeyClass::Text`) and Escape (`KeyClass::Escape`) — so polling
            // them here would see nothing, and the widget that consumed them
            // is the one that can report them anyway.
            let mut edit = TextEdit::new(&mut st.draft).id(wid).style(theme);
            if let Some(max_chars) = max_chars {
                edit = edit.max_chars(max_chars);
            }
            let edit = edit
                // Renaming replaces a name far more often than it edits
                // one, so the draft arrives selected and the first
                // keystroke wipes it. Safe against the double-click that
                // opened the session: `double_clicked` fires on the
                // second *release*, so the button is already up by the
                // frame the editor first records — and the select-all is
                // gated on no press being held, which is what keeps a
                // click *into* an open editor placing the caret instead.
                .select_all_on_focus(true)
                .size((Sizing::HUG, Sizing::HUG))
                .min_size((MIN_EDIT_WIDTH, line_h))
                .text_align(text_align)
                .show(ui);
            if !edit.committed && edit.ended != Some(EditEnd::Canceled) {
                return RenameEvent::Idle;
            }
            // Only a committing frame needs the draft as a value of its own;
            // every other one leaves it in the row, so an open rename copies its
            // text once per commit rather than once per frame.
            let committed = edit.committed && st.draft != name;
            st.active = false;
            ui.clear_focus();
            if committed {
                RenameEvent::Committed(st.draft.clone())
            } else {
                RenameEvent::Idle
            }
        })
    }
}

/// Id of the idle label inside the rename widget `wid`, derived from its
/// parent so a title keeps its identity when the node's paint order
/// changes. Only the idle path opens it — in edit mode the `TextEdit`
/// takes over and records under `wid` itself.
fn label_wid(wid: WidgetId) -> WidgetId {
    wid.with("inline_rename.label")
}

#[cfg(test)]
mod tests;
