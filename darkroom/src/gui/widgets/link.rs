//! [`Link`]: text that does something when clicked, in the link ink.

use palantir::{
    Button, ButtonTheme, Chrome, Configure, TextInput, TextStyleOverrides, Ui, WidgetId, WidgetLook,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// A text link: the link ink at rest, lifted under the pointer, with an optional tooltip naming
/// where it leads. Builder chain ending in [`show`](Self::show), like a palantir widget.
#[derive(Debug)]
pub(crate) struct Link<'a> {
    wid: WidgetId,
    text: TextInput<'a>,
    tooltip: Option<TextInput<'a>>,
    size: Option<f32>,
}

impl<'a> Link<'a> {
    pub(crate) fn new(wid: WidgetId, text: impl Into<TextInput<'a>>) -> Self {
        Self {
            wid,
            text: text.into(),
            tooltip: None,
            size: None,
        }
    }

    /// Shown under the pointer, for a destination the text does not spell out.
    pub(crate) fn tooltip(mut self, tooltip: impl Into<TextInput<'a>>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// The text's size. Default: the body tier.
    pub(crate) const fn size(mut self, px: f32) -> Self {
        self.size = Some(px);
        self
    }

    /// Record the link; `true` when it was clicked this frame.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) -> bool {
        let Self {
            wid,
            text,
            tooltip,
            size,
        } = self;
        let style = style(theme, size.unwrap_or(theme.font.body));
        let response = Button::new()
            .id(wid)
            .label(text)
            .focusable(false)
            .style(&style)
            .show(ui);
        let snapshot = response.snapshot();
        let clicked = response.clicked();
        support::tooltip_after(ui, &snapshot, tooltip);
        clicked
    }
}

/// A link's looks: no chrome, the text at `px` in the link ink, the hovered ink under the
/// pointer and while pressed.
fn style(theme: &Theme, px: f32) -> ButtonTheme {
    let look = |ink| WidgetLook {
        chrome: Chrome::NONE,
        text: TextStyleOverrides::NONE.with_font_size(px).with_color(ink),
    };
    let looks = support::hover_looks(look(theme.colors.link), look(theme.colors.link_hovered));
    support::flush_button(looks.clone(), looks)
}
