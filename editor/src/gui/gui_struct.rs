use std::marker::PhantomData;
use std::ptr::NonNull;

use egui::{Painter, Rect, Ui};

use crate::gui::style::Style;

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
    pub fn new(ui: &'a mut Ui, scale: f32) -> Self {
        assert!(scale.is_finite());

        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = GuiPainter(ui.painter_at(rect));
        Self {
            ui: NonNull::from(ui),
            style,
            painter,
            rect,
            scale,
            _marker: PhantomData,
        }
    }

    pub fn ui(&mut self) -> &mut Ui {
        unsafe { self.ui.as_mut() }
    }

    pub fn painter(&self) -> Painter {
        self.painter.0.clone()
    }
}
