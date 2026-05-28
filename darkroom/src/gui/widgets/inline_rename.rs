//! Reusable inline-rename label. Renders plain text that swaps to a
//! fixed-width `TextEdit` on double-click; Enter / focus-loss commits the
//! edited string, Esc cancels. Used by the node-header title
//! ([`crate::gui::node::header`]) and the subgraph boundary-port names
//! ([`crate::gui::node::port_rename`]); each maps the returned
//! [`RenameEvent`] onto its own intent. Mirrors the per-widget split of
//! [`crate::gui::node::value_editor`].

use palantir::{
    Align, Configure, HAlign, InternedStr, Justify, Key, Panel, Sense, Shortcut, Sizing, Text,
    TextEdit, TextEditTheme, TextStyle, Ui, WidgetId,
};

use crate::theme::InlineRenameTheme;

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

/// What one frame of [`InlineRename`] surfaced. `clicked` (idle label
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

/// Default character cap. Caller's `.max_chars(n)` overrides.
const DEFAULT_MAX_CHARS: usize = 64;

/// Inline-renamable label builder. Idle = click-sensing `Text`;
/// double-click swaps in a `max_chars`-capped `TextEdit` that hugs its
/// text width (grows as you type). Enter or blur commits, Esc cancels.
/// The same `id` is recorded every frame across the label⇄editor swap so
/// palantir keeps the state row alive — pick something stable per
/// underlying domain item (node id, port id, subgraph id, …).
///
pub(crate) struct InlineRename<'a> {
    id: WidgetId,
    name: InternedStr,
    /// Borrowed when the caller supplied one via [`Self::theme`];
    /// otherwise `show()` falls back to `InlineRenameTheme::default()`
    /// (the built-in flat editor).
    theme: Option<&'a InlineRenameTheme>,
    max_chars: usize,
    style: Option<TextStyle>,
    halign: HAlign,
}

impl<'a> InlineRename<'a> {
    pub(crate) fn new(id: WidgetId, name: impl Into<InternedStr>) -> Self {
        Self {
            id,
            name: name.into(),
            theme: None,
            max_chars: DEFAULT_MAX_CHARS,
            style: None,
            halign: HAlign::Left,
        }
    }

    /// Borrow a darkroom [`InlineRenameTheme`] for the editor look.
    /// Optional — without it, `show()` uses the type's `Default`.
    pub(crate) fn theme(mut self, theme: &'a InlineRenameTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Override the character cap applied to the active `TextEdit`.
    pub(crate) fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = n;
        self
    }

    /// Override the text style of both the idle label and the active
    /// editor (font size / colour / family). Defaults to ambient
    /// `ui.theme.text`.
    pub(crate) fn style(mut self, style: TextStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Pick which side of the min-width box the text hugs. Default
    /// `HAlign::Left`; pass `HAlign::Right` for labels that live in a
    /// right-aligned column (e.g. boundary input ports).
    pub(crate) fn halign(mut self, halign: HAlign) -> Self {
        self.halign = halign;
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
        let text_align = Align::h(halign);
        // Floor the height at one text line so an empty label still has
        // a clickable box (a `Hug` panel with no text would collapse to
        // zero height). Derived from the (possibly overridden) text
        // style so overriding the font size also tightens the click
        // target.
        let style_for_metrics = style.unwrap_or(ui.theme.text);
        let line_h = style_for_metrics.line_height_for(style_for_metrics.font_size_px);
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
                .size((Sizing::Hug, Sizing::Hug))
                .min_size((MIN_EDIT_WIDTH, line_h))
                .justify(justify)
                .sense(Sense::CLICK | Sense::DRAG)
                .show(ui, |ui| {
                    // `InternedStr::clone` is allocation-free for the
                    // `Owned` names darkroom builds, so this is cheap
                    // per frame.
                    let mut t = Text::new(name.clone());
                    if let Some(s) = style {
                        t = t.style(s);
                    }
                    t.show(ui);
                })
                .response;
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
        // Fall back to the built-in flat theme when the caller didn't
        // hand one in. Owned binding lives until the end of `show` so
        // `theme_ref` stays valid for the duration of the editor draw.
        let owned_default;
        let theme_ref: &InlineRenameTheme = match theme {
            Some(t) => t,
            None => {
                owned_default = InlineRenameTheme::default();
                &owned_default
            }
        };
        TextEdit::new(&mut draft)
            .id(id)
            .style(edit_style(theme_ref, style))
            .max_chars(max_chars)
            .size((Sizing::Hug, Sizing::Hug))
            .min_size((MIN_EDIT_WIDTH, line_h))
            .text_align(text_align)
            .show(ui);
        let focused = ui.focused_id() == Some(id);
        let escape = ui.escape_pressed();
        let enter = ui.key_pressed(Shortcut::key(Key::Enter));
        let commit = {
            let st = ui.state_mut::<RenameState>(id);
            st.draft = draft.clone();
            st.focused_once |= focused;
            // Commit on Enter or on blur (once focus had landed); Esc
            // wins as a cancel.
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
}

/// Build the active editor's theme: start from the bundle's flat
/// `text_edit` (no padding/margin/border, transparent fill) and, when
/// a label style is supplied, copy it into every WidgetLook's text
/// slot so the field renders at the same font as the idle label.
fn edit_style(theme: &InlineRenameTheme, text: Option<TextStyle>) -> TextEditTheme {
    let mut style = theme.text_edit.clone();
    if text.is_some() {
        for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
            look.text = text;
        }
    }
    style
}
