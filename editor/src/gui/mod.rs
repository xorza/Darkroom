use std::{marker::PhantomData, ptr::NonNull};

use egui::{Painter, Rect, Ui};

use crate::{common::UiEquals, gui::style::Style};

pub mod background;
pub mod connection_breaker;
pub mod connection_ui;
pub mod const_bind_ui;
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

#[derive(Clone)]
struct GuiPainter(Painter);

impl std::fmt::Debug for GuiPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiPainter").finish()
    }
}

#[derive(Debug)]
pub struct Gui<'a> {
    ui: NonNull<Ui>,
    pub style: Style,
    painter: GuiPainter,
    pub rect: Rect,
    pub scale: f32,
    _marker: PhantomData<&'a mut Ui>,
}

impl<'a> Gui<'a> {
    pub fn new(ui: &'a mut Ui, style: Style) -> Self {
        let rect = ui.available_rect_before_wrap();
        let painter = GuiPainter(ui.painter_at(rect));

        Self {
            ui: NonNull::from(ui),
            style,
            painter,
            rect,
            scale: 1.0,
            _marker: PhantomData,
        }
    }

    pub fn ui(&mut self) -> &mut Ui {
        unsafe { self.ui.as_mut() }
    }

    pub fn painter(&self) -> Painter {
        self.painter.0.clone()
    }

    pub fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "gui scale must be finite");
        assert!(scale > 0.0, "gui scale must be greater than 0");

        if self.scale.ui_equals(&scale) {
            self.scale = scale;
            return;
        }

        self.scale = scale;
        self.style.set_scale(scale);
    }
}
