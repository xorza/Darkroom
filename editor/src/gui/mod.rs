use std::{marker::PhantomData, ptr::NonNull, rc::Rc};

use egui::{FontId, InnerResponse, Painter, Rect, Ui};

use crate::{common::UiEquals, gui::style::Style};

pub mod connection_breaker;
pub mod connection_ui;
pub mod const_bind_ui;
pub mod graph_background;
pub mod graph_ctx;
pub mod graph_layout;
pub mod graph_ui;
pub mod graph_ui_interaction;
pub mod log_ui;
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
    pub scale: f32,
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

    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }

    pub fn painter(&self) -> &Painter {
        self.ui.painter()
    }

    pub fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "gui scale must be finite");
        assert!(scale > 0.0, "gui scale must be greater than 0");

        if self.scale.ui_equals(&scale) {
            self.scale = scale;
            return;
        }

        self.scale = scale;
        Rc::make_mut(&mut self.style).set_scale(scale);
    }

    pub fn font_height(&mut self, font_id: FontId) -> f32 {
        self.ui.fonts_mut(|f| f.row_height(&font_id))
    }

    pub fn horizontal<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        self.ui.horizontal(|ui| {
            let mut gui = Gui::new(ui, &style);
            gui.scale = self.scale;
            add_contents(&mut gui)
        })
    }

    pub fn vertical<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        self.ui.vertical(|ui| {
            let mut gui = Gui::new(ui, &style);
            gui.scale = self.scale;
            add_contents(&mut gui)
        })
    }
}
