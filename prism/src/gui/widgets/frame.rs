use std::rc::Rc;

use egui::{Color32, CornerRadius, InnerResponse, Margin, Response, Sense, Stroke};

use crate::common::StableId;
use crate::gui::{
    Gui,
    style::{PopupStyle, Style},
};

#[derive(Debug, Clone)]
pub struct Frame {
    inner: egui::Frame,
    sense: Option<Sense>,
}

impl Frame {
    pub fn none() -> Self {
        Self {
            inner: egui::Frame::NONE,
            sense: None,
        }
    }

    pub fn popup(style: &PopupStyle) -> Self {
        Self {
            inner: egui::Frame::NONE
                .fill(style.fill)
                .stroke(style.stroke)
                .corner_radius(style.corner_radius)
                .inner_margin(style.padding),
            sense: None,
        }
    }

    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = Some(sense);
        self
    }

    pub fn fill(mut self, fill: Color32) -> Self {
        self.inner = self.inner.fill(fill);
        self
    }

    pub fn stroke(mut self, stroke: Stroke) -> Self {
        self.inner = self.inner.stroke(stroke);
        self
    }

    pub fn inner_margin(mut self, margin: impl Into<Margin>) -> Self {
        self.inner = self.inner.inner_margin(margin);
        self
    }

    pub fn outer_margin(mut self, margin: impl Into<Margin>) -> Self {
        self.inner = self.inner.outer_margin(margin);
        self
    }

    pub fn corner_radius(mut self, corner_radius: impl Into<CornerRadius>) -> Self {
        self.inner = self.inner.corner_radius(corner_radius);
        self
    }

    pub fn show<R>(
        self,
        gui: &mut Gui<'_>,
        id: StableId,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let sense = self.sense.unwrap_or(Sense::empty());
        let inner_frame = self.inner;

        // Anchor a stable-id scope OUTSIDE egui::Frame via
        // `Gui::scope`. egui::Frame::begin builds its content_ui
        // with `UiBuilder::new()` (no id_salt), so without this outer
        // scope the content_ui's widget id would be auto-generated from
        // the parent's widget counter — drifting whenever sibling
        // widgets toggled. `Gui::scope` sets `global_scope=true`
        // internally, pinning the outer scope's id verbatim.
        gui.scope(id).sense(sense).show(|gui| {
            let style = gui.style.clone();
            let result = inner_frame.show(gui.ui_raw(), |ui| {
                let mut gui = Gui::new(ui, &style);
                add_contents(&mut gui)
            });
            InnerResponse::new(result.inner, result.response)
        })
    }
}
