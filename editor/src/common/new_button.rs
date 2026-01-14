use common::BoolExt;
use egui::{
    Atom, AtomExt as _, AtomKind, AtomLayout, AtomLayoutResponse, Color32, CornerRadius, Frame,
    Image, IntoAtoms, NumExt as _, Response, RichText, Sense, Stroke, TextStyle, TextWrapMode, Ui,
    Vec2, Widget, WidgetInfo, WidgetText, WidgetType,
};

/// Clickable button with text.
///
/// See also [`Ui::button`].
///
/// ```
/// # egui::__run_test_ui(|ui| {
/// # fn do_stuff() {}
///
/// if ui.add(egui::Button::new("Click me")).clicked() {
///     do_stuff();
/// }
///
/// // A greyed-out and non-interactive button:
/// if ui.add_enabled(false, egui::Button::new("Can't click this")).clicked() {
///     unreachable!();
/// }
/// # });
/// ```
#[must_use = "You should put this widget in a ui with `ui.add(widget);`"]
pub struct Button<'a> {
    text: &'a str,
    fill: Option<Color32>,
    stroke: Option<Stroke>,
    corner_radius: Option<CornerRadius>,
    selected: bool,
    sense: Sense,
}

impl<'a> Button<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            fill: None,
            stroke: None,
            corner_radius: None,
            selected: false,
            sense: Sense::click(),
        }
    }

    /// Override background fill color. Note that this will override any on-hover effects.
    /// Calling this will also turn on the frame.
    #[inline]
    pub fn fill(mut self, fill: impl Into<Color32>) -> Self {
        self.fill = Some(fill.into());
        self
    }

    /// Override button stroke. Note that this will override any on-hover effects.
    /// Calling this will also turn on the frame.
    #[inline]
    pub fn stroke(mut self, stroke: impl Into<Stroke>) -> Self {
        self.stroke = Some(stroke.into());
        self
    }

    /// By default, buttons senses clicks.
    /// Change this to a drag-button with `Sense::drag()`.
    #[inline]
    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = sense;
        self
    }

    /// Set the rounding of the button.
    #[inline]
    pub fn corner_radius(mut self, corner_radius: impl Into<CornerRadius>) -> Self {
        self.corner_radius = Some(corner_radius.into());
        self
    }

    /// If `true`, mark this button as "selected".
    #[inline]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Show the button and return a [`AtomLayoutResponse`] for painting custom contents.
    pub fn atom_ui(self, ui: &mut Ui) -> AtomLayoutResponse {
        let Button {
            text,
            fill,
            stroke,
            corner_radius,
            selected,
            sense,
        } = self;
        let button_padding = ui.spacing().button_padding;
        let text_color = selected.then_else(
            ui.style().visuals.selection.stroke.color,
            ui.style().visuals.widgets.inactive.fg_stroke.color,
        );

        let layout = AtomLayout::new(RichText::new(text).color(text_color))
            .sense(sense)
            .fallback_font(TextStyle::Button);

        let mut prepared = layout
            .frame(Frame::new().inner_margin(button_padding))
            // .min_size(min_size)
            .allocate(ui);

        let response = if ui.is_rect_visible(prepared.response.rect) {
            let visuals = ui.style().interact_selectable(&prepared.response, selected);

            prepared.fallback_text_color = visuals.text_color();

            let stroke = stroke.unwrap_or(visuals.bg_stroke);
            let fill = fill.unwrap_or(visuals.weak_bg_fill);
            prepared.frame = prepared
                .frame
                .inner_margin(
                    button_padding + Vec2::splat(visuals.expansion) - Vec2::splat(stroke.width),
                )
                .outer_margin(-Vec2::splat(visuals.expansion))
                .fill(fill)
                .stroke(stroke)
                .corner_radius(corner_radius.unwrap_or(visuals.corner_radius));

            prepared.paint(ui)
        } else {
            AtomLayoutResponse::empty(prepared.response)
        };

        response
            .response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), text));

        response
    }
}

impl Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.atom_ui(ui).response
    }
}
