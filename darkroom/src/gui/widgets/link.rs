//! [`Link`]: text that does something when clicked, in the link ink.

use palantir::{Configure, Sense, Text, TextInput, Ui, WidgetId};

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// A text link: the accent at rest, lifted under the pointer, with an optional tooltip naming
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
        // Last frame's hover: this frame's response is known only after `show`.
        let ink = if ui.response_for(wid).hovered() {
            theme.colors.link_hovered
        } else {
            theme.colors.link
        };
        let style = support::colored_text(ui, ink, size.unwrap_or(theme.font.body));
        let response = Text::new(text)
            .id(wid)
            .style(&style)
            .sense(Sense::CLICK)
            .show(ui);
        let snapshot = response.snapshot();
        let clicked = response.left.clicked();
        support::tooltip_after(ui, &snapshot, tooltip);
        clicked
    }
}
