//! Imperative layout constraints applied to the current scope's `Ui`.
//! Lets app-layer code say "this block must be at least N wide / at most
//! M tall / should eat all remaining horizontal space" without touching
//! raw egui.

use crate::gui::Gui;

#[derive(Debug, Default)]
pub struct Layout {
    min_width: Option<f32>,
    min_height: Option<f32>,
    max_height: Option<f32>,
    fill_width: bool,
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = Some(height);
        self
    }

    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = Some(height);
        self
    }

    /// Takes the full available width of the parent layout.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }

    pub fn apply(self, gui: &mut Gui<'_>) {
        let ui = gui.ui_raw();
        if let Some(w) = self.min_width {
            ui.set_min_width(w);
        }
        if let Some(h) = self.min_height {
            ui.set_min_height(h);
        }
        if let Some(h) = self.max_height {
            ui.set_max_height(h);
        }
        if self.fill_width {
            ui.take_available_width();
        }
    }
}
