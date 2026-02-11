use std::{marker::PhantomData, ptr::NonNull, rc::Rc};

use egui::{Align, FontId, InnerResponse, Layout, Painter, Rect, Ui, UiBuilder};

use crate::{common::UiEquals, gui::style::Style};

pub mod connection_breaker;
pub mod connection_ui;
pub mod const_bind_ui;
pub mod graph_background;
pub mod graph_ctx;
pub mod graph_layout;
pub mod graph_ui;
pub mod graph_ui_interaction;
pub mod interaction_state;
pub mod log_ui;
pub mod new_node_ui;
pub mod node_details_ui;
pub mod node_layout;
pub mod node_ui;
pub mod polyline_mesh;
pub mod style;
pub mod style_settings;

// #[derive(Debug)]
pub struct Gui<'a> {
    ui: &'a mut Ui,
    pub style: Rc<Style>,
    pub rect: Rect,
    scale: f32,
    _marker: PhantomData<&'a mut Ui>,
}

impl<'a> Gui<'a> {
    pub fn new(ui: &'a mut Ui, style: &Rc<Style>) -> Self {
        let rect = ui.available_rect_before_wrap();

        Self {
            ui,
            style: Rc::clone(style),
            rect,
            scale: 1.0,
            _marker: PhantomData,
        }
    }
    pub fn new_with_scale(ui: &'a mut Ui, style: &Rc<Style>, scale: f32) -> Self {
        let rect = ui.available_rect_before_wrap();

        Self {
            ui,
            style: Rc::clone(style),
            rect,
            scale,
            _marker: PhantomData,
        }
    }

    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }

    pub fn painter(&self) -> &Painter {
        self.ui.painter()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "gui scale must be finite");
        assert!(scale > 0.0, "gui scale must be greater than 0");

        self.scale = scale;

        if self.style.scale.ui_equals(scale) {
            return;
        }

        Rc::make_mut(&mut self.style).set_scale(scale);
    }

    pub fn font_height(&mut self, font_id: &FontId) -> f32 {
        self.ui.fonts_mut(|f| f.row_height(font_id))
    }

    pub fn horizontal<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        self.ui.horizontal(|ui| {
            let mut gui = Gui::new_with_scale(ui, &style, self.scale);
            add_contents(&mut gui)
        })
    }

    /// Horizontal layout where children are stretched to fill the cross-axis (vertical) space.
    /// This allows ScrollAreas inside to expand to the full available height.
    pub fn horizontal_justified<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        self.ui.with_layout(
            Layout::left_to_right(Align::Min).with_cross_justify(true),
            |ui| {
                let mut gui = Gui::new_with_scale(ui, &style, self.scale);
                add_contents(&mut gui)
            },
        )
    }

    pub fn vertical<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        self.ui.vertical(|ui| {
            let mut gui = Gui::new_with_scale(ui, &style, self.scale);
            add_contents(&mut gui)
        })
    }

    pub fn new_child<R>(
        &mut self,
        builder: UiBuilder,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> R {
        let style = Rc::clone(&self.style);
        let mut child_ui = self.ui.new_child(builder);
        let mut gui = Gui::new_with_scale(&mut child_ui, &style, self.scale);
        add_contents(&mut gui)
    }

    /// Runs a closure with a temporarily changed scale, restoring the original scale afterward.
    pub fn with_scale<R>(&mut self, scale: f32, f: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let prev_scale = self.scale;
        self.set_scale(scale);
        let result = f(self);
        self.set_scale(prev_scale);
        result
    }
}
