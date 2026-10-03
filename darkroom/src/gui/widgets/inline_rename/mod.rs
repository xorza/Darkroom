//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! (`gui::pane::graph::node::header`), which maps the returned
//! [`RenameEvent`] onto a `RenameNode` intent. Mirrors the per-widget split of
//! `gui::pane::graph::node::value_editor`; both share the buffered-text core
//! and the commit/cancel rule in [`crate::gui::widgets::edit_buffer`].

use palantir::prelude::*;

use crate::gui::theme::inline_rename_theme::InlineRenameTheme;
use crate::gui::widgets::edit_buffer::{DraftOutcome, EditBuffer};
use std::mem;

/// Cross-frame state for one inline-rename editor, held in palantir's
/// `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone, Debug)]
struct RenameState {
    active: bool,
    /// The in-progress draft. Commit and cancel come from the editor's own
    /// focus edges ([`DraftOutcome::of`]), which only report a blur once
    /// focus has landed, so the `set_focus` → focus-landing gap this widget
    /// opens never reads as one.
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
/// Shaped like a palantir widget: the name and the look are positional, and
/// identity ([`Self::id`]) is an optional override over a call-site id.
#[derive(Debug)]
pub(crate) struct InlineRename<'a> {
    id: WidgetId,
    name: &'a str,
    style: &'a InlineRenameTheme,
    max_chars: usize,
}

impl<'a> InlineRename<'a> {
    /// A rename label for `name` in the look `style`, identified by its call
    /// site.
    ///
    /// Font, colour and leading ride along inside the bundle's per-state
    /// `text` slots, as they do for every palantir widget — to bold a title,
    /// hand over a bundle built with [`InlineRenameTheme::with_text`]. A slot
    /// left `None` inherits ambient `palantir::Theme::text`.
    ///
    /// Unlike a palantir widget's auto id, this one is *not* scoped to
    /// the enclosing node and *cannot* be disambiguated by occurrence:
    /// the widget reads its own state row before it opens a node, to
    /// decide whether to record a label or an editor at all, so there is
    /// no resolved parent id to mix in yet. Two rename labels built from
    /// one call site therefore share a draft — set [`Self::id`] from the
    /// domain item whenever this is built in a loop.
    #[track_caller]
    pub(crate) fn new(name: &'a str, style: &'a InlineRenameTheme) -> Self {
        Self {
            id: WidgetId::auto_stable(),
            name,
            style,
            max_chars: DEFAULT_MAX_CHARS,
        }
    }

    /// Use `id` verbatim instead of the call-site default. The label and
    /// the editor both record under it, which is what keeps the draft
    /// alive across the swap — so it has to be stable per underlying
    /// domain item (node id, port id, graph id, …).
    pub(crate) const fn id(mut self, id: WidgetId) -> Self {
        self.id = id;
        self
    }

    /// Override the character cap applied to the active `TextEdit`.
    pub(crate) const fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = n;
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> RenameEvent {
        let Self {
            id,
            name,
            style: theme,
            max_chars,
        } = self;
        // The label sits inside a `MIN_EDIT_WIDTH` panel so short names
        // still present a clickable target, with the text flush left. Both
        // axes are pinned — TextEdit's single-line default (`Align::LEFT`)
        // is sticky in edit mode, but idle needs vertical centering too so
        // the swap doesn't snap glyphs vertically.
        let text_align = Align::new(HAlign::Left, VAlign::Center);
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
        // `text_edit/mod.rs::show`) and puts left-aligned text flush at its
        // left edge, so all of that slack sits on the right. The idle panel
        // reserves the same, on the same side, so the row keeps its width and
        // the glyphs stay put across the swap.
        let idle_padding = Spacing::new(0.0, 0.0, 2.0 * caret_room, 0.0);
        if !ui.state_or_default::<RenameState>(id).active {
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
                let st = ui.state_or_default::<RenameState>(id);
                st.active = true;
                // Refilled, not replaced — the row's buffer keeps whatever
                // capacity the last rename grew it to.
                st.edit.text.clear();
                st.edit.text.push_str(name);
                ui.set_focus(id);
            }
            return RenameEvent {
                clicked,
                committed: None,
            };
        }

        let mut draft = mem::take(&mut ui.state_or_default::<RenameState>(id).edit.text);
        // Both signals come off the editor, not off `ui`. A focused
        // `TextEdit` declares a `TEXT_FIELD` scope, which takes Enter
        // (`KeyClass::Text`) and Escape (`KeyClass::Escape`) — so polling
        // them here would see nothing, and the widget that consumed them
        // is the one that can report them anyway.
        let outcome = {
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
                .size((Sizing::HUG, Sizing::HUG))
                .min_size((MIN_EDIT_WIDTH, line_h))
                .text_align(text_align)
                .show(ui);
            DraftOutcome::of(&edit)
        };
        let commit = outcome == DraftOutcome::Commit;
        // Only a committing frame needs the draft as a value of its own;
        // every other one hands the buffer straight back, so an open rename
        // copies its text once per commit rather than once per frame.
        let committed = (commit && draft.as_str() != name).then(|| draft.clone());
        ui.state_or_default::<RenameState>(id).edit.text = draft;
        if outcome == DraftOutcome::Editing {
            return RenameEvent {
                clicked: false,
                committed: None,
            };
        }
        let st = ui.state_or_default::<RenameState>(id);
        st.active = false;
        ui.clear_focus();
        RenameEvent {
            clicked: false,
            committed,
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
mod tests;
